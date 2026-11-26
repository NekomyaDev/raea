//! A cut job held to its brief (MK25-MK27): the contract's doctor, the quotes, the count and open items, and each
//! delivered file's shape, words and frames, on fixtures written here (an MP4 is only its moov box).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use raea_core_brief::{cut, make::Make, read};
use serde_json::{Value, json};

use crate::asked::Ledger;
use crate::mp4;
use crate::status::{Job, status};

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("raea-make-cut-{name}-{}", std::process::id()));
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

fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

/// An MP4 that is only its boxes: `seconds` long, a w x h video track, turned a quarter when `turned`.
fn mp4_bytes(seconds: f64, w: u32, h: u32, turned: bool) -> Vec<u8> {
    let mut mvhd = vec![0u8; 100];
    mvhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
    mvhd[16..20].copy_from_slice(&((seconds * 1000.0) as u32).to_be_bytes());
    let mut tkhd = vec![0u8; 84];
    let (a, b): (u32, u32) = if turned { (0, 0x0001_0000) } else { (0x0001_0000, 0) };
    tkhd[40..44].copy_from_slice(&a.to_be_bytes());
    tkhd[44..48].copy_from_slice(&b.to_be_bytes());
    tkhd[76..80].copy_from_slice(&(w << 16).to_be_bytes());
    tkhd[80..84].copy_from_slice(&(h << 16).to_be_bytes());
    let moov = bx(b"moov", &[bx(b"mvhd", &mvhd), bx(b"trak", &bx(b"tkhd", &tkhd))].concat());
    [bx(b"ftyp", b"isom\0\0\0\0"), bx(b"mdat", &[0u8; 64]), moov].concat()
}

const BRIEF: &str = "Deliverables: 10 total videos (4 solos / 1 mashup per angle).\nUse the phrase \"free, fun, and easy.\"\nDon't let it read as White Elephant.\nEnd on the CTA: \"Make your Wishlist and organize your Gift Exchange on Elfster. It's free.\"\nCTV specs from the client [TO CONFIRM]\n";

fn contract(extra: &str) -> Make {
    read(
        "make",
        &format!(
            r#"{{"kind": "cut", "cut": {{"brief": "media/brief.txt", "count": {{"value": 2, "from": "10 total videos"}}, "seconds": [20, 30], "formats": ["9:16"],
            "cuts": [{{"id": "a1_s1", "angle": "1", "hook": "Direct Problem", "text": "captions"}}, {{"id": "a1_s2"}}],
            "say": [{{"text": "free, fun, and easy", "from": "Use the phrase \"free, fun, and easy.\""}}],
            "never": [{{"text": "white elephant", "from": "Don't let it read as White Elephant."}}],
            "cta": {{"text": "Make your Wishlist and organize your Gift Exchange on Elfster. It's free.", "from": "End on the CTA"}}{extra}}}}}"#
        ),
    )
    .unwrap()
}

fn run(make: &Make, d: &Path) -> Value {
    status(&Job { make, dir: d, project: None, record: &[], rounds: &[], verdict: None, judged_at: None, ledger: &Ledger::default(), score_gates: false })
}

fn gates<'a>(s: &'a Value, name: &str) -> Vec<&'a Value> {
    s["gates"].as_array().unwrap().iter().filter(|g| g["gate"] == name).collect()
}

/// Words said evenly from 0.5 s, one every 0.6 s, as `raea words` writes them.
fn words(line: &str) -> String {
    let w: Vec<Value> = line.split_whitespace().enumerate().map(|(i, t)| json!({"text": t, "start": 0.5 + i as f64 * 0.6, "end": 0.8 + i as f64 * 0.6})).collect();
    serde_json::to_string(&w).unwrap()
}

/// A delivered file with its words and frames check written after it.
fn deliver(d: &Path, id: &str, said: &str, frame_errors: &[&str]) {
    let f = put(d, &format!("out/cuts/{id}_9x16.mp4"), &mp4_bytes(said.split_whitespace().count() as f64 * 0.6 + 1.0, 1080, 1920, false));
    touch(&f, -60);
    put(d, &format!("out/cuts/{id}_9x16.words.json"), words(said).as_bytes());
    let findings: Vec<Value> = frame_errors.iter().map(|e| json!({"level": "error", "kind": "face-cut", "says": e})).collect();
    put(d, &format!("out/cuts/{id}_9x16.frames-check.json"), json!({"findings": findings}).to_string().as_bytes());
}

const GOOD: &str = "So it draws the names for you it's free fun and easy to set up and everyone makes a wishlist so nobody draws their own name ever again Make your Wishlist and organize your Gift Exchange on Elfster It's free";

