//! A cut job held to its brief (MK25-MK27): the contract's doctor, the quotes, the count and open items, and each
//! delivered file's shape, words and frames. The delivered file is raea's 3 s 320x180 test clip.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use raea_core_brief::{cut, make::Make, read};
use serde_json::{Value, json};

use crate::{Gate, gates};

const CLIP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../engine/rune/media/tests/fixtures/clip.mp4");

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("raea-cut-plan-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn put(d: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
    let p = d.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, bytes).unwrap();
    p
}

/// `p` as changed `secs` from now (a report written after its video is newer than it)
fn touch(p: &Path, secs: i64) {
    let t = if secs >= 0 { SystemTime::now() + Duration::from_secs(secs as u64) } else { SystemTime::now() - Duration::from_secs((-secs) as u64) };
    std::fs::File::options().write(true).open(p).unwrap().set_modified(t).unwrap();
}

const BRIEF: &str = "Deliverables: 10 total videos (4 solos / 1 mashup per angle).\nUse the phrase \"free, fun, and easy.\"\nDon't let it read as White Elephant.\nEnd on the CTA: \"Make your Wishlist and organize your Gift Exchange on Elfster. It's free.\"\nCTV specs from the client [TO CONFIRM]\n";

fn contract(extra: &str) -> Make {
    read(
        "make",
        &format!(
            r#"{{"kind": "cut", "cut": {{"brief": "media/brief.txt", "count": {{"value": 2, "from": "10 total videos"}}, "seconds": [2, 4], "formats": ["16:9"],
            "cuts": [{{"id": "a1_s1", "angle": "1", "hook": "Direct Problem", "text": "captions"}}, {{"id": "a1_s2"}}],
            "say": [{{"text": "free, fun, and easy", "from": "Use the phrase \"free, fun, and easy.\""}}],
            "never": [{{"text": "white elephant", "from": "Don't let it read as White Elephant."}}],
            "cta": {{"text": "Make your Wishlist and organize your Gift Exchange on Elfster. It's free.", "from": "End on the CTA"}}{extra}}}}}"#
        ),
    )
    .unwrap()
}

fn run(m: &Make, d: &Path) -> Vec<Gate> {
    gates(m.cut.as_ref().unwrap(), d)
}

fn named<'a>(g: &'a [Gate], name: &str) -> Vec<&'a Gate> {
    g.iter().filter(|g| g.name == name).collect()
}

fn of(g: &[Gate], name: &str, file: &str) -> Gate {
    g.iter().find(|g| g.name == name && g.of.as_deref() == Some(file)).unwrap_or_else(|| panic!("no {name} gate of {file}")).clone()
}

/// Words said evenly from 0.5 s, one every 0.6 s, as `raea words` writes them.
fn words(line: &str) -> String {
    let w: Vec<Value> = line.split_whitespace().enumerate().map(|(i, t)| json!({"text": t, "start": 0.5 + i as f64 * 0.6, "end": 0.8 + i as f64 * 0.6})).collect();
    serde_json::to_string(&w).unwrap()
}

/// A delivered file (the test clip) with its words and frames check written after it.
fn deliver(d: &Path, file: &str, said: &str, frame_errors: &[&str]) {
    let f = put(d, file, &std::fs::read(CLIP).unwrap());
    touch(&f, -60);
    let stem = file.trim_end_matches(".mp4");
    put(d, &format!("{stem}.words.json"), words(said).as_bytes());
    let findings: Vec<Value> = frame_errors.iter().map(|e| json!({"level": "error", "kind": "face-cut", "says": e})).collect();
    put(d, &format!("{stem}.frames-check.json"), json!({"findings": findings}).to_string().as_bytes());
}

const GOOD: &str = "So it draws the names for you it's free fun and easy to set up and everyone makes a wishlist so nobody draws their own name ever again Make your Wishlist and organize your Gift Exchange on Elfster It's free";
const S1: &str = "out/cuts/a1_s1_16x9.mp4";
const S2: &str = "out/cuts/a1_s2_16x9.mp4";

