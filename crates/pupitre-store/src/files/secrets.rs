// SPDX-License-Identifier: MIT OR Apache-2.0
//! The secrets of the backend `files` (2.24, 2.31, 2.34): the value in clear text in a file
//! with the mode 0600, the time and the subject of the last write, and the events.
//!
//! ```text
//! values/<scope folder>/<name>        the bytes of the value
//! meta/<scope folder>/<name>.json     the time and the subject of the last write
//! events/<scope folder>/<name>.jsonl  one event on each line
//! ```

use pupitre_core::ids::{EnvId, KeyPath, Subject, UnitId};
use pupitre_core::json::{self, Json, ReadLimits};
use pupitre_core::node::SecretScope;
use pupitre_core::time::UtcMillis;

use super::fsio::{Dir, FType};
use super::meta::{MAX_SMALL_FILE, decode_secret_meta, encode_secret_meta};
use crate::secret::{SecretValue, StoreKeys};
use crate::transfer::{SecretAction, SecretEvent, SecretExport};
use crate::types::{SecretMeta, StoreError, WriteMeta};

/// The largest secret file that the backend reads. A value has 65,536 bytes at most (2.25).
const MAX_VALUE_FILE: usize = 1 << 20;
/// The longest secret name that the layout of 2.31 holds: a file name has 255 bytes at most, and
/// the longest suffix of a name is `.jsonl`.
const MAX_NAME_BYTES: usize = 255 - ".jsonl".len();
/// The largest file of events that the backend reads.
const MAX_EVENTS_FILE: usize = 64 << 20;
/// The name of the folder of the unit scopes inside the folder of an environment scope.
const UNITS: &str = "units";

/// The secrets folder.
pub(crate) struct SecretStore {
    root: Dir,
    tmp: Dir,
    keys: StoreKeys,
    history_keep: u32,
}

impl std::fmt::Debug for SecretStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretStore").field("root", &self.root.label()).finish()
    }
}

fn segments(tree: &'static str, scope: &SecretScope) -> Vec<String> {
    let mut parts = vec![tree.to_string()];
    parts.extend(scope.folder().split('/').map(str::to_string));
    parts
}

fn as_refs(parts: &[String]) -> Vec<&str> {
    parts.iter().map(String::as_str).collect()
}

fn event_line(scope: &SecretScope, name: &KeyPath, action: SecretAction, by: &Subject, at: UtcMillis) -> Vec<u8> {
    let action = match action {
        SecretAction::Set => "set",
        SecretAction::Delete => "delete",
    };
    json::to_compact(&Json::object([
        ("action", Json::str(action)),
        ("at", Json::str(at.to_string())),
        ("by", Json::str(by.as_str())),
        ("name", Json::str(name.as_str())),
        ("scope", Json::str(scope.text())),
    ]))
}

fn parse_event(line: &[u8], place: &str) -> Result<SecretEvent, StoreError> {
    let corrupt = |what: &str| StoreError::Corrupt(format!("{place}: {what}"));
    let value = json::parse(line, ReadLimits::bytes(MAX_SMALL_FILE)).map_err(|e| corrupt(&e.to_string()))?;
    let obj = value.as_object().ok_or_else(|| corrupt("an event is not an object"))?;
    let action = match obj.get("action").and_then(Json::as_str) {
        Some("set") => SecretAction::Set,
        Some("delete") => SecretAction::Delete,
        _ => return Err(corrupt("the action of an event is not known")),
    };
    let at = obj.get("at").and_then(Json::as_str).and_then(UtcMillis::parse).ok_or_else(|| corrupt("at"))?;
    let by = obj.get("by").and_then(Json::as_str).and_then(|t| Subject::parse(t).ok()).ok_or_else(|| corrupt("by"))?;
    Ok(SecretEvent { action, by, at })
}

impl SecretStore {
    pub(crate) fn new(root: Dir, tmp: Dir, keys: StoreKeys, history_keep: u32) -> SecretStore {
        SecretStore { root, tmp, keys, history_keep }
    }

