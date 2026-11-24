// SPDX-License-Identifier: MIT OR Apache-2.0
//! The lines of the callback plugin (4.20) and their stored form (4.20, 4.22).
//!
//! The plugin is in the file `callback/pupitre_events.py`. The binary holds its bytes in
//! [`PLUGIN`]. The runner writes them into each snapshot.

use std::collections::BTreeMap;

use pupitre_core::ids::HostId;
use pupitre_core::json::{self, Json, Members, ReadLimits};
use pupitre_core::run::HostStats;
use pupitre_core::time::UtcMillis;

use crate::child::Phase;

/// The bytes of the callback plugin `pupitre_events` (4.20).
pub const PLUGIN: &[u8] = include_bytes!("callback/pupitre_events.py");

/// The version of the line format: the member `v` (4.20).
pub const EVENT_VERSION: u64 = 1;

/// The longest text of `diff` or `message` (4.20).
pub const MAX_TEXT_BYTES: usize = 16_384;

/// The status of a host result (4.20).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostStatus {
    /// `ok`
    Ok,
    /// `changed`
    Changed,
    /// `failed`
    Failed,
    /// `skipped`
    Skipped,
    /// `unreachable`
    Unreachable,
}

impl HostStatus {
    /// The name of the status in a line.
    pub fn name(self) -> &'static str {
        match self {
            HostStatus::Ok => "ok",
            HostStatus::Changed => "changed",
            HostStatus::Failed => "failed",
            HostStatus::Skipped => "skipped",
            HostStatus::Unreachable => "unreachable",
        }
    }

    fn from_name(name: &str) -> Option<HostStatus> {
        [HostStatus::Ok, HostStatus::Changed, HostStatus::Failed, HostStatus::Skipped, HostStatus::Unreachable]
            .into_iter()
            .find(|s| s.name() == name)
    }
}

/// One line of the plugin, after the strict read (4.20).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackEvent {
    /// `playbook_start`
    PlaybookStart {
        /// The file name of the playbook.
        playbook: String,
        /// True in the mode `check`.
        check: bool,
    },
    /// `play_start`
    PlayStart {
        /// The name of the play.
        play: String,
    },
    /// `task_start`
    TaskStart {
        /// The name of the task.
        task: String,
    },
    /// `host_result`
    HostResult {
        /// The inventory name of the host.
        host: String,
        /// The name of the task.
        task: String,
        /// The status.
        status: HostStatus,
        /// The difference of the task.
        diff: Option<String>,
        /// The message of a failed or unreachable result.
        message: Option<String>,
        /// True when the plugin left out a text above 16,384 bytes.
        omitted: bool,
    },
    /// `stats`: the counts as the line gives them.
    Stats {
        /// The object `hosts` of the line.
        hosts: Json,
    },
}

impl CallbackEvent {
    /// The members of the line, with `kind` and `v` (4.20). The runner adds `seq`, `phase` and
    /// `t` (4.20).
    pub fn to_json(&self) -> Json {
        let mut m: BTreeMap<String, Json> = BTreeMap::new();
        let mut put = |k: &str, v: Json| {
            m.insert(k.to_string(), v);
        };
        put("v", Json::Int(EVENT_VERSION));
        match self {
            CallbackEvent::PlaybookStart { playbook, check } => {
                put("kind", Json::str("playbook_start"));
                put("playbook", Json::str(playbook.clone()));
                put("check", Json::Bool(*check));
            }
            CallbackEvent::PlayStart { play } => {
                put("kind", Json::str("play_start"));
                put("play", Json::str(play.clone()));
            }
            CallbackEvent::TaskStart { task } => {
                put("kind", Json::str("task_start"));
                put("task", Json::str(task.clone()));
            }
            CallbackEvent::HostResult { host, task, status, diff, message, omitted } => {
                put("kind", Json::str("host_result"));
                put("host", Json::str(host.clone()));
                put("task", Json::str(task.clone()));
                put("status", Json::str(status.name()));
                if let Some(d) = diff {
                    put("diff", Json::str(d.clone()));
                }
                if let Some(t) = message {
                    put("message", Json::str(t.clone()));
                }
                if *omitted {
                    put("omitted", Json::Bool(true));
                }
            }
            CallbackEvent::Stats { hosts } => {
                put("kind", Json::str("stats"));
                put("hosts", hosts.clone());
            }
        }
        Json::Obj(m)
    }

