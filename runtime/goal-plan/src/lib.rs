//! A goal job's gates (`make: goal({...})` on brief.ts, core/brief goal.rs): the person's goal held as rules, from what's
//! on disk and what a fresh critic said, never from the agent that did the work. `/goal` in raea.app and an agent's own
//! goal are this one loop (11-26, Shane: "it can't stop unless all the goals pass").
//!   run    the command exits 0 in the brief's folder, within its timeout (run again at every status)
//!   file   the file exists: a deliverable, so `raea make status` lists it under missing, not here
//!   says   a fresh critic (`raea make judge`) found it true of the work, after the work's last change
//! The critic's verdict is review/goal.judge.json: {at, by, rules: {ID: {pass, why}}}.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use raea_core_brief::goal::Goal;
use serde_json::{Value, json};

/// One gate as `raea make status` lists it (a cut's are the same kind): `of` is the rule it's about
pub use raea_core_brief::make::Gate;

fn gate(name: &'static str, of: &str, clear: bool, says: String) -> Gate {
    Gate { name, of: Some(of.to_string()), clear, says }
}

/// Where the critic's verdict is kept, beside the brief.
pub const VERDICT: &str = "review/goal.judge.json";
/// The seconds a `run` rule gets when it names none.
pub const TIMEOUT: u32 = 300;

/// Folders that aren't the work: what the job writes about itself, and what tools keep.
const NOT_WORK: [&str; 7] = ["review", ".git", "node_modules", ".raea", "target", ".cache", ".raea-cloud"];

/// When the work last changed: the newest file under DIR, its own bookkeeping left out (a bounded walk).
pub fn newest(dir: &Path) -> Option<SystemTime> {
    let mut best: Option<SystemTime> = None;
    let mut stack = vec![dir.to_path_buf()];
    let mut seen = 0usize;
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            seen += 1;
            if seen > 20_000 {
                return best;
            }
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                if !(d == dir && NOT_WORK.contains(&name.as_str())) && name != "node_modules" && name != ".git" {
                    stack.push(p);
                }
            } else if t.is_file() && name != "NOTES.md" {
                if let Ok(m) = e.metadata().and_then(|m| m.modified()) {
                    best = Some(best.map_or(m, |b| b.max(m)));
                }
            }
        }
    }
    best
}

/// A command run in DIR through the system shell, stopped at its timeout: (passed, its last words).
pub fn run(cmd: &str, dir: &Path, secs: u32) -> (bool, String) {
    let mut c = if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", cmd]);
        c
    } else {
        let mut c = std::process::Command::new("sh");
        c.args(["-c", cmd]);
        c
    };
    let out = std::env::temp_dir().join(format!("raea-goal-{}-{}.log", std::process::id(), Instant::now().elapsed().as_nanos()));
    let Ok(log) = std::fs::File::create(&out) else { return (false, "couldn't make a log file for it".into()) };
    let Ok(err) = log.try_clone() else { return (false, "couldn't make a log file for it".into()) };
    let child = c.current_dir(dir).stdin(std::process::Stdio::null()).stdout(log).stderr(err).spawn();
    let mut child = match child {
        Ok(ch) => ch,
        Err(e) => return (false, format!("couldn't start it: {e}")),
    };
    let until = Instant::now() + Duration::from_secs(secs as u64);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if Instant::now() >= until => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => break None,
        }
    };
    let text = std::fs::read_to_string(&out).unwrap_or_default();
    let _ = std::fs::remove_file(&out);
    let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    let last: String = last.chars().take(200).collect();
    match status {
        Some(s) if s.success() => (true, "passes".into()),
        Some(s) => (false, format!("exits {}{}", s.code().map_or("by a signal".into(), |c| c.to_string()), if last.is_empty() { String::new() } else { format!(": {last}") })),
        None => (false, format!("still running after {secs} s: stopped (give the rule a longer `timeout`, or make it quicker)")),
    }
}

/// The critic's verdict, when there is one.
pub fn verdict(dir: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(dir.join(VERDICT)).ok()?).ok()
}