    /// An environment scope cannot hold a secret named `units`: the name is the name of the
    /// folder of the unit scopes in the layout of 2.31.
    fn check_name(scope: &SecretScope, name: &KeyPath) -> Result<(), StoreError> {
        if name.as_str().len() > MAX_NAME_BYTES {
            return Err(StoreError::Unavailable(format!(
                "the secret name has {} bytes: the backend files accepts {MAX_NAME_BYTES} at most (2.31)",
                name.as_str().len()
            )));
        }
        if matches!(scope, SecretScope::Environment(_)) && name.as_str() == UNITS {
            return Err(StoreError::Unavailable(format!(
                "the secret name {UNITS} in an environment scope collides with the layout of 2.31"
            )));
        }
        Ok(())
    }

    fn dir(&self, tree: &'static str, scope: &SecretScope, create: bool) -> Result<Option<Dir>, StoreError> {
        let parts = segments(tree, scope);
        Ok(self.root.open_path(&as_refs(&parts), create)?)
    }

    fn names(&self, scope: &SecretScope) -> Result<Vec<KeyPath>, StoreError> {
        let Some(dir) = self.dir("values", scope, false)? else { return Ok(Vec::new()) };
        let mut names: Vec<KeyPath> = dir
            .list()?
            .into_iter()
            .filter(|i| i.ftype == FType::File && i.name_ok)
            .filter_map(|i| KeyPath::parse(&i.name).ok())
            .collect();
        names.sort();
        Ok(names)
    }

    fn read_value(&self, scope: &SecretScope, name: &KeyPath) -> Result<SecretValue, StoreError> {
        let dir = self.dir("values", scope, false)?.ok_or(StoreError::NotFound)?;
        match dir.read_file_opt(name.as_str(), MAX_VALUE_FILE)? {
            Some((bytes, _)) if bytes.len() > MAX_VALUE_FILE => Err(StoreError::Corrupt(format!(
                "{}/{name}: the secret value file is above the limit of {MAX_VALUE_FILE} bytes",
                dir.label()
            ))),
            Some((bytes, _)) => Ok(SecretValue::new(bytes)),
            None => Err(StoreError::NotFound),
        }
    }

    fn secret_meta(&self, scope: &SecretScope, name: &KeyPath, value: &SecretValue) -> Result<SecretMeta, StoreError> {
        let dir = self.dir("meta", scope, false)?.ok_or_else(|| StoreError::Corrupt(format!("meta of {name}")))?;
        let file = format!("{name}.json");
        let (bytes, _) = dir.read_file(&file, MAX_SMALL_FILE)?;
        let (updated_at, updated_by) = decode_secret_meta(&bytes, &format!("{}/{file}", dir.label()))?;
        Ok(SecretMeta {
            scope: scope.clone(),
            name: name.clone(),
            fingerprint: self.keys.fingerprint(scope, name, value),
            updated_at,
            updated_by,
        })
    }

    /// Give the metadata of the secrets of a scope, in the byte order of the name.
    pub(crate) fn list(&self, scope: &SecretScope) -> Result<Vec<SecretMeta>, StoreError> {
        let mut out = Vec::new();
        for name in self.names(scope)? {
            let value = match self.read_value(scope, &name) {
                Ok(v) => v,
                Err(StoreError::NotFound) => continue,
                Err(e) => return Err(e),
            };
            out.push(self.secret_meta(scope, &name, &value)?);
        }
        Ok(out)
    }

    fn write_events(&self, scope: &SecretScope, name: &KeyPath, mut lines: Vec<Vec<u8>>) -> Result<(), StoreError> {
        let keep = usize::try_from(self.history_keep).unwrap_or(usize::MAX);
        if lines.len() > keep {
            lines.drain(..lines.len() - keep);
        }
        let mut bytes = Vec::new();
        for line in lines {
            bytes.extend_from_slice(&line);
            bytes.push(b'\n');
        }
        let dir = self.dir("events", scope, true)?.ok_or_else(|| StoreError::Corrupt("events folder".into()))?;
        dir.write_atomic(&self.tmp, &format!("{name}.jsonl"), &bytes)?;
        Ok(())
    }

