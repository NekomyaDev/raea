//! A cut job's gates (docs/make.feature MK25-MK27): the brief as rules (`make: cut({...})`, core/brief cut.rs), held
//! from what's on disk. The quotes: every rule's `from` is in the brief's text. The count: as many cuts as the brief
//! asks for, or an answer saying why not. The open items: each answered. Then each delivered file: its shape and
//! length; its words, heard after its last change (FILE.words.json, `raea words`), with every `say` phrase heard or
//! shown, no `never` phrase in its words or its text, and the CTA in its last seconds; and its frames check, run after
//! its last change (FILE.frames-check.json, `raea frames check`), with no face cut and no text too small.

use std::path::Path;

use raea_core_brief::cut::{self, Cut, norm};
use serde_json::Value;

/// One gate as `raea make status` lists it: its name, the file or item it's about, whether it's clear, what it says.
#[derive(Debug, Clone, PartialEq)]
pub struct Gate {
    pub name: &'static str,
    pub of: Option<String>,
    pub clear: bool,
    pub says: String,
}

fn gate(name: &'static str, of: Option<&str>, clear: bool, says: String) -> Gate {
    Gate { name, of: of.map(str::to_string), clear, says }
}

/// A delivered file's width, height and seconds, from its header boxes (io/soft's probe: the media isn't read, and a
/// file re-encoded after its render is read as it is now).
fn shape(f: &Path) -> Option<(u32, u32, f64)> {
    raea_io_soft::probe(f).ok().map(|p| (p.width.round() as u32, p.height.round() as u32, p.secs))
}

/// A report beside the file (FILE.words.json, FILE.frames-check.json), read when it's newer than the file (a report
/// from before the last change says nothing about it now): Ok(json), or Err(why not).
fn fresh(f: &Path, ext: &str, run: &str) -> Result<Value, String> {
    let r = f.with_extension(ext);
    let changed = |p: &Path| -> Option<std::time::SystemTime> { p.metadata().ok()?.modified().ok() };
    let Some(at) = changed(&r) else { return Err(format!("never run: {run}")) };
    if changed(f).is_some_and(|m| m > at) {
        return Err(format!("the file changed after it: {run} again"));
    }
    std::fs::read_to_string(&r).ok().and_then(|s| serde_json::from_str(&s).ok()).ok_or_else(|| format!("{} isn't JSON: {run} again", r.display()))
}

/// What a file says: its heard words with their times in seconds (`raea words`: start/end, or start_ms/end_ms).
fn heard(words: &Value) -> Vec<(String, f64, f64)> {
    let list = words.as_array().or_else(|| words["words"].as_array()).cloned().unwrap_or_default();
    list.iter()
        .filter_map(|w| {
            let t = norm(w["text"].as_str().or_else(|| w["word"].as_str())?);
            let at = |k: &str| w[k].as_f64().or(w[format!("{k}_ms")].as_f64().map(|ms| ms * 0.001));
            let s = at("start")?;
            let e = at("end").unwrap_or(s);
            (!t.is_empty()).then_some((t, s, e))
        })
        .collect()
}

/// What a file shows: each text of its render manifest (FILE.render.json) and the second it's first on screen.
fn shown(manifest: &Value) -> Vec<(String, f64)> {
    let fps = manifest["fps"].as_f64().filter(|f| *f > 0.0).unwrap_or(30.0);
    let mut out: Vec<(String, f64)> = Vec::new();
    for fr in manifest["frames"].as_array().into_iter().flatten() {
        let at = fr["frame"].as_f64().unwrap_or(0.0) / fps;
        for t in fr["text"].as_array().into_iter().flatten() {
            let s = norm(t["text"].as_str().unwrap_or(""));
            if !s.is_empty() && t["opacity"].as_f64().unwrap_or(1.0) > 0.05 && !out.iter().any(|(o, _)| *o == s) {
                out.push((s, at));
            }
        }
    }
    out
}

/// `phrase` in `text` as whole words.
fn has(text: &str, phrase: &str) -> bool {
    !phrase.is_empty() && format!(" {text} ").contains(&format!(" {phrase} "))
}

/// `got`'s words with each compound the brief writes as one word joined back ("wish list" heard, "Wishlist" written),
/// each with the index of its first word in `got`.
fn joined<'a>(got: &[&'a str], want: &[&str]) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < got.len() {
        let k = (2..=3).rev().find(|&k| i + k <= got.len() && want.contains(&got[i..i + k].concat().as_str())).unwrap_or(1);
        out.push((got[i..i + k].concat(), i));
        i += k;
    }
    out
}

/// Where `phrase` (normalised) is first said in `got`: the index of its first word. Its words in order, compounds
/// joined, each at most `gap` words after the one before (1: word for word; people say "it's free, it's fun, and it's
/// easy" for "free, fun, and easy", which a gap of 2 takes and a reordered "fun, free, and easy" doesn't).
fn said_at(got: &[&str], phrase: &str, gap: usize) -> Option<usize> {
    let want: Vec<&str> = phrase.split(' ').filter(|w| !w.is_empty()).collect();
    let got = joined(got, &want);
    (0..got.len()).find(|&s| {
        if want.is_empty() || got[s].0 != want[0] {
            return false;
        }
        let mut at = s;
        want[1..].iter().all(|w| match (at + 1..(at + 1 + gap).min(got.len())).find(|&j| got[j].0 == *w) {
            Some(j) => {
                at = j;
                true
            }
            None => false,
        })
    })
    .map(|s| got[s].1)
}

