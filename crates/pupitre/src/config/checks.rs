// SPDX-License-Identifier: MIT OR Apache-2.0
//! Stage 2 (6.13): the checks of the values that the table of keys cannot express. Stage 3
//! (6.14): the contradictions. And the warnings (6.15).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hclconf::fsio::{ReadError, read_regular_file};
use hclconf::value::format_duration;
use hclconf::{ConfigError, ResolvedKey, Value};
use pupitre_core::capability::{Capabilities, PlaybookKind, PlaybookSpec, SchemaSpec};
use pupitre_core::command::{CommandSpec, MAX_PARAMS, ParamSpec, ParamType, Target};
use pupitre_core::ids::{CommandId, ParamName, PlaybookId, SchemaId};
use pupitre_core::schema::{MAX_SCHEMA_FILE_BYTES, UnitSchema, check_schema_set};
use pupitre_store::secret::SecretKey;

use super::url::{parse_http_url, rule_text};
use super::{ReadMode, View, ca, keys};

/// The values that stage 2 reads or parses, for the typed configuration.
#[derive(Debug, Clone)]
pub(crate) struct Stage2 {
    pub(crate) secret_key: SecretKey,
    pub(crate) telemetry_ca: Option<Vec<u8>>,
    pub(crate) pg_ca: Option<Vec<u8>>,
    pub(crate) pdp_ca: Option<Vec<u8>>,
    pub(crate) capabilities: Capabilities,
}

fn err(key: &ResolvedKey, message: impl Into<String>) -> ConfigError {
    ConfigError::at(key.display.clone(), message, key.line)
}

fn shown(key: &ResolvedKey) -> String {
    if key.secret { "(value hidden)".to_string() } else { key.value.print() }
}

/// The rule of 6.6: an absolute path with no `.` or `..` segment, no repeated `/` and no `/`
/// at the end, except the path `/`.
fn canonical_path(text: &str) -> bool {
    if text == "/" {
        return true;
    }
    text.starts_with('/')
        && !text.ends_with('/')
        && text[1..].split('/').all(|s| !s.is_empty() && s != "." && s != "..")
}

fn base_path_ok(text: &str) -> bool {
    if text == "/" {
        return true;
    }
    let Some(inner) = text.strip_prefix('/').and_then(|t| t.strip_suffix('/')) else {
        return false;
    };
    inner.split('/').all(|seg| {
        (1..=64).contains(&seg.len())
            && seg != "."
            && seg != ".."
            && seg.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-'))
    })
}

fn tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

const FORBIDDEN_HEADERS: [&str; 6] =
    ["authorization", "proxy-authorization", "cookie", "host", "content-type", "content-length"];

fn read_ca(v: &View<'_>, path: &str, errors: &mut Vec<ConfigError>) -> Option<Vec<u8>> {
    let key = v.key(path)?;
    let text = v.text(path)?;
    match ca::read_ca_file(text) {
        Ok(bytes) => Some(bytes),
        Err(message) => {
            errors.push(err(key, message));
            None
        }
    }
}

fn read_schema(id: &SchemaId, key: &ResolvedKey, path: &str, errors: &mut Vec<ConfigError>) -> Option<SchemaSpec> {
    let bytes = match read_regular_file(Path::new(path), MAX_SCHEMA_FILE_BYTES as u64) {
        Ok(b) => b,
        Err(ReadError::TooLarge(_)) => {
            errors.push(err(
                key,
                format!("cannot read the file {path}: the file has more than {MAX_SCHEMA_FILE_BYTES} bytes"),
            ));
            return None;
        }
        Err(e) => {
            errors.push(err(key, format!("cannot read the file {path}: {e}")));
            return None;
        }
    };
    match UnitSchema::parse(&bytes) {
        Ok(schema) => Some(SchemaSpec { id: id.clone(), path: PathBuf::from(path), schema }),
        Err(problems) => {
            for p in problems.iter().take(10) {
                errors.push(err(key, format!("the schema file {path} is not valid: {p}")));
            }
            None
        }
    }
}

