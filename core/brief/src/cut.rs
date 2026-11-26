//! A cut job's contract (docs/make.feature MK25): `make: cut({...})` on brief.ts, the client's brief made into rules
//! before anything is planned or cut. A brief read as context gets lost (11-25, Shane, on StreetTalk's street interview
//! ads: "you need to make the process of ingesting the brief more direct, or else it's just context that gets lost. It
//! needs to be written as a brief typescript rule that is then enforced"). Every rule quotes the brief line it came
//! from (`from`), and `raea make status` (apps/make cut.rs) finds each quote in the brief's text, so a rule can't be
//! invented and a brief line can't drift; then it holds every delivered file to the rules, from what's on disk.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(optional_fields)]
#[ts(rename = "MakeCut")]
pub struct Cut {
    /// the brief as text, relative to brief.ts (`pdftotext -layout brief.pdf media/brief.txt`): every `from` is found in it
    pub brief: String,
    /// how many cuts the brief asks for; fewer planned is held open until an answered `open` item about "count" says why
    pub count: Option<Quoted<u32>>,
    /// a cut's length when its own `seconds` says none: [least, most]
    pub seconds: Option<[f64; 2]>,
    /// the formats every cut is delivered in when its own `formats` says none: "9:16", "16:9", "1:1", "4:5"
    #[serde(default)]
    #[ts(as = "Option<Vec<String>>", optional)]
    pub formats: Vec<String>,
    #[serde(default)]
    #[ts(as = "Option<Vec<CutPlan>>", optional)]
    pub cuts: Vec<CutPlan>,
    /// what is said or shown: in every cut unless `every: false` (then in one or more)
    #[serde(default)]
    #[ts(as = "Option<Vec<Phrase>>", optional)]
    pub say: Vec<Phrase>,
    /// what no cut says or shows: a banned claim, a word, a name
    #[serde(default)]
    #[ts(as = "Option<Vec<Phrase>>", optional)]
    pub never: Vec<Phrase>,
    /// the line every cut ends on, heard or on screen in its last `within` seconds
    pub cta: Option<Cta>,
    /// the brief's open items ([TO CONFIRM]): each holds the job open until it has an answer
    #[serde(default)]
    #[ts(as = "Option<Vec<Open>>", optional)]
    pub open: Vec<Open>,
    /// every face whole and every text big enough, by the frames check of each file (true when left out)
    pub frames: Option<bool>,
}

/// A value with the brief's words for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(rename = "MakeQuoted")]
pub struct Quoted<T: TS> {
    pub value: T,
    /// the brief's own words, as they are in its text
    pub from: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(optional_fields)]
#[ts(rename = "MakeCutPlan")]
pub struct CutPlan {
    /// [a-z0-9_], unique: each format lands as out/cuts/<id>_<W>x<H>.mp4 ("a1_s1_9x16.mp4")
    pub id: String,
    pub angle: Option<String>,
    /// solo or mashup
    pub kind: Option<String>,
    /// one of the brief's hook types ("Direct Problem", "Reaction")
    pub hook: Option<String>,
    /// captions, hook (a text hook) or none
    pub text: Option<String>,
    #[serde(default)]
    #[ts(as = "Option<Vec<String>>", optional)]
    pub formats: Vec<String>,
    pub seconds: Option<[f64; 2]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(optional_fields)]
#[ts(rename = "MakePhrase")]
pub struct Phrase {
    /// as it's said or shown; compared in lower case, by letters and digits ("24/7" and "24-7" are the same)
    pub text: String,
    pub from: String,
    /// `say` only: false when one cut saying it is enough
    pub every: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(optional_fields)]
#[ts(rename = "MakeCta")]
pub struct Cta {
    pub text: String,
    pub from: String,
    /// how near the end it must land, in seconds (8 when left out)
    pub within: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(optional_fields)]
#[ts(rename = "MakeOpen")]
pub struct Open {
    pub question: String,
    pub from: String,
    /// what it decides: "count", "cta", "offer", a cut's id...
    pub about: Option<String>,
    /// the person's answer, as they gave it, and who gave it: until then the job can't be done
    pub answer: Option<String>,
    pub by: Option<String>,
}

/// How near the end the CTA lands when the contract says nothing.
pub const CTA_WITHIN: f64 = 8.0;

/// A format's frame shape, "9:16" -> (9, 16).
pub fn ratio(f: &str) -> Option<(u32, u32)> {
    let (w, h) = f.split_once(':')?;
    Some((w.trim().parse().ok().filter(|&w| w > 0)?, h.trim().parse().ok().filter(|&h| h > 0)?))
}