/// How far apart a `say` phrase's words may be heard.
const SAY_GAP: usize = 2;

/// The share of `want`'s words that `got` has in order (longest common subsequence): a CTA heard with a word misheard
/// still lands.
fn in_order(want: &[&str], got: &[&str]) -> f64 {
    if want.is_empty() {
        return 1.0;
    }
    let mut row = vec![0usize; got.len() + 1];
    for w in want {
        let mut prev = 0;
        for (j, g) in got.iter().enumerate() {
            let keep = row[j + 1];
            row[j + 1] = if w == g { prev + 1 } else { row[j + 1].max(row[j]) };
            prev = keep;
        }
    }
    row[got.len()] as f64 / want.len() as f64
}

/// How much of the CTA a run of words must have, in order.
const CTA_SHARE: f64 = 0.85;

/// One delivered file's gates: shape, words (say, never, cta) and frames.
fn file_gates(c: &Cut, plan: &cut::CutPlan, format: &str, f: &Path, rel: &str, said: &mut Vec<(usize, String)>) -> Vec<Gate> {
    let mut out = Vec::new();
    let shape = shape(f);
    match (shape, cut::ratio(format), cut::seconds(c, plan)) {
        (None, ..) => out.push(gate("shape", Some(rel), false, "not an MP4 or MOV with a video track".into())),
        (Some((w, h, secs)), Some((rw, rh)), len) => {
            let mut wrong = Vec::new();
            let (want, got) = (rw as f64 / rh as f64, w as f64 / h.max(1) as f64);
            if (got / want - 1.0).abs() > 0.02 {
                wrong.push(format!("{w}x{h} isn't {format}"));
            }
            if let Some([a, b]) = len
                && (secs + 0.05 < a || secs > b + 0.05)
            {
                wrong.push(format!("{secs:.1} s isn't {a}-{b} s"));
            }
            out.push(gate("shape", Some(rel), wrong.is_empty(), if wrong.is_empty() { format!("{w}x{h}, {secs:.1} s") } else { wrong.join("; ") }));
        }
        _ => {}
    }
    // what it says and shows
    match fresh(f, "words.json", &format!("raea words {rel}")) {
        Err(why) => out.push(gate("words", Some(rel), false, why)),
        Ok(w) => {
            let words = heard(&w);
            let spoken: Vec<&str> = words.iter().map(|(t, ..)| t.as_str()).collect();
            let screen = std::fs::read_to_string(f.with_extension("render.json")).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).map(|m| shown(&m)).unwrap_or_default();
            let on_screen = |s: &str, p: &str, gap| said_at(&s.split(' ').collect::<Vec<_>>(), p, gap).is_some();
            let on = |p: &str| said_at(&spoken, p, SAY_GAP).is_some() || screen.iter().any(|(s, _)| on_screen(s, p, SAY_GAP));
            let missing: Vec<&str> = c.say.iter().filter(|p| p.every != Some(false) && !on(&norm(&p.text))).map(|p| p.text.as_str()).collect();
            said.extend(c.say.iter().enumerate().filter(|(_, p)| on(&norm(&p.text))).map(|(i, _)| (i, rel.to_string())));
            out.push(gate("say", Some(rel), missing.is_empty(), if missing.is_empty() { format!("says all {} of its phrases", c.say.iter().filter(|p| p.every != Some(false)).count()) } else { format!("never says or shows: {}", missing.iter().map(|m| format!("\"{m}\"")).collect::<Vec<_>>().join(", ")) }));
            let banned: Vec<String> = c
                .never
                .iter()
                .filter_map(|p| {
                    let n = norm(&p.text);
                    if let Some(i) = said_at(&spoken, &n, 1) {
                        return Some(format!("\"{}\" heard at {:.1} s (brief: {})", p.text, words[i].1, p.from));
                    }
                    screen.iter().find(|(s, _)| on_screen(s, &n, 1)).map(|(_, at)| format!("\"{}\" on screen at {at:.1} s (brief: {})", p.text, p.from))
                })
                .collect();
            out.push(gate("never", Some(rel), banned.is_empty(), if banned.is_empty() { "says and shows none of the banned phrases".into() } else { banned.join("; ") }));
            if let Some(cta) = &c.cta {
                let within = cta.within.unwrap_or(cut::CTA_WITHIN);
                let end = shape.map(|s| s.2).or_else(|| words.last().map(|w| w.2)).unwrap_or(0.0);
                let last: Vec<&str> = words.iter().filter(|w| w.2 >= end - within).map(|w| w.0.as_str()).collect();
                // the share of a line's words, in order, in `got`; the best of the brief's lines
                let share = |line: &str, got: &[&str]| {
                    let want_s = norm(line);
                    let want: Vec<&str> = want_s.split(' ').collect();
                    let j = joined(got, &want);
                    in_order(&want, &j.iter().map(|(w, _)| w.as_str()).collect::<Vec<_>>())
                };
                let lines: Vec<&str> = std::iter::once(cta.text.as_str()).chain(cta.also.iter().map(String::as_str)).collect();
                let (best, heard_share) = lines.iter().map(|l| (*l, share(l, &last))).fold(("", 0.0), |a, b| if b.1 > a.1 { b } else { a });
                let on_screen = screen.iter().any(|(s, at)| *at >= end - within && lines.iter().any(|l| share(l, &s.split(' ').collect::<Vec<_>>()) >= CTA_SHARE));
                let ok = heard_share >= CTA_SHARE || on_screen;
                let says = match (ok, on_screen) {
                    (true, true) => "ends on the CTA (on screen)".to_string(),
                    (true, false) => format!("ends on the CTA (heard: \"{best}\")"),
                    _ => format!("no CTA in its last {within:.0} s ({:.0}% of the nearest one's words heard there): \"{}\"", heard_share * 100.0, if best.is_empty() { &cta.text } else { best }),
                };
                out.push(gate("cta", Some(rel), ok, says));
            }
        }
    }
    // faces whole, text big enough: the frames check of the file as it is now
    if c.frames != Some(false) {
        match fresh(f, "frames-check.json", &format!("raea frames check {rel}")) {
            Err(why) => out.push(gate("frames", Some(rel), false, why)),
            Ok(r) => {
                let errors: Vec<&str> = r["findings"].as_array().into_iter().flatten().filter(|x| x["level"] == "error").filter_map(|x| x["says"].as_str()).collect();
                out.push(gate("frames", Some(rel), errors.is_empty(), match errors.as_slice() {
                    [] => "no face cut, no text too small".into(),
                    [one] => one.to_string(),
                    many => format!("{} errors, first: {}", many.len(), many[0]),
                }));
            }
        }
    }
    out
}