/// Every rule's gate but `file` rules (deliverables: status lists them as missing).
pub fn gates(g: &Goal, dir: &Path) -> Vec<Gate> {
    let says: Vec<_> = g.rules.iter().filter(|r| r.says.is_some()).collect();
    let v = if says.is_empty() { None } else { verdict(dir) };
    let judged = v.as_ref().and_then(|v| v["at"].as_u64()).map(|s| SystemTime::UNIX_EPOCH + Duration::from_secs(s));
    let changed = if says.is_empty() { None } else { newest(dir) };
    // at whole seconds, as the verdict keeps its time: work saved in the second the critic was asked counts as judged
    let secs = |t: SystemTime| t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let fresh = match (judged, changed) {
        (Some(j), Some(c)) => secs(j) >= secs(c),
        (Some(_), None) => true,
        _ => false,
    };
    let mut out = Vec::new();
    for r in &g.rules {
        if let Some(cmd) = &r.run {
            let (ok, said) = run(cmd, dir, r.timeout.unwrap_or(TIMEOUT));
            out.push(gate("run", &r.id, ok, if ok { format!("`{cmd}` passes") } else { format!("`{cmd}` {said}") }));
        } else if r.says.is_some() {
            let one = v.as_ref().map(|v| &v["rules"][&r.id]);
            let (clear, said) = match one {
                None => (false, "not judged yet: raea make judge".to_string()),
                Some(Value::Null) => (false, "the last judging didn't cover it: raea make judge".to_string()),
                Some(_) if !fresh => (false, "the work changed after the critic judged it: raea make judge again".to_string()),
                Some(x) if x["pass"] == true => (true, format!("a fresh critic found it true: {}", x["why"].as_str().unwrap_or(""))),
                Some(x) => (false, format!("a fresh critic found it not true yet: {}", x["why"].as_str().unwrap_or(""))),
            };
            out.push(gate("says", &r.id, clear, said));
        }
    }
    out
}

/// What the critic is asked: the goal, each `says` rule by id, and the answer's shape. It reads; it changes nothing.
pub fn prompt(g: &Goal, dir: &Path) -> String {
    let rules: Vec<String> = g.rules.iter().filter_map(|r| r.says.as_ref().map(|s| format!("- {}: {s}", r.id))).collect();
    format!(
        "You are a fresh critic. You did none of this work and owe it nothing. The work is in {d}: read whatever you need \
there (its files, its renders, NOTES.md), and change nothing.\n\nThe goal, as the person said it: {goal}\n\nFor each rule \
below, decide whether it is true of the work as it is on disk now. Count it true only when you have seen it hold; if you \
can't tell, it isn't true yet. Say why in one sentence, naming what you looked at.\n\n{rules}\n\nAnswer last with one JSON \
object and nothing after it: {{\"rules\": {{\"ID\": {{\"pass\": true, \"why\": \"...\"}}}}}}",
        d = dir.display(),
        goal = g.goal,
        rules = rules.join("\n")
    )
}

/// The verdict: the object around the text's last `"rules"` key (a critic may think aloud, or quote the shape, first).
pub fn last_verdict(text: &str) -> Option<Value> {
    let key = text.rfind("\"rules\"")?;
    // the nearest brace before the key opens the verdict
    let open = text[..key].rfind('{')?;
    let v: Value = serde_json::Deserializer::from_str(&text[open..]).into_iter::<Value>().next()?.ok()?;
    v["rules"].is_object().then_some(v)
}

/// The critics `raea make judge` can ask, each a fresh session that may only read.
pub const BY: [&str; 2] = ["claude", "kimi"];

/// One fresh critic over the goal's `says` rules: its verdict kept at review/goal.judge.json, stamped with when it was
/// asked (so work changed while it judged counts as changed after). Returns the verdict.
pub fn judge(g: &Goal, dir: &Path, by: &str) -> Result<Value, String> {
    if !g.rules.iter().any(|r| r.says.is_some()) {
        return Err("raea make judge: the goal has no `says` rules; its `run` and `file` rules are checked by raea make status".into());
    }
    let asked = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let p = prompt(g, dir);
    let mut cmd = match by {
        "claude" => {
            let mut c = std::process::Command::new("claude");
            c.args(["-p", &p, "--allowedTools", "Read Grep Glob", "--output-format", "json"]);
            c
        }
        "kimi" => {
            let mut c = std::process::Command::new("kimi");
            c.args(["-p", &p]);
            c
        }
        o => return Err(format!("raea make judge: --by {o}: the critics are {}", BY.join(", "))),
    };
    let out = cmd.current_dir(dir).stdin(std::process::Stdio::null()).output().map_err(|e| format!("raea make judge: couldn't run {by}: {e} (is its CLI installed and signed in?)"))?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    // claude's json output is {result: "..."}; kimi's is the words
    let words = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["result"].as_str().map(String::from)).unwrap_or(text);
    let tail = &words[words.floor_char_boundary(words.len().saturating_sub(300))..];
    let mut v = last_verdict(&words).ok_or_else(|| format!("raea make judge: {by} answered no verdict: {}", tail.trim()))?;
    v["at"] = json!(asked);
    v["by"] = json!(by);
    v["goal"] = json!(g.goal);
    let file: PathBuf = dir.join(VERDICT);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("raea make judge: {}: {e}", parent.display()))?;
    }
    std::fs::write(&file, serde_json::to_string_pretty(&v).unwrap_or_default()).map_err(|e| format!("raea make judge: {}: {e}", file.display()))?;
    Ok(v)
}

#[cfg(test)]
mod tests;
