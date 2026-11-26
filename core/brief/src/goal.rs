//! A goal job's contract: `make: goal({...})` on brief.ts, the person's goal written as rules before the work starts
//! (11-26, Shane: "we essentially just need to make sure that it can't stop unless all the goals pass, or all the rules
//! in the brief"). `/goal` in raea.app and an agent's own goal are this: the goal in the person's words, then each
//! thing that must hold, as a command that must pass, a file that must exist, or words a fresh critic must find true.
//! `raea make status` (runtime/goal-plan) decides from them whether the agent may stop; the agent saying so never does.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(optional_fields)]
#[ts(rename = "MakeGoal")]
pub struct Goal {
    /// the goal as the person said it
    pub goal: String,
    /// what must hold before the agent may stop: every one, each checked in code or by a fresh critic
    #[serde(default)]
    #[ts(as = "Option<Vec<GoalRule>>", optional)]
    pub rules: Vec<GoalRule>,
}

/// One thing that must hold. Exactly one of `run`, `file` or `says`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(optional_fields)]
#[ts(rename = "MakeGoalRule")]
pub struct GoalRule {
    /// a short name: [a-z0-9_], how status, the critic and NOTES.md name it
    pub id: String,
    /// a command that must exit 0, run in the brief's folder (`bun test`, `raea check edit.tsx`)
    pub run: Option<String>,
    /// the seconds `run` gets before it counts as failed (300 when left out)
    pub timeout: Option<u32>,
    /// a file that must exist, relative to the brief (`a|b`: either; `dir/`: three or more pictures in it)
    pub file: Option<String>,
    /// words that must be true of the work, judged by a fresh critic (`raea make judge`), never by the agent that did it
    pub says: Option<String>,
}

/// What the goal's rules still lack, each with what to do. Empty when it's ready.
pub fn doctor(g: &Goal) -> Vec<String> {
    let mut out = Vec::new();
    if g.goal.trim().is_empty() {
        out.push("no `goal`: the goal in the person's own words".into());
    }
    if g.rules.is_empty() {
        out.push("no `rules`: what must hold before you stop, each a `run` command, a `file` or words a critic judges (`says`)".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for r in &g.rules {
        if !crate::make::is_name(&r.id) {
            out.push(format!("rule `{}`: ids are [a-z0-9_]", r.id));
        }
        if !seen.insert(r.id.as_str()) {
            out.push(format!("rule `{}` is listed twice", r.id));
        }
        let n = [r.run.is_some(), r.file.is_some(), r.says.is_some()].iter().filter(|x| **x).count();
        if n != 1 {
            out.push(format!("rule `{}` needs exactly one of `run`, `file` or `says` (it has {n})", r.id));
        }
        if r.timeout.is_some() && r.run.is_none() {
            out.push(format!("rule `{}` has a `timeout` but no `run`", r.id));
        }
    }
    out
}
