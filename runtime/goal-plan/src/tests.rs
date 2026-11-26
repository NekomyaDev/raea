use super::*;
use raea_core_brief::goal::{Goal, GoalRule};

fn rule(id: &str) -> GoalRule {
    GoalRule { id: id.into(), run: None, timeout: None, file: None, says: None }
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("raea-goal-plan-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_command_passes_or_says_why() {
    let d = tmp("run");
    let g = Goal { goal: "tests pass".into(), rules: vec![GoalRule { run: Some("true".into()), ..rule("ok") }, GoalRule { run: Some("echo broke; exit 3".into()), ..rule("bad") }] };
    let gs = gates(&g, &d);
    assert_eq!(gs.len(), 2);
    assert!(gs[0].clear, "{gs:?}");
    assert!(!gs[1].clear);
    assert!(gs[1].says.contains("exits 3") && gs[1].says.contains("broke"), "{}", gs[1].says);
}

#[test]
fn a_command_past_its_timeout_fails() {
    let d = tmp("slow");
    let (ok, said) = run("sleep 5", &d, 1);
    assert!(!ok);
    assert!(said.contains("still running after 1 s"), "{said}");
}

#[test]
fn words_need_a_fresh_critic() {
    let d = tmp("says");
    std::fs::write(d.join("work.txt"), "the work").unwrap();
    let g = Goal { goal: "a friendly reply".into(), rules: vec![GoalRule { says: Some("the reply is friendly".into()), ..rule("friendly") }] };
    // never judged
    let gs = gates(&g, &d);
    assert!(!gs[0].clear && gs[0].says.contains("raea make judge"), "{gs:?}");
    // judged after the work, and found true
    let later = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs() + 5;
    std::fs::create_dir_all(d.join("review")).unwrap();
    let v = json!({"at": later, "by": "claude", "rules": {"friendly": {"pass": true, "why": "it says hello warmly"}}});
    std::fs::write(d.join(VERDICT), v.to_string()).unwrap();
    assert!(gates(&g, &d)[0].clear);
    // the critic's own file doesn't count as the work changing
    assert!(newest(&d).unwrap() < SystemTime::UNIX_EPOCH + Duration::from_secs(later));
    // judged before the work changed: stale
    let earlier = json!({"at": 1, "rules": {"friendly": {"pass": true, "why": ""}}});
    std::fs::write(d.join(VERDICT), earlier.to_string()).unwrap();
    let gs = gates(&g, &d);
    assert!(!gs[0].clear && gs[0].says.contains("changed after"), "{gs:?}");
    // judged and found not true
    let no = json!({"at": later, "rules": {"friendly": {"pass": false, "why": "it is curt"}}});
    std::fs::write(d.join(VERDICT), no.to_string()).unwrap();
    let gs = gates(&g, &d);
    assert!(!gs[0].clear && gs[0].says.contains("it is curt"), "{gs:?}");
}

#[test]
fn file_rules_are_left_to_the_deliverables() {
    let d = tmp("file");
    let g = Goal { goal: "an export".into(), rules: vec![GoalRule { file: Some("out/final.mp4".into()), ..rule("export") }] };
    assert!(gates(&g, &d).is_empty());
}

#[test]
fn the_verdict_is_the_last_json_with_rules() {
    let t = "the shape is {\"rules\": {\"ID\": ...}} so: {\"x\": 1} then {\"rules\": {\"a\": {\"pass\": true, \"why\": \"seen\"}}}";
    assert_eq!(last_verdict(t).unwrap()["rules"]["a"]["pass"], true);
    assert!(last_verdict("no json here").is_none());
}

#[test]
fn the_rules_are_doctored() {
    let g = Goal { goal: "".into(), rules: vec![GoalRule { run: Some("x".into()), says: Some("y".into()), ..rule("Two") }, rule("two")] };
    let d = raea_core_brief::goal::doctor(&g);
    assert!(d.iter().any(|l| l.contains("no `goal`")), "{d:?}");
    assert!(d.iter().any(|l| l.contains("`Two`: ids are")), "{d:?}");
    assert!(d.iter().any(|l| l.contains("exactly one") && l.contains("it has 2")), "{d:?}");
    assert!(d.iter().any(|l| l.contains("`two` needs exactly one") && l.contains("it has 0")), "{d:?}");
}