    /// The counts of a `stats` event, by host. `None` for another event.
    pub fn host_stats(&self) -> Option<BTreeMap<HostId, HostStats>> {
        let CallbackEvent::Stats { hosts } = self else { return None };
        stats_of(hosts)
    }
}

fn stats_of(hosts: &Json) -> Option<BTreeMap<HostId, HostStats>> {
    hosts
        .as_object()?
        .iter()
        .map(|(h, v)| Some((HostId::parse(h).ok()?, HostStats::from_json(v)?)))
        .collect::<Option<BTreeMap<_, _>>>()
}

fn text(m: &mut Members, name: &str) -> Option<String> {
    match m.take(name)? {
        Json::Str(t) => Some(t),
        _ => None,
    }
}

fn optional_text(m: &mut Members, name: &str) -> Result<Option<String>, ()> {
    match m.take(name) {
        None => Ok(None),
        Some(Json::Str(t)) if t.len() <= MAX_TEXT_BYTES => Ok(Some(t)),
        Some(_) => Err(()),
    }
}

/// Read one line of the plugin with the strict reader of 2.5 and the closed members of 4.20.
/// `None` means a line that is not valid: the runner then stores the event `invalid`.
pub fn read_line(line: &[u8], max_bytes: usize) -> Option<CallbackEvent> {
    let value = json::parse(line, ReadLimits::bytes(max_bytes)).ok()?;
    let mut m = Members::of(&value).ok()?;
    if m.take("v") != Some(Json::Int(EVENT_VERSION)) {
        return None;
    }
    let kind = text(&mut m, "kind")?;
    let event = match kind.as_str() {
        "playbook_start" => {
            let playbook = text(&mut m, "playbook")?;
            let Some(Json::Bool(check)) = m.take("check") else { return None };
            CallbackEvent::PlaybookStart { playbook, check }
        }
        "play_start" => CallbackEvent::PlayStart { play: text(&mut m, "play")? },
        "task_start" => CallbackEvent::TaskStart { task: text(&mut m, "task")? },
        "host_result" => {
            let host = text(&mut m, "host")?;
            let task = text(&mut m, "task")?;
            let status = HostStatus::from_name(&text(&mut m, "status")?)?;
            let diff = optional_text(&mut m, "diff").ok()?;
            let message = optional_text(&mut m, "message").ok()?;
            let omitted = match m.take("omitted") {
                None => false,
                Some(Json::Bool(true)) => true,
                Some(_) => return None,
            };
            CallbackEvent::HostResult { host, task, status, diff, message, omitted }
        }
        "stats" => {
            let hosts = m.take("hosts")?;
            stats_of(&hosts)?;
            CallbackEvent::Stats { hosts }
        }
        _ => return None,
    };
    if !m.finish().is_empty() {
        return None;
    }
    Some(event)
}