/// A cut's formats and its length: its own, else the contract's.
pub fn formats<'a>(c: &'a Cut, p: &'a CutPlan) -> &'a [String] {
    if p.formats.is_empty() { &c.formats } else { &p.formats }
}
pub fn seconds(c: &Cut, p: &CutPlan) -> Option<[f64; 2]> {
    p.seconds.or(c.seconds)
}

/// Where a cut's format lands, relative to the brief: out/cuts/a1_s1_9x16.mp4.
pub fn file(p: &CutPlan, format: &str) -> String {
    format!("out/cuts/{}_{}.mp4", p.id, format.replace(':', "x"))
}

/// Every file the contract asks for.
pub fn deliverables(c: &Cut) -> Vec<String> {
    c.cuts.iter().flat_map(|p| formats(c, p).iter().map(move |f| file(p, f))).collect()
}

/// Every `from` in the contract, with what it backs: the quotes `raea make status` finds in the brief's text.
pub fn quotes(c: &Cut) -> Vec<(String, &str)> {
    let mut q: Vec<(String, &str)> = Vec::new();
    if let Some(n) = &c.count {
        q.push(("count".into(), &n.from));
    }
    q.extend(c.say.iter().map(|p| (format!("say \"{}\"", p.text), p.from.as_str())));
    q.extend(c.never.iter().map(|p| (format!("never \"{}\"", p.text), p.from.as_str())));
    if let Some(cta) = &c.cta {
        q.push(("cta".into(), &cta.from));
    }
    q.extend(c.open.iter().map(|o| (format!("open \"{}\"", o.question), o.from.as_str())));
    q
}

/// What the contract lacks before it can be cut against, each with what to do.
pub fn doctor(c: &Cut) -> Vec<String> {
    let mut out = Vec::new();
    if c.brief.trim().is_empty() {
        out.push("no `brief`: the brief as text beside brief.ts (pdftotext -layout BRIEF.pdf media/brief.txt), which every `from` quotes".into());
    }
    if c.cuts.is_empty() {
        out.push("no cuts: list every cut the brief asks for, with its id, angle, hook and formats".into());
    }
    if c.count.is_none() {
        out.push("no `count`: how many cuts the brief asks for, with its words".into());
    }
    if c.cta.is_none() {
        out.push("no `cta`: the line the brief says every cut ends on (or an `open` item asking for it)".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for p in &c.cuts {
        if !seen.insert(p.id.as_str()) {
            out.push(format!("cut `{}` is listed twice", p.id));
        }
        if p.id.is_empty() || !p.id.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_') {
            out.push(format!("cut `{}`: ids are [a-z0-9_], the file it lands as", p.id));
        }
        let fs = formats(c, p);
        if fs.is_empty() {
            out.push(format!("cut `{}` has no formats: give it `formats`, or the contract's", p.id));
        }
        for f in fs.iter().filter(|f| ratio(f).is_none()) {
            out.push(format!("cut `{}`: format `{f}` isn't W:H (\"9:16\", \"16:9\", \"1:1\", \"4:5\")", p.id));
        }
        match seconds(c, p) {
            None => out.push(format!("cut `{}` has no length: `seconds: [least, most]`, its own or the contract's", p.id)),
            Some([a, b]) if !(a > 0.0 && a <= b) => out.push(format!("cut `{}`: seconds [{a}, {b}] isn't [least, most]", p.id)),
            _ => {}
        }
        if let Some(t) = &p.text
            && !matches!(t.as_str(), "captions" | "hook" | "none")
        {
            out.push(format!("cut `{}`: text is captions, hook or none, not `{t}`", p.id));
        }
    }
    for (what, from) in quotes(c) {
        if from.trim().is_empty() {
            out.push(format!("{what} has no `from`: the brief's own words for it"));
        }
    }
    for p in c.say.iter().chain(&c.never).filter(|p| norm(&p.text).is_empty()) {
        out.push(format!("phrase `{}` has no letters or digits to find", p.text));
    }
    out
}

/// Text as it's compared: lower case, letters and digits, one space between words ("24/7" -> "24 7").
pub fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut gap = false;
    for ch in s.chars().flat_map(char::to_lowercase) {
        // a curly apostrophe or a plain one joins its word ("it's" is one word, "its" is another)
        if ch == '\'' || ch == '\u{2019}' {
            continue;
        }
        if ch.is_alphanumeric() {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.push(ch);
        } else {
            gap = true;
        }
    }
    out
}
