//! gray-observe — local turn telemetry, no external service.
//!
//! Every host event lands as one JSON line in
//! `~/.gray/observe/<YYYY-MM-DD>.jsonl`:
//!
//! - `pre_tool`   → `{type, ts, session, tool, args_len}`
//! - `post_tool`  → `{type, ts, session, tool, is_error, content_len}`
//! - `turn_end`   → `{type, ts, session, usage}`
//!
//! `/observe` summarizes today's file (turns, tool calls, top tools, error
//! rate); `/observe tail <n>` prints the last n raw lines. Files rotate to
//! `<date>.jsonl.1` when they would pass 10 MB.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// Files rotate to `.1` once they would pass this size.
const CAP_BYTES: u64 = 10 * 1024 * 1024;

fn manifest_observe() -> Value {
    json!({
        "name": "observe",
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": "1.1",
        "tools": [],
        "commands": ["/observe"],
    })
}

pub fn observe_dir() -> PathBuf {
    let home = std::env::var_os("GRAY_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".gray")))
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("observe")
}

/// Local calendar date (`YYYY-MM-DD`) via `date`; falls back to UTC civil
/// math when `date` can't run.
fn today() -> String {
    if let Ok(o) = std::process::Command::new("date").arg("+%F").output()
        && o.status.success()
    {
        let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if !s.is_empty() {
            return s;
        }
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    utc_date(secs)
}

/// Howard Hinnant's civil_from_days: unix days → (year, month, day) UTC.
fn utc_date(secs: u64) -> String {
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (m, y) = if mp < 10 { (mp + 3, y) } else { (mp - 9, y + 1) };
    let d = doy - (153 * mp + 2) / 5 + 1;
    format!("{y:04}-{m:02}-{d:02}")
}

fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn session_id(params: &Value) -> &str {
    params
        .get("session")
        .and_then(|s| s.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// Build the JSONL record for one `event/notify`, or `None` for types we
/// don't track.
fn event_line(params: &Value) -> Option<String> {
    let ty = params.get("type").and_then(Value::as_str)?;
    let ts = now_ts();
    let session = session_id(params);
    let rec = match ty {
        "pre_tool" => json!({
            "type": "pre_tool", "ts": ts, "session": session,
            "tool": params.get("name").and_then(Value::as_str).unwrap_or(""),
            "args_len": params.get("args").map(|a| a.to_string().len()).unwrap_or(0),
        }),
        "post_tool" => json!({
            "type": "post_tool", "ts": ts, "session": session,
            "tool": params.get("name").and_then(Value::as_str).unwrap_or(""),
            "is_error": params.get("is_error").and_then(Value::as_bool).unwrap_or(false),
            "content_len": params.get("content").and_then(Value::as_str)
                .map(|c| c.len()).unwrap_or(0),
        }),
        "turn_end" => json!({
            "type": "turn_end", "ts": ts, "session": session,
            "usage": params.get("usage").cloned().unwrap_or(json!({})),
        }),
        _ => return None,
    };
    Some(rec.to_string())
}

/// Append one line to `<dir>/<date>.jsonl`, rotating to `.1` first when the
/// file would pass the cap.
fn append_line(dir: &Path, date: &str, line: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{date}.jsonl"));
    let cur = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if cur + line.len() as u64 + 1 > CAP_BYTES {
        let _ = std::fs::rename(&path, dir.join(format!("{date}.jsonl.1")));
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(line.as_bytes())?;
    f.write_all(b"\n")
}

/// Parsed stats from one JSONL file's lines.
struct Stats {
    turns: u64,
    calls: u64,
    errors: u64,
    posts: u64,
    tools: std::collections::BTreeMap<String, u64>,
}

fn stats(lines: &[String]) -> Stats {
    let mut s = Stats {
        turns: 0,
        calls: 0,
        errors: 0,
        posts: 0,
        tools: Default::default(),
    };
    for line in lines {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match v.get("type").and_then(Value::as_str) {
            Some("pre_tool") => {
                s.calls += 1;
                let t = v.get("tool").and_then(Value::as_str).unwrap_or("?");
                *s.tools.entry(t.to_string()).or_default() += 1;
            }
            Some("post_tool") => {
                s.posts += 1;
                if v.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
                    s.errors += 1;
                }
            }
            Some("turn_end") => s.turns += 1,
            _ => {}
        }
    }
    s
}

fn read_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .map(|c| c.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// `/observe …` → status text. `argv` excludes the command name.
fn run_command(dir: &Path, date: &str, argv: &[&str]) -> String {
    let path = dir.join(format!("{date}.jsonl"));
    match argv.first().copied() {
        Some("tail") => {
            let n: usize = argv.get(1).and_then(|s| s.parse().ok()).unwrap_or(10);
            let lines = read_lines(&path);
            if lines.is_empty() {
                return format!("nothing logged for {date} yet");
            }
            lines.iter().rev().take(n).rev().cloned().collect::<Vec<_>>().join("\n")
        }
        _ => {
            let lines = read_lines(&path);
            if lines.is_empty() {
                return format!("nothing logged for {date} yet — log: {}", path.display());
            }
            let s = stats(&lines);
            let rate = if s.posts > 0 {
                s.errors as f64 / s.posts as f64 * 100.0
            } else {
                0.0
            };
            let mut top: Vec<(String, u64)> = s.tools.into_iter().collect();
            top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let top = top
                .iter()
                .take(5)
                .map(|(t, n)| format!("{t} ×{n}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{date}: {} turn(s) · {} tool call(s) · {rate:.1}% error rate\n\
                 top tools: {}\nlog: {}",
                s.turns,
                s.calls,
                if top.is_empty() { "(none)" } else { &top },
                path.display()
            )
        }
    }
}

/// One request → `Some(reply)`, or `None` for notifications. The bool asks
/// the loop to exit after writing the reply.
pub fn handle_observe(req: &Value, dir: &Path) -> (Option<Value>, bool) {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    let Some(id) = id else {
        if method == "event/notify"
            && let Some(line) = event_line(&params)
        {
            let _ = append_line(dir, &today(), &line);
        }
        return (None, method == "plugin/shutdown");
    };
    if method.is_empty() {
        return (None, false);
    }
    let result = match method {
        "plugin/manifest" => manifest_observe(),
        "command/run" => {
            let argv: Vec<&str> = params
                .get("argv")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            json!({ "text": run_command(dir, &today(), &argv) })
        }
        "plugin/shutdown" => return (Some(json!({ "id": id, "result": {} })), true),
        _ => {
            let error = json!({ "code": -32601, "message": "method not found" });
            return (Some(json!({ "id": id, "error": error })), false);
        }
    };
    (Some(json!({ "id": id, "result": result })), false)
}

fn main_observe() -> std::io::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("manifest") {
        println!("{}", manifest_observe());
        return Ok(());
    }
    let dir = observe_dir();
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let (reply, exit) = handle_observe(&req, &dir);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn notify(dir: &Path, params: Value) {
        handle_observe(&json!({ "method": "event/notify", "params": params }), dir);
    }

    fn cmd(dir: &Path, date: &str, argv: &[&str]) -> String {
        run_command(dir, date, argv)
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gray-obs-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn manifest_has_observe_command() {
        let m = manifest_observe();
        assert_eq!(m["commands"], json!(["/observe"]));
        assert_eq!(m["tools"], json!([]));
    }

    #[test]
    fn event_line_shapes() {
        let pre = event_line(&json!({"type":"pre_tool","name":"bash",
            "args":{"command":"ls"},"session":{"id":"s1"}})).unwrap();
        let v: Value = serde_json::from_str(&pre).unwrap();
        assert_eq!(v["tool"], "bash");
        assert!(v["args_len"].as_u64().unwrap() > 0);
        let post = event_line(&json!({"type":"post_tool","name":"read",
            "content":"abc","is_error":true,"session":{}})).unwrap();
        let v: Value = serde_json::from_str(&post).unwrap();
        assert_eq!(v["is_error"], true);
        assert_eq!(v["content_len"], 3);
        let end = event_line(&json!({"type":"turn_end","usage":{"total":42},"session":{}})).unwrap();
        let v: Value = serde_json::from_str(&end).unwrap();
        assert_eq!(v["usage"]["total"], 42);
        assert!(event_line(&json!({"type":"other"})).is_none());
    }

    #[test]
    fn appends_and_reports() {
        let dir = tmpdir("app");
        let date = "1999-01-01";
        let s = json!({"id":"s1"});
        notify(&dir, json!({"type":"pre_tool","name":"bash","args":{},"session":s}));
        notify(&dir, json!({"type":"post_tool","name":"bash","content":"ok","is_error":false,"session":s}));
        notify(&dir, json!({"type":"turn_end","usage":{"total":10},"session":s}));
        // events land under today(); the stats test below uses a fixed date.
        let file = dir.join(format!("{}.jsonl", today()));
        assert_eq!(read_lines(&file).len(), 3);

        append_line(&dir, date, r#"{"type":"pre_tool","tool":"bash","ts":1,"session":""}"#).unwrap();
        append_line(&dir, date, r#"{"type":"post_tool","tool":"bash","is_error":true,"content_len":2,"ts":1,"session":""}"#).unwrap();
        append_line(&dir, date, r#"{"type":"turn_end","usage":{},"ts":1,"session":""}"#).unwrap();
        let out = cmd(&dir, date, &[]);
        assert!(out.contains("1 turn(s)"));
        assert!(out.contains("1 tool call(s)"));
        assert!(out.contains("100.0% error rate"));
        assert!(out.contains("bash ×1"));
    }

    #[test]
    fn tail_returns_last_n() {
        let dir = tmpdir("tail");
        let date = "1999-01-01";
        for i in 0..5 {
            append_line(&dir, date, &format!("{{\"n\":{i}}}")).unwrap();
        }
        let out = cmd(&dir, date, &["tail", "2"]);
        assert_eq!(out, "{\"n\":3}\n{\"n\":4}");
    }

    #[test]
    fn rotates_at_cap() {
        let dir = tmpdir("rot");
        let date = "1999-01-01";
        let big = "x".repeat(CAP_BYTES as usize);
        append_line(&dir, date, &big).unwrap();
        append_line(&dir, date, "{}").unwrap();
        assert!(dir.join(format!("{date}.jsonl.1")).exists());
        assert_eq!(read_lines(&dir.join(format!("{date}.jsonl"))).len(), 1);
    }

    #[test]
    fn utc_date_fallback() {
        assert_eq!(utc_date(0), "1970-01-01");
        assert_eq!(utc_date(1_760_000_000), "2025-10-09");
    }
}
