//! gray-telemetry — one sidecar for every local event ledger.
//!
//! Merges gray-observe, gray-stamp and gray-usage. Each source lives
//! verbatim in `mods/` with its own state files and slash commands — only
//! the wire plumbing is shared.
//!
//! Fan-out:
//! - `event/notify`   → observe (JSONL log) + stamp (timing ledger) + usage
//!   (live turn_end counters); notifications, never answered.
//! - `prompt/context` → stamp (last-turn timing annotation).
//! - `command/run`    → routed by `params.name` to the owning module.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::{Value, json};

#[allow(dead_code)]
#[path = "mods/observe.rs"]
mod observe;
#[allow(dead_code)]
#[path = "mods/stamp.rs"]
mod stamp;
#[allow(dead_code)]
#[path = "mods/usage.rs"]
mod usage;

fn manifest() -> Value {
    json!({
        "name": "telemetry",
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": "1.1",
        "tools": [],
        "commands": ["/telemetry", "/observe", "/stamps", "/usage", "/tokens"],
        "completion": ["all", "reset"],
        "hooks": ["prompt/context"],
    })
}

/// Per-process module state: observe's log dir and stamp's in-flight timers.
struct Telemetry {
    observe_dir: PathBuf,
    stamp: stamp::State,
}

impl Telemetry {
    fn new() -> Self {
        Self {
            observe_dir: observe::observe_dir(),
            stamp: Mutex::new(HashMap::new()),
        }
    }
}

/// `/telemetry` — merged-plugin overview.
fn telemetry_command() -> String {
    format!(
        "gray-telemetry {} — one sidecar, three local event ledgers:\n\
         \x20 /observe [tail N]     JSONL turn/tool events under ~/.gray/observe\n\
         \x20 /stamps [clear|path]  per-session timing ledger under ~/.gray/stamps\n\
         \x20 /usage, /tokens [all|reset]  token + cost report, ~/.gray/usage",
        env!("CARGO_PKG_VERSION")
    )
}

fn handle(req: &Value, t: &Telemetry) -> (Option<Value>, bool) {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    let Some(id) = id else {
        if method == "event/notify" {
            // All three ledgers observe the same events; none reply.
            let _ = observe::handle_observe(req, &t.observe_dir);
            let _ = stamp::handle_stamp(req, &t.stamp);
            let _ = usage::handle_usage(req);
        }
        return (None, method == "plugin/shutdown");
    };
    let result = match method {
        "plugin/manifest" => manifest(),
        "prompt/context" => {
            let (reply, _) = stamp::handle_stamp(req, &t.stamp);
            return (reply, false);
        }
        "command/run" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let reply = match name {
                "/telemetry" => return (Some(json!({ "id": id, "result": { "text": telemetry_command() } })), false),
                "/observe" => observe::handle_observe(req, &t.observe_dir).0,
                "/stamps" => stamp::handle_stamp(req, &t.stamp).0,
                "/usage" | "/tokens" => usage::handle_usage(req).0,
                _ => None,
            };
            match reply {
                Some(r) => r
                    .get("result")
                    .cloned()
                    .unwrap_or_else(|| json!({ "text": format!("{name}: error") })),
                None => json!({ "text": format!("unknown command {name} — try /telemetry") }),
            }
        }
        "plugin/shutdown" => return (Some(json!({ "id": id, "result": {} })), true),
        _ => {
            let error = json!({ "code": -32601, "message": "method not found" });
            return (Some(json!({ "id": id, "error": error })), false);
        }
    };
    (Some(json!({ "id": id, "result": result })), false)
}

fn main() -> std::io::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("manifest") {
        println!("{}", manifest());
        return Ok(());
    }
    let t = Telemetry::new();
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let (reply, exit) = handle(&req, &t);
        if let Some(reply) = reply {
            writeln!(stdout, "{reply}")?;
            stdout.flush()?;
        }
        if exit {
            break;
        }
    }
    Ok(())
}