    fn read_event_lines(&self, scope: &SecretScope, name: &KeyPath) -> Result<Vec<Vec<u8>>, StoreError> {
        let Some(dir) = self.dir("events", scope, false)? else { return Ok(Vec::new()) };
        let Some((bytes, _)) = dir.read_file_opt(&format!("{name}.jsonl"), MAX_EVENTS_FILE)? else {
            return Ok(Vec::new());
        };
        Ok(bytes.split(|b| *b == b'\n').filter(|l| !l.is_empty()).map(<[u8]>::to_vec).collect())
    }

    fn add_event(
        &self,
        scope: &SecretScope,
        name: &KeyPath,
        action: SecretAction,
        meta: &WriteMeta,
    ) -> Result<(), StoreError> {
        let mut lines = self.read_event_lines(scope, name)?;
        lines.push(event_line(scope, name, action, &meta.saved_by, meta.saved_at));
        self.write_events(scope, name, lines)
    }

    /// Write a secret and its event.
    pub(crate) fn put(
        &self,
        scope: &SecretScope,
        name: &KeyPath,
        value: &SecretValue,
        meta: &WriteMeta,
    ) -> Result<SecretMeta, StoreError> {
        Self::check_name(scope, name)?;
        // The metadata comes first: a crash then leaves a metadata file with no value, which
        // the list does not show.
        let meta_dir = self.dir("meta", scope, true)?.ok_or_else(|| StoreError::Corrupt("meta folder".into()))?;
        meta_dir.write_atomic(
            &self.tmp,
            &format!("{name}.json"),
            &encode_secret_meta(meta.saved_at, &meta.saved_by),
        )?;
        let value_dir = self.dir("values", scope, true)?.ok_or_else(|| StoreError::Corrupt("values folder".into()))?;
        value_dir.write_atomic(&self.tmp, name.as_str(), value.expose())?;
        self.add_event(scope, name, SecretAction::Set, meta)?;
        Ok(SecretMeta {
            scope: scope.clone(),
            name: name.clone(),
            fingerprint: self.keys.fingerprint(scope, name, value),
            updated_at: meta.saved_at,
            updated_by: meta.saved_by.clone(),
        })
    }

    /// Delete a secret and write its event.
    pub(crate) fn delete(&self, scope: &SecretScope, name: &KeyPath, meta: &WriteMeta) -> Result<(), StoreError> {
        Self::check_name(scope, name)?;
        let value_dir = self.dir("values", scope, false)?.ok_or(StoreError::NotFound)?;
        if value_dir.stat_opt(name.as_str())?.is_none() {
            return Err(StoreError::NotFound);
        }
        value_dir.unlink(name.as_str())?;
        value_dir.fsync()?;
        if let Some(meta_dir) = self.dir("meta", scope, false)? {
            meta_dir.unlink_opt(&format!("{name}.json"))?;
            meta_dir.fsync()?;
        }
        self.add_event(scope, name, SecretAction::Delete, meta)
    }