/// The text of a key of a labelled block, or of an inner labelled block.
fn label_text<'a>(v: &View<'a>, label: &str, sub: Option<&str>, path: &str) -> Option<&'a ResolvedKey> {
    match sub {
        None => v.checked.get_in(label, path),
        Some(s) => v.checked.get_in_sub(label, s, path),
    }
}

fn chars_1_to_64(key: &ResolvedKey, errors: &mut Vec<ConfigError>) -> Option<String> {
    let Value::Str(text) = &key.value else { return None };
    let n = text.chars().count();
    if n == 0 || n > 64 {
        errors.push(err(key, format!("expected 1 to 64 characters, found {n} characters")));
        return None;
    }
    Some(text.clone())
}

fn param_of(v: &View<'_>, command: &str, name: &str, errors: &mut Vec<ConfigError>) -> Option<ParamSpec> {
    let block = v.checked.block_in_sub(command, name, "command.param")?;
    let param_name = match ParamName::parse(name) {
        Ok(n) => Some(n),
        Err(e) => {
            errors.push(ConfigError::at(block.display.clone(), e.to_string(), block.line));
            None
        }
    };
    let get = |path: &str| label_text(v, command, Some(name), path);
    let label = get("command.param.label").and_then(|k| chars_1_to_64(k, errors));
    let required = get("command.param.required").and_then(|k| match k.value {
        Value::Bool(b) => Some(b),
        _ => None,
    });
    let ty_name = get("command.param.type").and_then(|k| match &k.value {
        Value::Str(s) => Some(s.clone()),
        _ => None,
    })?;
    let int = |path: &str| {
        get(path).and_then(|k| match k.value {
            Value::Int(n) => Some(n),
            _ => None,
        })
    };
    let ms = |path: &str| {
        get(path).and_then(|k| match k.value {
            Value::DurationMs(n) => Some(n),
            _ => None,
        })
    };
    let permitted: &[&str] = match ty_name.as_str() {
        "text" | "integer" => &["command.param.min", "command.param.max"],
        "duration" => &["command.param.min_duration", "command.param.max_duration"],
        "choice" => &["command.param.choices"],
        _ => &[],
    };
    let mut ok = true;
    for path in [
        "command.param.min",
        "command.param.max",
        "command.param.min_duration",
        "command.param.max_duration",
        "command.param.choices",
    ] {
        match (permitted.contains(&path), get(path)) {
            (false, Some(k)) => {
                errors.push(err(k, format!("not permitted for the type {ty_name}")));
                ok = false;
            }
            (true, None) => {
                errors.push(ConfigError::at(
                    format!("{}.{}", block.display, path.rsplit('.').next().unwrap_or(path)),
                    format!("missing attribute; necessary for the type {ty_name}"),
                    block.line,
                ));
                ok = false;
            }
            _ => {}
        }
    }
    let ty = match ty_name.as_str() {
        "text" | "integer" => {
            let (min, max) = (int("command.param.min")?, int("command.param.max")?);
            if min > max {
                errors.push(ConfigError::at(
                    block.display.clone(),
                    format!("min {min} is above max {max}"),
                    block.line,
                ));
                return None;
            }
            if ty_name == "text" { ParamType::Text { min, max } } else { ParamType::Integer { min, max } }
        }
        "duration" => {
            let (min_ms, max_ms) = (ms("command.param.min_duration")?, ms("command.param.max_duration")?);
            if min_ms > max_ms {
                errors.push(ConfigError::at(
                    block.display.clone(),
                    format!(
                        "min_duration {} is above max_duration {}",
                        format_duration(min_ms),
                        format_duration(max_ms)
                    ),
                    block.line,
                ));
                return None;
            }
            ParamType::Duration { min_ms, max_ms }
        }
        "choice" => {
            let k = get("command.param.choices")?;
            let Value::List(items) = &k.value else { return None };
            let mut choices: Vec<String> = Vec::new();
            for item in items {
                if let Value::Str(t) = item {
                    if choices.contains(t) {
                        errors.push(err(k, format!("the choice {} occurs two times", hclconf::value::json_string(t))));
                        return None;
                    }
                    choices.push(t.clone());
                }
            }
            ParamType::Choice(choices)
        }
        _ => ParamType::Boolean,
    };
    if !ok {
        return None;
    }
    Some(ParamSpec { name: param_name?, label: label?, required: required?, ty })
}