#[test]
fn the_contract_is_read_and_doctored() {
    let m = contract("");
    assert_eq!(m.kind, raea_core_brief::make::Kind::Cut);
    assert_eq!(raea_core_brief::make::deliverables(&m), vec![S1, S2]);
    assert!(raea_core_brief::make::doctor(&m).is_empty(), "{:?}", raea_core_brief::make::doctor(&m));
    let bad: Make = read("make", r#"{"kind": "cut", "cut": {"brief": "b.txt", "cuts": [{"id": "A-1", "formats": ["tall"], "text": "both"}], "say": [{"text": "x", "from": ""}]}}"#).unwrap();
    let d = raea_core_brief::make::doctor(&bad).join("\n");
    for want in ["no `count`", "no `cta`", "ids are [a-z0-9_]", "isn't W:H", "no length", "captions, hook or none", "has no `from`"] {
        assert!(d.contains(want), "{want} in\n{d}");
    }
    let none: Make = read("make", r#"{"kind": "cut"}"#).unwrap();
    assert!(raea_core_brief::make::doctor(&none).iter().any(|x| x.contains("no `cut`")));
    assert_eq!(cut::norm("24/7 Virtual Vet Care. It’s free!"), "24 7 virtual vet care its free");
}

#[test]
fn every_rule_quotes_the_brief() {
    let d = dir("quotes");
    assert!(named(&run(&contract(""), &d), "quotes")[0].says.contains("no brief text"));
    put(&d, "media/brief.txt", BRIEF.as_bytes());
    assert!(named(&run(&contract(""), &d), "quotes")[0].clear);
    // a rule the brief doesn't say, or says in other words, is held
    let invented = contract(r#", "open": [{"question": "Which code?", "from": "the offer is PETS99"}]"#);
    let g = run(&invented, &d);
    assert!(!named(&g, "quotes")[0].clear);
    assert!(named(&g, "quotes")[0].says.contains("Which code?"), "{:?}", named(&g, "quotes"));
}

#[test]
fn the_count_and_open_items_hold_the_job() {
    let d = dir("count");
    put(&d, "media/brief.txt", BRIEF.as_bytes());
    let mut m = contract("");
    m.cut.as_mut().unwrap().count.as_mut().unwrap().value = 10;
    assert!(!named(&run(&m, &d), "count")[0].clear);
    let mut asked = contract(r#", "open": [{"question": "Only 2 interviews came: cut 2?", "from": "10 total videos", "about": "count"}]"#);
    asked.cut.as_mut().unwrap().count.as_mut().unwrap().value = 10;
    let g = run(&asked, &d);
    assert!(!named(&g, "count")[0].clear, "an open item without its answer doesn't excuse the count");
    assert!(named(&g, "open")[0].says.starts_with("unanswered"));
    let o = &mut asked.cut.as_mut().unwrap().open[0];
    o.answer = Some("yes, the 2 we have".into());
    o.by = Some("Shane".into());
    let g = run(&asked, &d);
    assert!(named(&g, "count")[0].clear && named(&g, "open")[0].clear);
}

#[test]
fn a_delivered_cut_is_held_to_its_words_shape_and_frames() {
    let d = dir("files");
    put(&d, "media/brief.txt", BRIEF.as_bytes());
    let m = contract("");
    deliver(&d, S1, GOOD, &[]);
    deliver(&d, S2, GOOD, &[]);
    let g = run(&m, &d);
    assert!(g.iter().all(|g| g.clear), "{g:#?}");
    assert_eq!(of(&g, "shape", S1).says, "320x180, 3.0 s");
    // a banned phrase heard, the CTA cut, a phrase missing, a face cut
    deliver(&d, S2, "we did a white elephant swap last year and it was chaos so we tried something new", &["a face is cut by the left edge from 4.0 s to 5.5 s"]);
    let g = run(&m, &d);
    assert!(of(&g, "never", S2).says.contains("\"white elephant\" heard at 2.3 s"), "{:?}", of(&g, "never", S2));
    assert!(!of(&g, "say", S2).clear && !of(&g, "cta", S2).clear);
    assert!(of(&g, "frames", S2).says.contains("left edge"));
    // a report from before the file's last change says nothing about it now
    touch(&d.join(S1), 60);
    let g = run(&m, &d);
    assert!(of(&g, "words", S1).says.contains("changed after it"), "{:?}", of(&g, "words", S1));
    assert!(of(&g, "frames", S1).says.contains("changed after it"));
}

#[test]
fn the_wrong_shape_is_named() {
    let d = dir("shape");
    put(&d, "media/brief.txt", BRIEF.as_bytes());
    let mut m = contract("");
    let p = &mut m.cut.as_mut().unwrap().cuts[0];
    p.formats = vec!["9:16".into()];
    p.seconds = Some([20.0, 30.0]);
    deliver(&d, "out/cuts/a1_s1_9x16.mp4", GOOD, &[]);
    let says = of(&run(&m, &d), "shape", "out/cuts/a1_s1_9x16.mp4").says;
    assert!(says.contains("320x180 isn't 9:16") && says.contains("3.0 s isn't 20-30 s"), "{says}");
    put(&d, "out/cuts/a1_s1_9x16.mp4", b"not a video");
    assert!(!of(&run(&m, &d), "shape", "out/cuts/a1_s1_9x16.mp4").clear);
}

#[test]
fn heard_the_way_people_say_it() {
    // Gemini's Elfster A1 solo (11-25): the transcript splits "Wishlist" and the speaker fills the phrase out
    let d = dir("said");
    put(&d, "media/brief.txt", BRIEF.as_bytes());
    let m = contract("");
    let line = "so it draws the names for you and everyone gets a gift they want It's free it's fun and it's easy so set it up tonight with your whole family Make your wish list and organize your gift exchange on Elfster It's free";
    deliver(&d, S1, line, &[]);
    let g = run(&m, &d);
    assert!(of(&g, "say", S1).clear, "{:?}", of(&g, "say", S1));
    assert!(of(&g, "cta", S1).clear, "{:?}", of(&g, "cta", S1));
    // the brief's order is the phrase: "fun, free and easy" isn't it
    deliver(&d, S1, &line.replace("It's free it's fun", "It's fun it's free"), &[]);
    assert!(!of(&run(&m, &d), "say", S1).clear);
}