/// The cut job's gates over its brief folder `dir`.
pub fn gates(c: &Cut, dir: &Path) -> Vec<Gate> {
    let mut out = Vec::new();
    // the quotes: each rule is backed by the brief's own words
    match std::fs::read_to_string(dir.join(&c.brief)) {
        Err(_) => out.push(gate("quotes", None, false, format!("no brief text at {}: pdftotext -layout BRIEF.pdf {}", c.brief, c.brief))),
        Ok(t) => {
            let text = norm(&t);
            let unfound: Vec<String> = cut::quotes(c).into_iter().filter(|(_, from)| !has(&text, &norm(from))).map(|(what, from)| format!("{what} (\"{}\")", from.chars().take(60).collect::<String>())).collect();
            out.push(gate("quotes", None, unfound.is_empty(), if unfound.is_empty() { format!("all {} rules quote the brief", cut::quotes(c).len()) } else { format!("not in the brief's text, word for word: {}", unfound.join("; ")) }));
        }
    }
    let answered = |about: &str| c.open.iter().any(|o| o.about.as_deref() == Some(about) && o.answer.as_deref().is_some_and(|a| !a.trim().is_empty()));
    if let Some(n) = &c.count {
        let have = c.cuts.len() as u32;
        let ok = have >= n.value || answered("count");
        out.push(gate("count", None, ok, if have >= n.value { format!("{have} cuts planned; the brief asks for {}", n.value) } else if ok { format!("{have} of the brief's {} cuts, as answered", n.value) } else { format!("{have} cuts planned; the brief asks for {} (\"{}\"): plan them, or add an `open` item about \"count\" and get it answered", n.value, n.from) }));
    }
    for o in &c.open {
        let ok = o.answer.as_deref().is_some_and(|a| !a.trim().is_empty());
        out.push(gate("open", o.about.as_deref(), ok, if ok { format!("{} -> {}{}", o.question, o.answer.as_deref().unwrap_or(""), o.by.as_deref().map(|b| format!(" ({b})")).unwrap_or_default()) } else { format!("unanswered: {} (brief: \"{}\"): ask at the review, then write the answer and who gave it", o.question, o.from) }));
    }
    let mut said: Vec<(usize, String)> = Vec::new();
    for p in &c.cuts {
        for fmt in cut::formats(c, p) {
            let rel = cut::file(p, fmt);
            let f = dir.join(&rel);
            if f.is_file() {
                out.extend(file_gates(c, p, fmt, &f, &rel, &mut said));
            }
        }
    }
    // a phrase one cut is enough for
    for (i, p) in c.say.iter().enumerate() {
        if p.every != Some(false) {
            continue;
        }
        let by = said.iter().find_map(|(j, f)| (*j == i).then_some(f));
        out.push(gate("say-once", None, by.is_some(), if let Some(f) = by { format!("\"{}\" in {f}", p.text) } else { format!("no cut says or shows \"{}\" (brief: \"{}\")", p.text, p.from) }));
    }
    out
}

#[cfg(test)]
mod tests;