fn command_of(v: &View<'_>, id: &str, errors: &mut Vec<ConfigError>) -> Option<CommandSpec> {
    let block = v.checked.block_in(id, "command")?;
    let command_id = match CommandId::parse(id) {
        Ok(c) => Some(c),
        Err(e) => {
            errors.push(ConfigError::at(block.display.clone(), e.to_string(), block.line));
            None
        }
    };
    let get = |path: &str| label_text(v, id, None, path);
    let label = get("command.label").and_then(|k| chars_1_to_64(k, errors));
    let playbook = get("command.playbook").and_then(|k| match &k.value {
        Value::Str(t) => PlaybookId::parse(t).map_err(|e| errors.push(err(k, e.to_string()))).ok(),
        _ => None,
    });
    let target = get("command.target").and_then(|k| match &k.value {
        Value::Str(t) if t == "environment" => Some(Target::Environment),
        Value::Str(t) if t == "unit" => Some(Target::Unit),
        Value::Str(t) if t == "host" => Some(Target::Host),
        _ => None,
    });
    let check = get("command.check").and_then(|k| match k.value {
        Value::Bool(b) => Some(b),
        _ => None,
    });
    let names = v.checked.sub_labels(id, "command.param");
    if names.len() > MAX_PARAMS {
        return None;
    }
    let params: Vec<ParamSpec> = names.iter().filter_map(|n| param_of(v, id, n, errors)).collect();
    if params.len() != names.len() {
        return None;
    }
    Some(CommandSpec { id: command_id?, label: label?, playbook: playbook?, target: target?, check: check?, params })
}

/// The key `server.allowed_hosts` (7.4): each entry is a host with an optional port, each entry
/// is there one time, and the key is necessary when `server.listen` is not a loopback address.
fn check_allowed_hosts(v: &View<'_>, errors: &mut Vec<ConfigError>) {
    match v.key("server.allowed_hosts") {
        Some(key) => {
            let Value::List(items) = &key.value else { return };
            let mut seen: Vec<super::hosts::HostPattern> = Vec::new();
            for item in items {
                let Value::Str(text) = item else { continue };
                match super::hosts::HostPattern::parse(text) {
                    Err(expected) => errors
                        .push(err(key, format!("expected {expected}, found {}", hclconf::value::json_string(text)))),
                    Ok(p) if seen.contains(&p) => errors
                        .push(err(key, format!("the entry {} occurs two times", hclconf::value::json_string(text)))),
                    Ok(p) => seen.push(p),
                }
            }
        }
        None => {
            let Some(listen) = v.text("server.listen").and_then(|t| t.parse::<std::net::SocketAddr>().ok()) else {
                return;
            };
            if super::hosts::default_hosts(listen).is_none() {
                errors.push(ConfigError::new(
                    "server.allowed_hosts",
                    format!(
                        "missing attribute; necessary when `server.listen` is not a loopback address, found `server.listen` = {}",
                        hclconf::value::json_string(&listen.to_string())
                    ),
                    v.key("server.listen").map(|k| k.line),
                ));
            }
        }
    }
}