    /// Give the values of the secrets of the scopes.
    pub(crate) fn read(&self, scopes: &[SecretScope]) -> Result<Vec<(SecretScope, KeyPath, SecretValue)>, StoreError> {
        let mut out = Vec::new();
        for scope in scopes {
            for name in self.names(scope)? {
                match self.read_value(scope, &name) {
                    Ok(v) => out.push((scope.clone(), name, v)),
                    Err(StoreError::NotFound) => {}
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(out)
    }

    // Transfer.

    /// Each scope that holds a secret value or the events of a secret, in byte order.
    pub(crate) fn scopes(&self) -> Result<Vec<SecretScope>, StoreError> {
        let mut out = self.scopes_of("values")?;
        out.extend(self.scopes_of("events")?);
        out.sort_by_key(SecretScope::text);
        out.dedup();
        Ok(out)
    }

    /// Each scope that has a folder in one tree (`values` or `events`), in byte order.
    fn scopes_of(&self, tree: &'static str) -> Result<Vec<SecretScope>, StoreError> {
        let mut out = Vec::new();
        let Some(envs) = self.root.open_path(&[tree, "environments"], false)? else { return Ok(out) };
        let mut env_items = envs.list()?;
        env_items.sort_by(|a, b| a.name.cmp(&b.name));
        for e in env_items {
            let Ok(env) = EnvId::parse(&e.name) else { continue };
            if e.ftype != FType::Dir || !e.name_ok {
                continue;
            }
            let env_dir = envs.open_dir(&e.name)?;
            out.push(SecretScope::Environment(env.clone()));
            if let Some(units) = env_dir.open_path(&[UNITS], false)? {
                let mut unit_items = units.list()?;
                unit_items.sort_by(|a, b| a.name.cmp(&b.name));
                for u in unit_items {
                    if let (Ok(unit), FType::Dir, true) = (UnitId::parse(&u.name), u.ftype, u.name_ok) {
                        out.push(SecretScope::Unit(env.clone(), unit));
                    }
                }
            }
        }
        Ok(out)
    }

    /// The names that have a file of events in a scope, in byte order.
    fn names_with_events(&self, scope: &SecretScope) -> Result<Vec<KeyPath>, StoreError> {
        let Some(dir) = self.dir("events", scope, false)? else { return Ok(Vec::new()) };
        let mut names: Vec<KeyPath> = dir
            .list()?
            .into_iter()
            .filter(|i| i.ftype == FType::File && i.name_ok)
            .filter_map(|i| i.name.strip_suffix(".jsonl").and_then(|n| KeyPath::parse(n).ok()))
            .collect();
        names.sort();
        Ok(names)
    }

    /// Each secret with its value and its events. A secret that was deleted has no value, and
    /// its events stay in the answer (2.24).
    pub(crate) fn export(&self) -> Result<Vec<SecretExport>, StoreError> {
        let mut out = Vec::new();
        for scope in self.scopes()? {
            let mut names = self.names(&scope)?;
            names.extend(self.names_with_events(&scope)?);
            names.sort();
            names.dedup();
            for name in names {
                let value = match self.read_value(&scope, &name) {
                    Ok(v) => Some(v),
                    Err(StoreError::NotFound) => None,
                    Err(e) => return Err(e),
                };
                let place = format!("{}/events/{}/{name}.jsonl", self.root.label(), scope.folder());
                let events = self
                    .read_event_lines(&scope, &name)?
                    .iter()
                    .map(|l| parse_event(l, &place))
                    .collect::<Result<Vec<_>, _>>()?;
                out.push(SecretExport { scope: scope.clone(), name, value, events });
            }
        }
        Ok(out)
    }

    /// True when the folder holds no secret value and no event of a secret.
    pub(crate) fn is_empty(&self) -> Result<bool, StoreError> {
        for scope in self.scopes()? {
            if !self.names(&scope)?.is_empty() || !self.names_with_events(&scope)?.is_empty() {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Write one secret of an import with its events.
    pub(crate) fn import(&self, secret: &SecretExport, now: UtcMillis) -> Result<(), StoreError> {
        Self::check_name(&secret.scope, &secret.name)?;
        if self.read_value(&secret.scope, &secret.name).is_ok() {
            return Err(StoreError::AlreadyExists);
        }
        let Some(value) = &secret.value else {
            // A deleted secret: the events only. No value file and no metadata file.
            let lines = secret
                .events
                .iter()
                .map(|e| event_line(&secret.scope, &secret.name, e.action, &e.by, e.at))
                .collect::<Vec<_>>();
            return self.write_events(&secret.scope, &secret.name, lines);
        };
        let (at, by) = match secret.events.last() {
            Some(e) => (e.at, e.by.clone()),
            None => (now, Subject::parse(Subject::IMPORT).unwrap_or_else(|_| Subject::anonymous())),
        };
        let meta_dir =
            self.dir("meta", &secret.scope, true)?.ok_or_else(|| StoreError::Corrupt("meta folder".into()))?;
        meta_dir.write_atomic(&self.tmp, &format!("{}.json", secret.name), &encode_secret_meta(at, &by))?;
        let value_dir =
            self.dir("values", &secret.scope, true)?.ok_or_else(|| StoreError::Corrupt("values folder".into()))?;
        value_dir.write_atomic(&self.tmp, secret.name.as_str(), value.expose())?;
        let lines = secret
            .events
            .iter()
            .map(|e| event_line(&secret.scope, &secret.name, e.action, &e.by, e.at))
            .collect::<Vec<_>>();
        self.write_events(&secret.scope, &secret.name, lines)
    }
}
