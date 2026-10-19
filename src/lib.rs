//! gray-repguard — a repetition guard for tool calls.
//!
//! Port of hermes' `agent/repetition_guard.py` idea, applied to tool calls:
//! `tool/before` hashes (name, canonicalized-args) and counts consecutive
//! repeats per session. When the same call runs `REPGUARD_MAX_REPEATS`
//! (default 3) times in a row, `tool/after` appends a "you are looping,
//! change approach" nudge to that call's result. A different call — or a
//! gap longer than `REPGUARD_WINDOW_SECS` (default 600) — resets the run,
//! so interleaved iteration never trips the guard.
//!
//! Canonicalization sorts object keys recursively before hashing, so
//! `{"a":1,"b":2}` and `{"b":2,"a":1}` count as the same call.
//!
//! `/repguard` shows config and the current session's repeat counts.

use std::collections::{HashMap, VecDeque, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, Write};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

fn manifest() -> Value {
    json!({
        "name": "repguard",
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": "2.0",
        "tools": [],
        "commands": ["/repguard"],
        "hooks": ["tool/before", "tool/after"],
    })
}

/// Consecutive identical calls that trigger the nudge.
fn max_repeats() -> usize {
    std::env::var("REPGUARD_MAX_REPEATS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|n: &usize| *n >= 2)
        .unwrap_or(3)
}

/// Sliding window for tracked calls; also the run-staleness cutoff.
fn window() -> Duration {
    Duration::from_secs(
        std::env::var("REPGUARD_WINDOW_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(600),
    )
}

/// Recursively sort object keys so key order can't dodge the hash.
fn canon(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let mut out = serde_json::Map::with_capacity(m.len());
            for k in keys {
                out.insert(k.clone(), canon(&m[k]));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(canon).collect()),
        _ => v.clone(),
    }
}

fn call_hash(name: &str, args: &Value) -> u64 {
    let mut h = DefaultHasher::new();
    name.hash(&mut h);
    canon(args).to_string().hash(&mut h);
    h.finish()
}

fn session_key(params: &Value) -> String {
    let s = params.get("session").cloned().unwrap_or(Value::Null);
    s.get("id")
        .and_then(Value::as_str)
        .filter(|x| !x.is_empty())
        .or_else(|| s.get("cwd").and_then(Value::as_str))
        .unwrap_or("")
        .to_string()
}

/// Per-session sliding window of recent call hashes, the consecutive-run
/// counter, and the flag the last `tool/before` raised for `tool/after`.
#[derive(Default)]
struct Sess {
    calls: VecDeque<(u64, Instant)>,
    last: Option<(u64, Instant)>, // hash+time of the previous call
    run: usize,                   // consecutive repeats of that hash
    flag: Option<usize>,          // run length that armed the nudge
}

type State = std::sync::Mutex<HashMap<String, Sess>>;

/// Record this call and return its run length: the run only grows when the
/// immediately preceding call had the same hash within `win`.
fn record(sess: &mut Sess, h: u64, win: Duration) -> usize {
    let now = Instant::now();
    while sess
        .calls
        .front()
        .is_some_and(|(_, t)| now.duration_since(*t) > win)
    {
        sess.calls.pop_front();
    }
    sess.calls.push_back((h, now));
    sess.run = match sess.last {
        Some((lh, lt)) if lh == h && now.duration_since(lt) <= win => sess.run + 1,
        _ => 1,
    };
    sess.last = Some((h, now));
    sess.run
}

fn tool_before(state: &State, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("args").cloned().unwrap_or(Value::Null);
    let h = call_hash(name, &args);
    let key = session_key(params);
    let mut guard = state.lock().expect("state");
    let sess = guard.entry(key).or_default();
    let run = record(sess, h, window());
    sess.flag = (run >= max_repeats()).then_some(run);
    json!({"decision": "allow"})
}

fn tool_after(state: &State, params: &Value) -> Value {
    let key = session_key(params);
    let mut guard = state.lock().expect("state");
    let Some(sess) = guard.get_mut(&key) else {
        return json!({});
    };
    let Some(run) = sess.flag.take() else {
        return json!({});
    };
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let content = params.get("content").and_then(Value::as_str).unwrap_or("");
    let nudge = format!(
        "\n\n[repguard] `{name}` has now run {run} times in a row with identical \
         arguments — you are looping. Change approach: use different \
         arguments, a different tool, or stop and report what is blocking you."
    );
    json!({ "content": format!("{content}{nudge}") })
}

fn run_command(argv: &[&str], params: &Value, state: &State) -> String {
    if argv.first().copied() == Some("status") || argv.is_empty() {
        let key = session_key(params);
        let guard = state.lock().expect("state");
        let (tracked, run) = guard
            .get(&key)
            .map(|s| (s.calls.len(), s.run))
            .unwrap_or((0, 0));
        return format!(
            "gray-repguard {} — nudge at {} identical calls in a row / {}s \
             window (env REPGUARD_MAX_REPEATS, REPGUARD_WINDOW_SECS)\n\
             this session: {} calls tracked, current run {}x",
            env!("CARGO_PKG_VERSION"),
            max_repeats(),
            window().as_secs(),
            tracked,
            run,
        );
    }
    "usage: /repguard [status]".into()
}

/// One request → `Some(reply)`, or `None` for notifications. The bool asks
/// the loop to exit after writing the reply.
fn handle(req: &Value, state: &State) -> (Option<Value>, bool) {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    let Some(id) = id else {
        return (None, method == "plugin/shutdown");
    };
    let result = match method {
        "plugin/manifest" => manifest(),
        "tool/before" => tool_before(state, &params),
        "tool/after" => tool_after(state, &params),
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

/// Sidecar entry: `gray-repguard manifest` prints the manifest, otherwise
/// serve JSON-lines requests on stdin until EOF or `plugin/shutdown`.
pub fn run() -> std::io::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("manifest") {
        println!("{}", manifest());
        return Ok(());
    }
    let state: State = std::sync::Mutex::new(HashMap::new());
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let (reply, exit) = handle(&req, &state);
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

    fn call(state: &State, method: &str, params: Value) -> Value {
        handle(&json!({ "id": 1, "method": method, "params": params }), state)
            .0
            .unwrap()["result"]
            .clone()
    }

    fn sess() -> Value {
        json!({"id": "s1", "cwd": "/tmp"})
    }

    fn before(state: &State, name: &str, args: Value) -> Value {
        call(state, "tool/before", json!({"name":name,"args":args,"session":sess()}))
    }

    fn after(state: &State, name: &str, content: &str) -> Value {
        call(state, "tool/after", json!({"name":name,"content":content,"session":sess()}))
    }

    #[test]
    fn manifest_claims_hooks() {
        let m = manifest();
        assert_eq!(m["name"], "repguard");
        assert_eq!(m["protocol"], "2.0");
        assert_eq!(m["hooks"], json!(["tool/before", "tool/after"]));
        assert_eq!(m["commands"], json!(["/repguard"]));
    }

    #[test]
    fn canon_ignores_key_order() {
        let a = json!({"x": 1, "y": {"b": 2, "a": 1}});
        let b = json!({"y": {"a": 1, "b": 2}, "x": 1});
        assert_eq!(call_hash("read", &a), call_hash("read", &b));
        assert_ne!(call_hash("read", &a), call_hash("write", &a));
    }

    #[test]
    fn consecutive_calls_nudge() {
        let state: State = std::sync::Mutex::new(HashMap::new());
        let args = json!({"cmd": "ls"});
        for i in 0..max_repeats() {
            let r = before(&state, "bash", args.clone());
            assert_eq!(r["decision"], "allow");
            let r = after(&state, "bash", "out");
            assert_eq!(
                r["content"].as_str().is_some_and(|c| c.contains("you are looping")),
                i + 1 == max_repeats()
            );
        }
        // flag consumed: a stray after with no new before stays untouched
        assert_eq!(after(&state, "bash", "out"), json!({}));
    }

    #[test]
    fn interleaved_calls_no_nudge() {
        // The reported false positive: re-running the same command between
        // edits — A-edit-A-edit-A — is iteration, not a loop.
        let state: State = std::sync::Mutex::new(HashMap::new());
        let render = json!({"cmd": "render chart && open chart.png"});
        for i in 0..max_repeats() {
            before(&state, "bash", render.clone());
            assert_eq!(after(&state, "bash", "out"), json!({}));
            before(&state, "edit", json!({"file": "chart.py", "n": i}));
            assert_eq!(after(&state, "edit", "ok"), json!({}));
        }
    }

    #[test]
    fn different_args_do_not_nudge() {
        let state: State = std::sync::Mutex::new(HashMap::new());
        for i in 0..max_repeats() + 2 {
            before(&state, "bash", json!({"cmd": i}));
            assert_eq!(after(&state, "bash", "out"), json!({}));
        }
    }

    #[test]
    fn window_prunes_old_calls() {
        let mut s = Sess::default();
        let old = Instant::now() - Duration::from_secs(9999);
        s.calls.push_back((7, old));
        s.last = Some((7, old));
        s.run = 5;
        // stale gap: window entry pruned, run restarts at 1
        assert_eq!(record(&mut s, 7, window()), 1);
    }

    #[test]
    fn shutdown_exits() {
        let state: State = std::sync::Mutex::new(HashMap::new());
        let (reply, exit) = handle(&json!({"method":"plugin/shutdown"}), &state);
        assert!(reply.is_none() && exit);
        let (reply, exit) = handle(&json!({"id":2,"method":"plugin/shutdown"}), &state);
        assert!(reply.is_some() && exit);
    }
}
