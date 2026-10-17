//! gray-stamp — a per-session timing ledger.
//!
//! Port of pi's `stamp` extension: `pre_tool`/`post_tool`/`turn_end` events
//! append `{kind, name?, at, ms}` lines to a per-session ledger at
//! `~/.gray/stamps/<sha1(session)>.jsonl`, and `/stamps` renders the table.
//! `prompt/context` adds a one-line transcript annotation ("last turn 42.1s
//! · 3 tool calls") so the timing signal rides along in context.
//!
//! Tool durations are measured pre→post with `Instant`; turn duration is
//! first-event-of-turn → `turn_end`. Wall-clock `at` is unix seconds, shown
//! as UTC HH:MM:SS.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

fn manifest_stamp() -> Value {
    json!({
        "name": "stamp",
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": "1.1",
        "tools": [],
        "commands": ["/stamps"],
        "hooks": ["prompt/context"],
    })
}

fn state_dir() -> PathBuf {
    std::env::var_os("GRAY_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".gray")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("stamps")
}

/// Session key: `session.id`, else `session.cwd`, else "default".
fn session_key(params: &Value) -> String {
    let s = params.get("session").cloned().unwrap_or(Value::Null);
    s.get("id")
        .and_then(Value::as_str)
        .filter(|x| !x.is_empty())
        .or_else(|| s.get("cwd").and_then(Value::as_str))
        .unwrap_or("default")
        .to_string()
}

fn ledger_path(key: &str) -> PathBuf {
    state_dir().join(format!("{}.jsonl", key_file(key)))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// In-flight tool calls and the open turn for one session.
#[derive(Default)]
pub struct Sess {
    pending: Vec<(String, Instant)>,
    turn_start: Option<Instant>,
    tools_in_turn: usize,
}

pub type State = Mutex<HashMap<String, Sess>>;

/// Append one ledger line; errors go to stderr, never the wire.
fn ledger_append(key: &str, entry: &Value) {
    let path = ledger_path(key);
    if let Some(dir) = path.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        eprintln!("gray-stamp: can't create {}: {e}", dir.display());
        return;
    }
    let line = format!("{entry}\n");
    if let Err(e) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(line.as_bytes()))
    {
        eprintln!("gray-stamp: can't append {}: {e}", path.display());
    }
}

fn ledger_read(path: &Path) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect()
}

/// `event/notify` — pre_tool/post_tool/turn_end keep the ledger. Pure
/// observation: nothing here answers or blocks the host.
fn on_notify(state: &State, params: &Value) {
    let ty = params.get("type").and_then(Value::as_str).unwrap_or("");
    let key = session_key(params);
    let mut guard = state.lock().expect("state");
    let sess = guard.entry(key.clone()).or_default();
    match ty {
        "pre_tool" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string();
            sess.turn_start.get_or_insert_with(Instant::now);
            sess.pending.push((name, Instant::now()));
        }
        "post_tool" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let hit = sess
                .pending
                .iter()
                .rposition(|(n, _)| n == name)
                .or_else(|| sess.pending.len().checked_sub(1));
            if let Some(i) = hit {
                let (name, t0) = sess.pending.remove(i);
                sess.tools_in_turn += 1;
                let entry = json!({
                    "kind": "tool", "name": name,
                    "at": now_unix(), "ms": t0.elapsed().as_millis() as u64,
                });
                drop(guard);
                ledger_append(&key, &entry);
            }
        }
        "turn_start" => {
            sess.turn_start = Some(Instant::now());
        }
        "turn_end" => {
            let ms = sess
                .turn_start
                .map(|t| t.elapsed().as_millis() as u64)
                .unwrap_or(0);
            let entry = json!({
                "kind": "turn", "at": now_unix(), "ms": ms,
                "tools": sess.tools_in_turn,
            });
            *sess = Sess::default();
            drop(guard);
            ledger_append(&key, &entry);
        }
        _ => {}
    }
}

fn fmt_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{}m{:02}s", ms / 60_000, (ms % 60_000) / 1000)
    }
}