/// The stored form of a line (4.20, 4.22): the compact bytes of the members of the line, with
/// `seq`, `phase` and `t`. `event` is `None` for a line that is not valid, and `length` is
/// then the byte length of that line.
pub fn stored_line(event: Option<&CallbackEvent>, length: usize, seq: u64, phase: Phase, t: UtcMillis) -> Vec<u8> {
    let mut value = match event {
        Some(e) => e.to_json(),
        None => Json::object([("kind", Json::str("invalid")), ("length", Json::Int(length as u64))]),
    };
    if let Json::Obj(map) = &mut value {
        map.insert("seq".to_string(), Json::Int(seq));
        map.insert("phase".to_string(), Json::str(phase.name()));
        map.insert("t".to_string(), Json::str(t.to_string()));
    }
    json::to_compact(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Option<CallbackEvent> {
        read_line(text.as_bytes(), 65_536)
    }

    #[test]
    fn reads_each_kind_of_4_20() {
        assert_eq!(
            read(r#"{"check":true,"kind":"playbook_start","playbook":"deploy.yml","v":1}"#),
            Some(CallbackEvent::PlaybookStart { playbook: "deploy.yml".into(), check: true })
        );
        assert_eq!(
            read(r#"{"kind":"play_start","play":"Deploy","v":1}"#),
            Some(CallbackEvent::PlayStart { play: "Deploy".into() })
        );
        assert_eq!(
            read(r#"{"kind":"task_start","task":"Copy","v":1}"#),
            Some(CallbackEvent::TaskStart { task: "Copy".into() })
        );
        let e = read(
            r#"{"diff":"--- a\n+++ b\n","host":"host-a","kind":"host_result","status":"changed","task":"Copy","v":1}"#,
        );
        assert_eq!(
            e,
            Some(CallbackEvent::HostResult {
                host: "host-a".into(),
                task: "Copy".into(),
                status: HostStatus::Changed,
                diff: Some("--- a\n+++ b\n".into()),
                message: None,
                omitted: false,
            })
        );
        let stats = r#"{"hosts":{"host-a":{"changed":1,"failed":0,"ignored":0,"ok":3,"rescued":0,"skipped":0,"unreachable":0}},"kind":"stats","v":1}"#;
        let e = read(stats).unwrap();
        let counts = e.host_stats().unwrap();
        assert_eq!(counts.len(), 1);
        assert_eq!(counts.values().next().unwrap().ok, 3);
    }

    #[test]
    fn refuses_what_4_20_does_not_give() {
        for bad in [
            "",
            "not json",
            r#"{"kind":"play_start","play":"x"}"#,
            r#"{"kind":"play_start","play":"x","v":2}"#,
            r#"{"kind":"play_start","play":"x","v":1,"extra":1}"#,
            r#"{"kind":"other","v":1}"#,
            r#"{"kind":"task_start","task":1,"v":1}"#,
            r#"{"kind":"playbook_start","playbook":"a.yml","check":"yes","v":1}"#,
            r#"{"host":"h","kind":"host_result","status":"weird","task":"t","v":1}"#,
            r#"{"host":"h","kind":"host_result","omitted":false,"status":"ok","task":"t","v":1}"#,
            r#"{"hosts":{"host-a":{"ok":1}},"kind":"stats","v":1}"#,
            r#"{"hosts":{"Bad Host":{"changed":1,"failed":0,"ignored":0,"ok":3,"rescued":0,"skipped":0,"unreachable":0}},"kind":"stats","v":1}"#,
            r#"{"kind":"play_start","kind":"play_start","play":"x","v":1}"#,
            r#"{"kind":"play_start","play":null,"v":1}"#,
            r#"{"kind":"play_start","play":"x","v":1} trailing"#,
        ] {
            assert_eq!(read(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_text_above_16384_bytes_is_not_valid() {
        let long = "a".repeat(MAX_TEXT_BYTES + 1);
        let line =
            format!(r#"{{"host":"h","kind":"host_result","message":"{long}","status":"failed","task":"t","v":1}}"#);
        assert_eq!(read(&line), None);
        let ok = "a".repeat(MAX_TEXT_BYTES);
        let line =
            format!(r#"{{"host":"h","kind":"host_result","message":"{ok}","status":"failed","task":"t","v":1}}"#);
        assert!(read(&line).is_some());
    }

    #[test]
    fn a_line_above_the_bound_is_not_valid() {
        let line = br#"{"kind":"play_start","play":"x","v":1}"#;
        assert!(read_line(line, line.len()).is_some());
        assert_eq!(read_line(line, line.len() - 1), None);
    }

    #[test]
    fn the_stored_form_adds_seq_phase_and_time() {
        let e = CallbackEvent::TaskStart { task: "Copy".into() };
        let bytes = stored_line(Some(&e), 0, 7, Phase::Render, UtcMillis(0));
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            r#"{"kind":"task_start","phase":"render","seq":7,"t":"1970-01-01T00:00:00.000Z","task":"Copy","v":1}"#
        );
        let bytes = stored_line(None, 12, 8, Phase::Main, UtcMillis(1));
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            r#"{"kind":"invalid","length":12,"phase":"main","seq":8,"t":"1970-01-01T00:00:00.001Z"}"#
        );
    }

    #[test]
    fn the_plugin_is_in_the_binary() {
        let text = std::str::from_utf8(PLUGIN).unwrap();
        assert!(text.contains("CALLBACK_NAME = \"pupitre_events\""));
        assert!(text.contains("PUPITRE_EVENTS_FILE"));
    }
}