#[test]
fn the_contract_is_read_and_doctored() {
    let m = contract("");
    assert_eq!(m.kind, raea_core_brief::make::Kind::Cut);
    assert_eq!(raea_core_brief::make::deliverables(&m), vec!["out/cuts/a1_s1_9x16.mp4", "out/cuts/a1_s2_9x16.mp4"]);
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
    let s = run(&contract(""), &d);
    assert!(gates(&s, "quotes")[0]["says"].as_str().unwrap().contains("no brief text"));
    put(&d, "media/brief.txt", BRIEF.as_bytes());
    assert_eq!(gates(&run(&contract(""), &d), "quotes")[0]["clear"], true);
    // a rule the brief doesn't say, or says in other words, is held
    let invented = contract(r#", "open": [{"question": "Which code?", "from": "the offer is PETS99"}]"#);
    let s = run(&invented, &d);
    let g = gates(&s, "quotes")[0];
    assert_eq!(g["clear"], false);
    assert!(g["says"].as_str().unwrap().contains("Which code?"), "{g}");
}

#[test]
fn the_count_and_open_items_hold_the_job() {
    let d = dir("count");
    put(&d, "media/brief.txt", BRIEF.as_bytes());
    let mut m = contract("");
    m.cut.as_mut().unwrap().count.as_mut().unwrap().value = 10;
    assert_eq!(gates(&run(&m, &d), "count")[0]["clear"], false);
    let mut asked = contract(r#", "open": [{"question": "Only 2 interviews came: cut 2?", "from": "10 total videos", "about": "count"}]"#);
    asked.cut.as_mut().unwrap().count.as_mut().unwrap().value = 10;
    let s = run(&asked, &d);
    assert_eq!(gates(&s, "count")[0]["clear"], false, "an open item without its answer doesn't excuse the count");
    assert!(gates(&s, "open")[0]["says"].as_str().unwrap().starts_with("unanswered"));
    let o = &mut asked.cut.as_mut().unwrap().open[0];
    o.answer = Some("yes, the 2 we have".into());
    o.by = Some("Shane".into());
    let s = run(&asked, &d);
    assert_eq!(gates(&s, "count")[0]["clear"], true);
    assert_eq!(gates(&s, "open")[0]["clear"], true);
}

#[test]
fn a_delivered_cut_is_held_to_its_words_shape_and_frames() {
    let d = dir("files");
    put(&d, "media/brief.txt", BRIEF.as_bytes());
    put(&d, "NOTES.md", b"## What the skill got wrong\n- nothing\n");
    let m = contract("");
    let s = run(&m, &d);
    assert_eq!(s["done"], false);
    assert_eq!(s["missing"].as_array().unwrap().len(), 2, "{s}");
    deliver(&d, "a1_s1", GOOD, &[]);
    deliver(&d, "a1_s2", GOOD, &[]);
    let s = run(&m, &d);
    assert_eq!(s["done"], true, "{}", crate::status::text(&s));
    // a banned phrase heard, the CTA cut, a phrase missing, a face cut
    deliver(&d, "a1_s2", "we did a white elephant swap last year and it was chaos so we tried something new", &["a face is cut by the left edge from 4.0 s to 5.5 s"]);
    let s = run(&m, &d);
    assert_eq!(s["done"], false);
    let of = |name: &str| gates(&s, name).into_iter().find(|g| g["of"] == "out/cuts/a1_s2_9x16.mp4").unwrap().clone();
    assert!(of("never")["says"].as_str().unwrap().contains("\"white elephant\" heard at 2.3 s"), "{}", of("never"));
    assert_eq!(of("say")["clear"], false);
    assert_eq!(of("cta")["clear"], false);
    assert!(of("frames")["says"].as_str().unwrap().contains("left edge"));
    // the wrong shape, and a report from before the file's last change
    let f = put(&d, "out/cuts/a1_s1_9x16.mp4", &mp4_bytes(45.0, 1920, 1080, false));
    touch(&f, 60);
    let s = run(&m, &d);
    let shape = gates(&s, "shape").into_iter().find(|g| g["of"] == "out/cuts/a1_s1_9x16.mp4").unwrap().clone();
    assert!(shape["says"].as_str().unwrap().contains("1920x1080 isn't 9:16") && shape["says"].as_str().unwrap().contains("45.0 s isn't 20-30 s"), "{shape}");
    let words = gates(&s, "words").into_iter().find(|g| g["of"] == "out/cuts/a1_s1_9x16.mp4").unwrap().clone();
    assert!(words["says"].as_str().unwrap().contains("changed after it"), "{words}");
}

#[test]
fn an_mp4_is_measured_from_its_boxes() {
    let d = dir("mp4");
    let f = put(&d, "a.mp4", &mp4_bytes(29.5, 1080, 1920, false));
    assert_eq!(mp4::shape(&f), Some(mp4::Shape { seconds: 29.5, w: 1080, h: 1920 }));
    // a phone take stored sideways and turned by its matrix reads as shown
    let f = put(&d, "b.mov", &mp4_bytes(12.0, 1920, 1080, true));
    assert_eq!(mp4::shape(&f), Some(mp4::Shape { seconds: 12.0, w: 1080, h: 1920 }));
    assert_eq!(mp4::shape(&put(&d, "c.mp4", b"not a video")), None);
}

#[test]
fn heard_the_way_people_say_it() {
    // Gemini's Elfster A1 solo (11-25): the transcript splits "Wishlist" and the speaker fills the phrase out
    let d = dir("said");
    put(&d, "media/brief.txt", BRIEF.as_bytes());
    let m = contract("");
    let line = "so it draws the names for you and everyone gets a gift they want It's free it's fun and it's easy so set it up tonight with your whole family Make your wish list and organize your gift exchange on Elfster It's free";
    deliver(&d, "a1_s1", line, &[]);
    let s = run(&m, &d);
    let of = |s: &Value, name: &str| gates(s, name).into_iter().find(|g| g["of"] == "out/cuts/a1_s1_9x16.mp4").unwrap().clone();
    assert_eq!(of(&s, "say")["clear"], true, "{}", of(&s, "say"));
    assert_eq!(of(&s, "cta")["clear"], true, "{}", of(&s, "cta"));
    // the brief's order is the phrase: "fun, free and easy" isn't it
    deliver(&d, "a1_s1", &line.replace("It's free it's fun", "It's fun it's free"), &[]);
    assert_eq!(of(&run(&m, &d), "say")["clear"], false);
}