/// Run the checks of stage 2 (6.13). The answer is the list of errors of the stage.
pub(crate) fn stage2(v: &View<'_>, mode: ReadMode) -> Result<Stage2, Vec<ConfigError>> {
    let mut errors = Vec::new();
    let _ = mode;

    // Each path of the file is canonical (6.6, 7.7).
    for key in v.checked.keys() {
        let is_path = keys::SCHEMA.key(key.path).is_some_and(|d| matches!(d.kind, hclconf::Kind::Path));
        if let (true, Value::Str(text)) = (is_path, &key.value)
            && !canonical_path(text)
        {
            errors.push(err(key, format!("expected a canonical absolute path with no `.` or `..` segment, no repeated `/` and no `/` at the end, found {}", shown(key))));
        }
    }
    if let (Some(key), Some(text)) = (v.key("server.base_path"), v.text("server.base_path"))
        && !base_path_ok(text)
    {
        errors.push(err(key, format!("expected `/` or a path that starts and ends with `/` with segments of 1 to 64 characters from `A-Z a-z 0-9 . _ ~ -`, found {}", shown(key))));
    }
    check_allowed_hosts(v, &mut errors);
    let secret_key = match (v.key("storage.secret_key"), v.text("storage.secret_key")) {
        (Some(key), Some(text)) => match SecretKey::parse(text) {
            Some(k) => Some(k),
            None => {
                errors.push(err(key, "expected 64 lower-case hexadecimal characters, found (value hidden)"));
                None
            }
        },
        _ => None,
    };
    {
        let path = "telemetry.otlp_endpoint";
        if let (Some(key), Some(text)) = (v.key(path), v.text(path))
            && parse_http_url(text, false).is_none()
        {
            errors.push(err(key, format!("expected {}, found {}", rule_text(false), shown(key))));
        }
    }
    if let (Some(key), Some(text)) = (v.key("access.pdp.url"), v.text("access.pdp.url"))
        && parse_http_url(text, true).is_none()
    {
        errors.push(err(key, format!("expected {}, found {}", rule_text(true), shown(key))));
    }
    if let (Some(key), Some(text)) = (v.key("access.identity.header"), v.text("access.identity.header"))
        && (!text.bytes().all(tchar) || FORBIDDEN_HEADERS.contains(&text.to_ascii_lowercase().as_str()))
    {
        errors.push(err(
            key,
            format!("expected 1 to 64 token characters that do not name a reserved header, found {}", shown(key)),
        ));
    }
    if let (Some(key), Some(text)) = (v.key("access.identity.public_keys"), v.text("access.identity.public_keys"))
        && let Err(message) = crate::access::check_public_keys(text)
    {
        errors.push(err(key, message));
    }
    // 6.4: two durations are whole numbers of seconds.
    for path in ["access.identity.max_age", "access.identity.clock_skew"] {
        if let (Some(key), Some(d)) = (v.key(path), v.duration(path))
            && d.subsec_millis() != 0
        {
            errors.push(err(key, format!("expected a whole number of seconds, found {}", shown(key))));
        }
    }
    let telemetry_ca = read_ca(v, "telemetry.ca_file", &mut errors);
    let pg_ca = read_ca(v, "storage.postgres.tls.ca_file", &mut errors);
    let pdp_ca = read_ca(v, "access.pdp.ca_file", &mut errors);

    // The catalog of capabilities (4.2).
    let mut capabilities = Capabilities::default();
    for label in v.checked.labels("schema") {
        let Some(block) = v.checked.block_in(label, "schema") else { continue };
        let id = match SchemaId::parse(label) {
            Ok(id) => id,
            Err(e) => {
                errors.push(ConfigError::at(block.display.clone(), e.to_string(), block.line));
                continue;
            }
        };
        if let (Some(key), Some(Value::Str(path))) =
            (v.checked.get_in(label, "schema.path"), v.checked.get_in(label, "schema.path").map(|k| &k.value))
            && let Some(spec) = read_schema(&id, key, path, &mut errors)
        {
            capabilities.schemas.insert(id, spec);
        }
    }
    for label in v.checked.labels("playbook") {
        let Some(block) = v.checked.block_in(label, "playbook") else { continue };
        let id = match PlaybookId::parse(label) {
            Ok(id) => id,
            Err(e) => {
                errors.push(ConfigError::at(block.display.clone(), e.to_string(), block.line));
                continue;
            }
        };
        let kind = v.checked.get_in(label, "playbook.kind").and_then(|k| match &k.value {
            Value::Str(t) => PlaybookKind::from_name(t),
            _ => None,
        });
        let path = v.checked.get_in(label, "playbook.path").and_then(|k| match &k.value {
            Value::Str(t) => Some(PathBuf::from(t)),
            _ => None,
        });
        if let (Some(kind), Some(path)) = (kind, path) {
            capabilities.playbooks.insert(id.clone(), PlaybookSpec { id, kind, path });
        }
    }
    for label in v.checked.labels("command") {
        if let Some(spec) = command_of(v, label, &mut errors) {
            capabilities.commands.insert(spec.id.clone(), spec);
        }
    }
    if !errors.is_empty() {
        return Err(hclconf::error::limit_errors(errors));
    }
    match secret_key {
        Some(secret_key) => Ok(Stage2 { secret_key, telemetry_ca, pg_ca, pdp_ca, capabilities }),
        None => Err(vec![ConfigError::new(
            "storage.secret_key",
            "missing secret; give `secret_key_env` or `secret_key_file`",
            None,
        )]),
    }
}