/// Unix seconds → `HH:MM:SS` (UTC; the ledger is a timing table, not a
/// calendar).
fn fmt_clock(unix: u64) -> String {
    let t = unix % 86_400;
    format!("{:02}:{:02}:{:02}", t / 3600, (t % 3600) / 60, t % 60)
}

fn render_table(entries: &[Value], path: &Path) -> String {
    let mut out = format!("stamp ledger — {}\n", path.display());
    if entries.is_empty() {
        out.push_str("(empty — no turns or tool calls stamped yet)");
        return out;
    }
    for e in entries {
        let at = fmt_clock(e.get("at").and_then(Value::as_u64).unwrap_or(0));
        let ms = fmt_ms(e.get("ms").and_then(Value::as_u64).unwrap_or(0));
        match e.get("kind").and_then(Value::as_str) {
            Some("tool") => {
                let name = e.get("name").and_then(Value::as_str).unwrap_or("?");
                out.push_str(&format!("{at}  tool {name:<12} {ms:>8}\n"));
            }
            Some("turn") => {
                let tools = e.get("tools").and_then(Value::as_u64).unwrap_or(0);
                out.push_str(&format!(
                    "{at}  turn {:<12} {ms:>8} · {tools} tool{}\n",
                    "", if tools == 1 { "" } else { "s" }
                ));
            }
            _ => {}
        }
    }
    out.push_str(&format!("{} entries", entries.len()));
    out
}

/// `prompt/context` — one-line transcript annotation riding every turn.
fn annotate(params: &Value) -> Value {
    let key = session_key(params);
    let entries = ledger_read(&ledger_path(&key));
    let Some(turn) = entries
        .iter()
        .rev()
        .find(|e| e.get("kind").and_then(Value::as_str) == Some("turn"))
    else {
        return json!({});
    };
    let ms = fmt_ms(turn.get("ms").and_then(Value::as_u64).unwrap_or(0));
    let tools = turn.get("tools").and_then(Value::as_u64).unwrap_or(0);
    json!({
        "text": format!(
            "[stamp] last turn {ms} · {tools} tool calls · ledger {}",
            ledger_path(&key).display()
        )
    })
}

fn run_command(argv: &[&str], params: &Value, state: &State) -> String {
    let key = session_key(params);
    let path = ledger_path(&key);
    match argv.first().copied() {
        None => render_table(&ledger_read(&path), &path),
        Some("clear") => {
            let n = ledger_read(&path).len();
            let _ = std::fs::remove_file(&path);
            state.lock().expect("state").remove(&key);
            format!("cleared {n} ledger entries")
        }
        Some("path") => path.display().to_string(),
        _ => "usage: /stamps [clear|path]".into(),
    }
}

/// One request → `Some(reply)`, or `None` for notifications. The bool asks
/// the loop to exit after writing the reply.
pub fn handle_stamp(req: &Value, state: &State) -> (Option<Value>, bool) {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    let Some(id) = id else {
        if method == "event/notify" {
            on_notify(state, &params);
        }
        return (None, method == "plugin/shutdown");
    };
    let result = match method {
        "plugin/manifest" => manifest_stamp(),
        "prompt/context" => annotate(&params),
        "command/run" => {
            let argv: Vec<&str> = params
                .get("argv")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            json!({ "text": run_command(&argv, &params, state) })
        }
        "plugin/shutdown" => return (Some(json!({ "id": id, "result": {} })), true),
        _ => {
            let error = json!({ "code": -32601, "message": "method not found" });
            return (Some(json!({ "id": id, "error": error })), false);
        }
    };
    (Some(json!({ "id": id, "result": result })), false)
}

/// Sidecar entry: `gray-stamp manifest` prints the manifest, otherwise serve
/// JSON-lines requests on stdin until EOF or `plugin/shutdown`.
pub fn run_stamp() -> std::io::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("manifest") {
        println!("{}", manifest_stamp());
        return Ok(());
    }
    let state: State = Mutex::new(HashMap::new());
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let (reply, exit) = handle_stamp(&req, &state);
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