fn nested(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

/// The contradictions of 6.14. Each one is an error with the two keys.
pub(crate) fn contradictions(v: &View<'_>, stage: &Stage2) -> Vec<ConfigError> {
    let mut errors = Vec::new();
    let line = |path: &str| v.key(path).map(|k| k.line);
    // C1: two listeners on one port.
    if let (Some(a), Some(b)) = (v.text("server.listen"), v.text("server.ops_listen")) {
        let port = |t: &str| t.parse::<std::net::SocketAddr>().ok().map(|s| s.port());
        if port(a).is_some() && port(a) == port(b) {
            errors.push(ConfigError::new(
                "server.ops_listen",
                "the port is the port of `server.listen`",
                line("server.ops_listen"),
            ));
        }
    }
    let enabled = v.bool("runner.enabled") == Some(true);
    // C2: the backend files has one instance, so it is the runner node.
    if v.text("storage.backend") == Some("files") && enabled && v.bool("runner.execute") == Some(false) {
        errors.push(ConfigError::new(
            "runner.execute",
            "the backend files has one instance, so `runner.execute` is true when `runner.enabled = true`",
            line("runner.execute"),
        ));
    }
    // C3: a block playbook or command needs runner.enabled = true.
    if !enabled {
        for path in ["playbook", "command"] {
            for b in v.checked.blocks().iter().filter(|b| b.path == path) {
                errors.push(ConfigError::at(b.display.clone(), "not permitted when `runner.enabled` is false", b.line));
            }
        }
    }
    // C4: a command names a playbook of kind command.
    for c in stage.capabilities.commands.values() {
        if stage.capabilities.playbook_of_kind(&c.playbook, PlaybookKind::Command).is_none() {
            errors.push(ConfigError::new(
                format!("command \"{}\".playbook", c.id),
                format!(
                    "expected the id of a block playbook with the kind command, found {}",
                    hclconf::value::json_string(c.playbook.as_str())
                ),
                v.checked.get_in(c.id.as_str(), "command.playbook").map(|k| k.line),
            ));
        }
    }
    // C5: one block playbook for each file.
    let mut paths: BTreeMap<&Path, &PlaybookId> = BTreeMap::new();
    for p in stage.capabilities.playbooks.values() {
        if let Some(other) = paths.insert(p.path.as_path(), &p.id) {
            errors.push(ConfigError::new(
                format!("playbook \"{}\".path", p.id),
                format!("the path is the path of `playbook \"{other}\"`"),
                v.checked.get_in(p.id.as_str(), "playbook.path").map(|k| k.line),
            ));
        }
    }
    // C6: the folders of the store and of the runner are not inside each other.
    let mut folders: Vec<(&str, PathBuf)> = Vec::new();
    for path in [
        "storage.files.path",
        "storage.files.secrets_path",
        "storage.files.runs_path",
        "runner.work_dir",
        "runner.secrets_dir",
    ] {
        if let Some(t) = v.text(path) {
            folders.push((path, PathBuf::from(t)));
        }
    }
    for (i, (pa, a)) in folders.iter().enumerate() {
        for (pb, b) in folders.iter().skip(i + 1) {
            if nested(a, b) {
                errors.push(ConfigError::new(
                    *pb,
                    format!("the folder is inside `{pa}`, or `{pa}` is inside it"),
                    line(pb),
                ));
            }
        }
    }
    // C7: plain_http and the scheme of the PDP URL agree.
    if let (Some(url), Some(plain)) = (v.text("access.pdp.url"), v.bool("access.pdp.plain_http"))
        && let Some((parsed, _)) = parse_http_url(url, true)
        && parsed.https == plain
    {
        errors.push(ConfigError::new(
            "access.pdp.plain_http",
            if plain {
                "the value true needs an `http` URL in `access.pdp.url`"
            } else {
                "the value false needs an `https` URL in `access.pdp.url`"
            },
            line("access.pdp.plain_http"),
        ));
    }
    // C8: telemetry.ca_file and the scheme of the endpoint agree.
    if v.bool("telemetry.enabled") == Some(true)
        && let Some((parsed, _)) = v.text("telemetry.otlp_endpoint").and_then(|u| parse_http_url(u, false))
    {
        match (parsed.https, v.key("telemetry.ca_file").is_some()) {
            (true, false) => errors.push(ConfigError::new(
                "telemetry.ca_file",
                "missing attribute; an `https` endpoint needs a CA file",
                line("telemetry.otlp_endpoint"),
            )),
            (false, true) => errors.push(ConfigError::new(
                "telemetry.ca_file",
                "not permitted with an `http` endpoint",
                line("telemetry.ca_file"),
            )),
            _ => {}
        }
    }
    // C11: the budget of the options route can end one item (3.16).
    if v.text("access.mode") == Some("authzen") {
        let wanted = if enabled { 8 } else { 4 };
        let budget = v.int("access.options_max_questions").unwrap_or(200);
        if budget < wanted {
            errors.push(ConfigError::new(
                "access.options_max_questions",
                format!(
                    "expected at least {wanted} with `runner.enabled = {enabled}`, found {budget}: one item needs that many questions"
                ),
                line("access.options_max_questions"),
            ));
        }
    }
    // C10: the schemas agree (2.11).
    let set: Vec<(&SchemaId, &UnitSchema)> = stage.capabilities.schemas.values().map(|s| (&s.id, &s.schema)).collect();
    for problem in check_schema_set(set) {
        errors.push(ConfigError::new("schema", problem, None));
    }
    errors
}

/// The warnings of 6.15.
pub(crate) fn warnings(v: &View<'_>) -> Vec<super::Warning> {
    let mut out = Vec::new();
    if v.text("access.mode") == Some("open") {
        out.push(super::Warning {
            event: "access.mode_open",
            message: "the access mode is open: Pupitre asks no question, and the proxy is the only guard".to_string(),
        });
    }
    if v.bool("access.pdp.plain_http") == Some(true) {
        out.push(super::Warning {
            event: "access.pdp_plain_http",
            message: "the link to the PDP has no TLS".to_string(),
        });
    }
    if v.bool("runner.allow_apply_in_open_mode") == Some(true) {
        out.push(super::Warning {
            event: "runner.apply_in_open_mode",
            message: "the mode open accepts the applies and the commands".to_string(),
        });
    }
    if v.text("telemetry.otlp_endpoint").and_then(|u| parse_http_url(u, false)).is_some_and(|(p, _)| !p.https) {
        out.push(super::Warning { event: "telemetry.plain_http", message: "the OTLP endpoint has no TLS".to_string() });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_and_base_paths() {
        assert!(canonical_path("/var/lib/pupitre"));
        assert!(canonical_path("/"));
        for bad in ["/a/", "/a//b", "/a/./b", "/a/../b", "a/b"] {
            assert!(!canonical_path(bad), "{bad}");
        }
        assert!(base_path_ok("/"));
        assert!(base_path_ok("/console/"));
        assert!(!base_path_ok("/console"));
        assert!(!base_path_ok("/../"));
    }

    #[test]
    fn nested_folders() {
        assert!(nested(Path::new("/a"), Path::new("/a/b")));
        assert!(!nested(Path::new("/a"), Path::new("/ab")));
    }
}