/// Filename-safe session key: sanitized prefix plus a `DefaultHasher`
/// suffix so different keys can't collide. The hash is per-build-stable,
/// which is all a state filename needs.
fn key_file(key: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    let clean: String = key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .take(48)
        .collect();
    format!("{}-{:016x}", clean.trim_matches('.'), h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as TestMutex;

    static LOCK: TestMutex<()> = TestMutex::new(());
    static CTR: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    /// Run `body` with GRAY_HOME pointed at a fresh temp dir. Serialized:
    /// the env var is process-global and tests run in one process.
    fn with_home(body: impl FnOnce(&Path)) {
        let _g = LOCK.lock().unwrap();
        let n = CTR.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let home = std::env::temp_dir().join(format!("gray-stamp-test-{n}"));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        unsafe { std::env::set_var("GRAY_HOME", &home) };
        body(&home);
        let _ = std::fs::remove_dir_all(&home);
    }

    fn notify(state: &State, ty: &str, extra: Value) {
        let mut params = json!({"type": ty, "session": {"id": "s1", "cwd": "/tmp"}});
        params.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let (reply, exit) = handle_stamp(&json!({"method": "event/notify", "params": params}), state);
        assert!(reply.is_none() && !exit);
    }

    fn call(state: &State, method: &str, params: Value) -> Value {
        handle_stamp(&json!({ "id": 1, "method": method, "params": params }), state)
            .0
            .unwrap()
    }

    #[test]
    fn manifest_shape() {
        let state: State = Mutex::new(HashMap::new());
        let m = call(&state, "plugin/manifest", Value::Null)["result"].clone();
        assert_eq!(m["name"], "stamp");
        assert_eq!(m["commands"], json!(["/stamps"]));
        assert_eq!(m["hooks"], json!(["prompt/context"]));
    }

    #[test]
    fn tool_and_turn_events_write_ledger() {
        with_home(|_| {
            let state: State = Mutex::new(HashMap::new());
            notify(&state, "pre_tool", json!({"name": "bash"}));
            notify(&state, "post_tool", json!({"name": "bash"}));
            notify(&state, "pre_tool", json!({"name": "read"}));
            notify(&state, "post_tool", json!({"name": "read"}));
            notify(&state, "turn_end", json!({}));

            let entries = ledger_read(&ledger_path("s1"));
            assert_eq!(entries.len(), 3);
            assert_eq!(entries[0]["kind"], "tool");
            assert_eq!(entries[0]["name"], "bash");
            assert_eq!(entries[2]["kind"], "turn");
            assert_eq!(entries[2]["tools"], 2);

            let r = call(
                &state,
                "command/run",
                json!({"name": "/stamps", "argv": [], "session": {"id": "s1"}}),
            );
            let text = r["result"]["text"].as_str().unwrap();
            assert!(text.contains("tool bash") && text.contains("turn"), "{text}");
            assert!(text.contains("3 entries"));

            let r = call(&state, "prompt/context", json!({"session": {"id": "s1"}}));
            assert!(r["result"]["text"].as_str().unwrap().contains("last turn"));
        });
    }

    #[test]
    fn annotation_empty_without_turns() {
        with_home(|_| {
            let state: State = Mutex::new(HashMap::new());
            let r = call(&state, "prompt/context", json!({"session": {"id": "s1"}}));
            assert_eq!(r["result"], json!({}));
        });
    }

    #[test]
    fn stamps_clear_resets() {
        with_home(|_| {
            let state: State = Mutex::new(HashMap::new());
            notify(&state, "turn_end", json!({}));
            let r = call(
                &state,
                "command/run",
                json!({"name": "/stamps", "argv": ["clear"], "session": {"id": "s1"}}),
            );
            assert!(r["result"]["text"].as_str().unwrap().contains("cleared 1"));
            let r = call(
                &state,
                "command/run",
                json!({"name": "/stamps", "argv": [], "session": {"id": "s1"}}),
            );
            assert!(r["result"]["text"].as_str().unwrap().contains("empty"));
        });
    }

    #[test]
    fn shutdown_replies_then_exits() {
        let state: State = Mutex::new(HashMap::new());
        let (reply, exit) =
            handle_stamp(&json!({ "id": 2, "method": "plugin/shutdown" }), &state);
        assert!(reply.is_some() && exit);
    }
}
