//! `raea light steady EDIT.tsx --frame N -o DIR`: whether the light holds still. Frame N is
//! drawn again and again (`raea render --hold`), so the scene and the camera don't move, and whatever still changes on
//! screen is the renderer's own. Two things change: grain, a pixel or two wide and new every frame, which a denoiser
//! and the eye average away; and blotches, patches tens of pixels wide that drift over lit floors and reflections for
//! a quarter of a second or more, which nothing averages away. Solari's world cache makes them when its cells are
//! lit by too few rays (MALL RAT, 11-24: "this noisy splotchy thing in the lights and reflections").
//!
//! The two are told apart by size. Each SMALL x SMALL and each LARGE x LARGE square's mean luminance is followed over
//! the held frames, once the light has built: its deviation in time over its mean. Grain's falls by the square's side
//! (LARGE / SMALL = 4 times less in the large squares); a blotch as wide as the large square keeps its deviation.
//! `blotch` is the large squares' deviation (rms over the lit ones, percent), `worst` the worst TILE's, and `spread`
//! the large squares' deviation over the small squares' (0.25 is grain alone, toward 1 is blotches). A run over
//! `--max` fails, as does a light whose level drifts (`drift`, stops between the first and the last quarter of the
//! measured frames: a cache gaining or losing energy the longer it runs), and DIR/steady.png shows where: each large square by its deviation, white at MAX_SHOWN percent.

use crate::metric::{self, Luma};
use raea_runtime_bundle::{flag, R};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::USAGE;

const SMALL: usize = 8;
const LARGE: usize = 32;
/// The large squares a side of the tile the worst is found among.
const TILE: usize = 4;
/// A square darker than this (linear) isn't lit enough for its deviation to show.
const LIT: f64 = 0.004;
/// The deviation steady.png draws as white, percent.
const MAX_SHOWN: f64 = 6.0;
/// What a run may show before it fails, percent, unless `--max` says: a held raster frame reads 0, Solari with a
/// built cache under 1.
const MAX: f64 = 1.5;
/// How far a frame's level at the end of a run may be from the frame drawn cold, in stops (`--run`).
const MAX_ENERGY: f64 = 0.25;
/// How far the held frame's level may move between the first and the last quarter of the measured frames, in stops.
const MAX_DRIFT: f64 = 0.05;
/// The frames measured: the last WINDOW of a hold (or its second half, if that's shorter). Fixed, because a slow
/// wander reads larger over a longer window, so two holds of different lengths wouldn't compare.
const WINDOW: usize = 75;
/// What the worst square that moves with the jitter, or flickers, may show, percent: an edge stepping a pixel a frame
/// reads 20 to 70; a resolved one under 2.
const MAX_FAST: f64 = 5.0;
/// The trend across the window a hold's squares may still have (the 95th percentile), percent: over it the light was
/// still building when it was measured, and what reads as boiling is partly that.
const MAX_SETTLING: f64 = 5.0;

/// Each square's mean and deviation in time over `frames`, side `k`: (w, h, [(mean, deviation / mean)]).
fn squares(frames: &[Luma], k: usize) -> (usize, usize, Vec<(f64, f64)>) {
    let first = frames[0].blocks(k);
    let mut sums = vec![(0.0f64, 0.0f64); first.y.len()];
    for f in frames {
        for (s, &v) in sums.iter_mut().zip(&f.blocks(k).y) {
            s.0 += v as f64;
            s.1 += (v as f64) * (v as f64);
        }
    }
    let n = frames.len() as f64;
    let out = sums.into_iter().map(|(a, b)| { let m = a / n; (m, (b / n - m * m).max(0.0).sqrt() / m.max(LIT)) }).collect();
    (first.w, first.h, out)
}

fn rms(v: impl Iterator<Item = f64>) -> f64 {
    let (s, n) = v.fold((0.0, 0usize), |(s, n), x| (s + x * x, n + 1));
    if n == 0 { 0.0 } else { (s / n as f64).sqrt() }
}

/// The measure of held frames (the module's doc), and steady.png's picture: the large squares' deviation.
pub fn measure(frames: &[Luma]) -> R<(Value, image::GrayImage, image::RgbImage)> {
    if frames.len() < 8 {
        return Err(format!("{} held frames: 8 at least say whether the light holds still", frames.len()));
    }
    let (_, _, small) = squares(frames, SMALL);
    let (w, h, large) = squares(frames, LARGE);
    let lit = |s: &&(f64, f64)| s.0 > LIT;
    let (blotch, grain) = (rms(large.iter().filter(lit).map(|s| s.1)), rms(small.iter().filter(lit).map(|s| s.1)));
    let mut worst = (0.0f64, 0usize, 0usize);
    for ty in 0..h.div_ceil(TILE) {
        for tx in 0..w.div_ceil(TILE) {
            let cells = (0..TILE * TILE).filter_map(|i| large.get((ty * TILE + i / TILE) * w + tx * TILE + i % TILE).filter(|_| tx * TILE + i % TILE < w)).filter(lit);
            let v = rms(cells.map(|s| s.1));
            if v > worst.0 {
                worst = (v, tx * TILE * LARGE, ty * TILE * LARGE);
            }
        }
    }
    // at the frame's size, so it lays over the frame
    let map = image::GrayImage::from_fn((w * LARGE) as u32, (h * LARGE) as u32, |x, y| {
        let s = large[y as usize / LARGE * w + x as usize / LARGE];
        image::Luma([if s.0 > LIT { (s.1 * 100.0 / MAX_SHOWN * 255.0).min(255.0) as u8 } else { 0 }])
    });
    let pct = |v: f64| (v * 1e4).round() / 100.0;
    // the light's level over the held frames: a cache gaining or losing energy is a ramp, not patches
    let level = |f: &[Luma]| f.iter().map(|f| f.y.iter().map(|&v| v as f64).sum::<f64>() / f.y.len().max(1) as f64).sum::<f64>() / f.len() as f64;
    let q = (frames.len() / 4).max(1);
    let drift = (level(&frames[frames.len() - q..]).max(1e-6) / level(&frames[..q]).max(1e-6)).log2();
    let (kinds, kinds_map) = crate::kinds::kinds(frames, LARGE, LIT, MAX_SHOWN);
    let report = json!({
        "kinds": kinds,
        "drift": (drift * 1e3).round() / 1e3,
        "frames": frames.len(), "blotch": pct(blotch), "grain": pct(grain), "spread": ((blotch / grain.max(1e-9)) * 100.0).round() / 100.0,
        "worst": pct(worst.0), "worst_at": [worst.1, worst.2], "lit": ((large.iter().filter(lit).count() as f64 / large.len().max(1) as f64) * 100.0).round() / 100.0,
    });
    Ok((report, map, kinds_map))
}

/// The PNG frames of a folder, in order.
fn held(dir: &Path) -> R<Vec<PathBuf>> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "png") && p.file_name().is_some_and(|n| n != "steady.png")).collect();
    v.sort();
    Ok(v)
}

/// Frame `frame` of a composition held for `n` frames into `out` (`raea render --hold`), run in `dir` under `env`.
pub fn hold(dir: &Path, file: &str, composition: Option<&str>, frame: &str, n: usize, env: &[(&str, String)], out: &Path) -> R<()> {
    draw(dir, file, composition, &["--hold", &format!("{frame}:{n}")], env, out)
}

/// A composition's frames drawn into `out` as `how` says (`--hold N:M`, `--at A,B,...`), run in `dir` under `env`.
fn draw(dir: &Path, file: &str, composition: Option<&str>, how: &[&str], env: &[(&str, String)], out: &Path) -> R<()> {
    let _ = std::fs::remove_dir_all(out);
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut cmd = std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    cmd.current_dir(dir).envs(env.iter().map(|(k, v)| (*k, v)));
    cmd.arg("render").arg(file).args(how).arg("-o").arg(format!("{}/", out.display()));
    if let Some(c) = composition {
        cmd.args(["--composition", c]);
    }
    let run = cmd.args(["--despite", "clips", "raea light steady measures light, not placement"]).output().map_err(|e| e.to_string())?;
    // what the render said, kept beside its frames: a refused cache load or a switch that didn't take is read there
    let err = String::from_utf8_lossy(&run.stderr);
    let _ = std::fs::write(out.join("render.log"), format!("{}{err}", String::from_utf8_lossy(&run.stdout)));
    if !run.status.success() {
        return Err(format!("the held render failed: {}", err.lines().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" | ")));
    }
    unshaded(out)
}

/// A held run whose shaders didn't compile drew without them (a Solari pipeline that failed leaves the frame to
/// whatever is left), and its light says nothing about the path it was held for: refused, with the shader's error.
fn unshaded(out: &Path) -> R<()> {
    let log = std::fs::read_to_string(out.join("render.log")).unwrap_or_default();
    let Some(at) = log.find("failed to process shader") else { return Ok(()) };
    let why: Vec<&str> = log[at..].lines().skip(1).map(str::trim).filter(|l| !l.is_empty()).take(3).collect();
    Err(format!("a shader of this render didn't compile, so its frames aren't the light it was held for: {} ({}/render.log)", why.join(" "), out.display()))
}

/// The held frames of `dir` measured: DIR/steady.png and steady.json written, and the report, its "steady" saying
/// whether the light held (`max`: the blotch a run may show, percent; MAX when None).
pub fn report(dir: &Path, max: Option<f64>) -> R<Value> {
    unshaded(dir)?;
    let max = max.unwrap_or(MAX);
    // the second half: the first is the light building from nothing, which a render's lead-in covers
    let files = held(dir)?;
    let frames: Vec<Luma> = files[files.len() - WINDOW.min(files.len() - files.len() / 2)..].iter().map(|p| metric::load(p)).collect::<R<_>>()?;
    let (mut report, map, kinds_map) = measure(&frames)?;
    let png = dir.join("steady.png");
    map.save(&png).map_err(|e| format!("{}: {e}", png.display()))?;
    let kinds_png = dir.join("steady-kinds.png");
    kinds_map.save(&kinds_png).map_err(|e| format!("{}: {e}", kinds_png.display()))?;
    let get = |k: &str| report[k].as_f64().unwrap_or(0.0);
    let kind = |k: &str, f: &str| report["kinds"][k][f].as_f64().unwrap_or(0.0);
    // each kind against its own limit, and what failed named: the fixes differ
    let mut failed: Vec<&str> = Vec::new();
    if kind("boiling", "rms") > max || kind("boiling", "worst") > max * 3.0 { failed.push("boiling"); }
    if kind("jitter", "worst") > MAX_FAST { failed.push("jitter"); }
    if kind("flicker", "worst") > MAX_FAST { failed.push("flicker"); }
    if get("drift").abs() > MAX_DRIFT { failed.push("drift"); }
    let ok = failed.is_empty();
    let unsettled = kind("settling", "p95") > MAX_SETTLING;
    let failed: Vec<String> = failed.into_iter().map(String::from).collect();
    report["failed"] = json!(failed);
    report["unsettled"] = json!(unsettled);
    report["kinds_picture"] = json!(kinds_png);
    report["max"] = json!(max);
    report["max_drift"] = json!(MAX_DRIFT);
    report["steady"] = json!(ok);
    report["picture"] = json!(png);
    std::fs::write(dir.join("steady.json"), serde_json::to_string_pretty(&report).unwrap_or_default()).map_err(|e| e.to_string())?;
    Ok(report)
}

/// A report as one line.
pub fn line(r: &Value) -> String {
    let k = &r["kinds"];
    let one = |name: &str| format!("{name} {}% (worst {}% at {},{})", k[name]["rms"], k[name]["worst"], k[name]["worst_at"][0], k[name]["worst_at"][1]);
    format!(
        "{}{}, {}{}, {}, settling {}% (p95 {}%){}, drift {} stops (max {}); all movement {}% (blotch), grain {}%, spread {} over {} held frames; {}",
        one("boiling"), format_args!(" (max {})", r["max"]), one("jitter"), if k["cycle"].as_u64().unwrap_or(0) > 0 { format!(" on a {}-frame cycle", k["cycle"]) } else { String::new() }, one("flicker"),
        k["settling"]["median"], k["settling"]["p95"], if r["unsettled"] == json!(true) { " STILL SETTLING: hold longer (--frames) before reading the rest" } else { "" },
        r["drift"], r["max_drift"], r["blotch"], r["grain"], r["spread"], r["frames"], r["kinds_picture"].as_str().unwrap_or("")
    )
}

/// What a held frame that moves is told, by what moved.
pub fn why(r: &Value) -> String {
    let failed: Vec<&str> = r["failed"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let mut says: Vec<String> = Vec::new();
    if failed.contains(&"boiling") || failed.contains(&"drift") { says.push(WHY.to_string()); }
    if failed.contains(&"jitter") { says.push("Jitter: squares that repeat on a short cycle are edges stepping with the camera's sub-pixel jitter, which the temporal resolve should have held: a silhouette against the sky or a clipped-bright opening. Hold it with RAEA_SOLARI_AA=0 to confirm (it goes), then keep the bright side under the resolve's range (exposure, the sky's level) or report it".to_string()); }
    if failed.contains(&"flicker") { says.push("Flicker: fast and without a cycle is a firefly, a sparkle in a reflection or two faces at one depth: look at the worst square in steady-kinds.png (red), and run raea scene lint for faces in one plane".to_string()); }
    says.join(" ")
}

/// What a light that boils is told (boiling: the trade's word for low-frequency patches that crawl on a held frame).
pub const WHY: &str = "Boiling: patches this size are light built over frames that hasn't settled. On Solari, paths (SolariLighting paths={4}) first, then the world cache, ReSTIR's reuse or the denoiser. Hold the frame with each switched off in turn (cache.steady or a baked cache, RAEA_SOLARI_DENOISE=0, RAEA_SOLARI_PSR=0) to see which; DIR/render.log holds what each render said";

/// The check a held frame's findings carry in the project's record, which `raea check` reads for a traced composition.
pub const CHECK: &str = "light-steady";

/// A held frame's report as findings, one a kind that failed: about the composition, on the frame held.
pub fn findings(r: &Value, composition: Option<&str>, frame: i64) -> Vec<raea_core_facts::Finding> {
    use raea_core_facts::{Finding, Level};
    let k = &r["kinds"];
    let num = |v: &Value| v.as_f64().unwrap_or(0.0);
    let at = |name: &str| format!("{},{}", k[name]["worst_at"][0], k[name]["worst_at"][1]);
    let mut out = Vec::new();
    let mut push = |kind: &str, level: Level, says: String, value: f64, limit: f64, fix: &str| {
        let mut f = Finding::new(CHECK, level, says, "raea light steady").frames(frame, frame + 1).measure(kind, value, "%", Some(limit)).fix(fix);
        if let Some(c) = composition {
            f = f.about(c, None);
        }
        out.push(f);
    };
    let failed: Vec<&str> = r["failed"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let max = num(&r["max"]);
    if failed.contains(&"boiling") {
        push("boiling", Level::Error, format!("the light boils on frame {frame} held: {}% (worst square {}% at {}; max {max}, worst {})", k["boiling"]["rms"], k["boiling"]["worst"], at("boiling"), max * 3.0), num(&k["boiling"]["worst"]), max * 3.0, WHY);
    }
    if failed.contains(&"jitter") {
        push("jitter", Level::Error, format!("edges step with the camera's jitter on frame {frame} held: worst square {}% at {} on a {}-frame cycle (max {MAX_FAST})", k["jitter"]["worst"], at("jitter"), k["cycle"]), num(&k["jitter"]["worst"]), MAX_FAST, "an edge against the sky, a forward mesh or an emitter left out of the denoiser's history: SolariLighting late and emissive=\"taa\" (the Solari guide)");
    }
    if failed.contains(&"flicker") {
        push("flicker", Level::Error, format!("a square flickers on frame {frame} held: {}% at {} (max {MAX_FAST})", k["flicker"]["worst"], at("flicker")), num(&k["flicker"]["worst"]), MAX_FAST, "a firefly, a sparkle in a reflection or two faces in one plane: look at the square in steady-kinds.png, and run raea scene lint for faces in one plane");
    }
    if failed.contains(&"drift") {
        push("drift", Level::Error, format!("the held frame's light drifts {} stops (max {MAX_DRIFT})", r["drift"]), num(&r["drift"]).abs(), MAX_DRIFT, WHY);
    }
    if failed.is_empty() {
        push("steady", Level::Note, format!("the light holds still on frame {frame} held: {}", line(r)), num(&r["blotch"]), max, "");
    }
    if r["unsettled"] == json!(true) {
        push("settling", Level::Note, format!("the light was still building on frame {frame} held (settling p95 {}%): hold longer, or give the shot a lead-in", k["settling"]["p95"]), num(&k["settling"]["p95"]), MAX_SETTLING, "a still lead-in before the shot, as long as the hold took to settle");
    }
    out
}

/// A held frame's findings kept in `file`'s project's record (nothing outside a project).
fn keep(file: &str, out: &Path, r: &Value, composition: Option<&str>, frame: i64) {
    let stamp = raea_io_trace::utc(std::time::SystemTime::now()).1;
    raea_core_facts::record::keep("raea light steady", Path::new(file), Some(out), &findings(r, composition, frame), Vec::new(), &stamp);
}

pub fn steady(args: &[String]) -> R<String> {
    let first = args.first().ok_or(format!("usage: {USAGE}"))?;
    let max = flag(args, "--max").map(|m| m.parse::<f64>().map_err(|_| format!("--max takes a percent, not {m}"))).transpose()?;
    let dir = if Path::new(first).is_dir() {
        PathBuf::from(first)
    } else {
        let frame = flag(args, "--frame").ok_or(format!("usage: {USAGE}"))?;
        let n: usize = flag(args, "--frames").map(|n| n.parse().map_err(|_| format!("--frames takes a count, not {n}"))).transpose()?.unwrap_or(96);
        let out = PathBuf::from(flag(args, "-o").ok_or(format!("usage: {USAGE}"))?).join("held");
        hold(Path::new("."), first, flag(args, "--composition").as_deref(), &frame, n, &[], &out)?;
        out
    };
    let mut r = report(&dir, max)?;
    let held_frame = flag(args, "--frame").and_then(|f| f.parse::<i64>().ok());
    if let (false, Some(f)) = (Path::new(first).is_dir(), held_frame) {
        keep(first, dir.parent().unwrap_or(&dir), &r, flag(args, "--composition").as_deref(), f);
    }
    // --run M: the frame cold against the same frame at the end of M frames played up to it. A cache that gains or
    // loses energy over minutes (MALL RAT's court, 11-24: 7 times brighter inside the long render) shows here and not
    // in a hold
    if let (Some(m), Some(frame), false) = (flag(args, "--run"), flag(args, "--frame"), Path::new(first).is_dir()) {
        let (m, n): (i64, i64) = (m.parse().map_err(|_| format!("--run takes a count of frames, not {m}"))?, frame.parse().map_err(|_| format!("--frame takes a frame, not {frame}"))?);
        let base = dir.parent().unwrap_or(&dir).to_path_buf();
        let level = |name: &str, at: Vec<i64>| -> R<f64> {
            let out = base.join(name);
            let at: Vec<String> = at.iter().map(i64::to_string).collect();
            draw(Path::new("."), first, flag(args, "--composition").as_deref(), &["--at", &at.join(",")], &[], &out)?;
            let last = held(&out)?.pop().ok_or(format!("{}: no frame drawn", out.display()))?;
            let f = metric::load(&last)?;
            Ok(f.y.iter().map(|&v| v as f64).sum::<f64>() / f.y.len().max(1) as f64)
        };
        let energy = (level("run", ((n - m).max(0)..=n).collect())?.max(1e-6) / level("cold", vec![n])?.max(1e-6)).log2();
        r["energy"] = json!((energy * 1e3).round() / 1e3);
        r["max_energy"] = json!(MAX_ENERGY);
        if energy.abs() > MAX_ENERGY {
            r["steady"] = json!(false);
        }
        std::fs::write(dir.join("steady.json"), serde_json::to_string_pretty(&r).unwrap_or_default()).map_err(|e| e.to_string())?;
        let said = format!("energy {} stops after {m} frames against the frame cold (max {MAX_ENERGY}), {}", r["energy"], line(&r));
        return if r["steady"] == json!(true) { Ok(format!("STEADY  {said}")) } else { Err(format!("the held frame moves ({}): {said}. {}", r["failed"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default(), why(&r))) };
    }
    if r["steady"] == json!(true) { Ok(format!("STEADY  {}", line(&r))) } else { Err(format!("the held frame moves ({}): {}. {}", r["failed"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default(), line(&r), why(&r))) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames of a flat grey, each pixel moved by `grain` (new every frame and pixel) and each PATCH x PATCH patch by
    /// `blotch` (new every frame, the same across the patch).
    fn frames(grain: f32, blotch: f32) -> Vec<Luma> {
        const PATCH: usize = 40;
        let (w, h) = (320, 240);
        let mut seed = 7u32;
        let mut rnd = move || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        };
        (0..24)
            .map(|_| {
                let patches: Vec<f32> = (0..(w / PATCH + 1) * (h / PATCH + 1)).map(|_| rnd() * blotch).collect();
                Luma { w, h, y: (0..w * h).map(|i| 0.3 * (1.0 + rnd() * grain + patches[(i / w / PATCH) * (w / PATCH + 1) + (i % w) / PATCH])).collect() }
            })
            .collect()
    }

    #[test]
    fn grain_is_steady_and_blotches_are_not() {
        let still = measure(&frames(0.0, 0.0)).unwrap().0;
        assert_eq!(still["kinds"]["boiling"]["rms"], 0.0);
        assert_eq!(still["blotch"], 0.0);
        assert_eq!(still["drift"], 0.0);
        // a light that doubles over the held frames: about 0.8 of a stop between the first and the last quarter
        let mut rising = frames(0.0, 0.0);
        let n = rising.len() as f32;
        rising.iter_mut().enumerate().for_each(|(t, f)| f.y.iter_mut().for_each(|v| *v *= 1.0 + t as f32 / n));
        assert!(measure(&rising).unwrap().0["drift"].as_f64().unwrap() > 0.5);
        // heavy grain, 30% a pixel: under 1% across a 32 px square, and a spread near 0.25
        let grain = measure(&frames(0.3, 0.0)).unwrap().0;
        assert!(grain["blotch"].as_f64().unwrap() < 1.0, "{grain}");
        assert!(grain["spread"].as_f64().unwrap() < 0.35, "{grain}");
        // patches moving 5%: the large squares keep it
        let blotch = measure(&frames(0.05, 0.05)).unwrap().0;
        assert!(blotch["blotch"].as_f64().unwrap() > 2.0, "{blotch}");
        assert!(blotch["spread"].as_f64().unwrap() > 0.6, "{blotch}");
    }

    /// A run whose shaders failed isn't measured: its frames are another light's.
    #[test]
    fn a_run_that_drew_without_its_shaders_is_refused() {
        let dir = std::env::temp_dir().join(format!("raea-steady-unshaded-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("render.log"), "ERROR bevy_render::render_resource::pipeline_cache: failed to process shader error:\n  use of a reserved word `target`\n").unwrap();
        let e = report(&dir, None).unwrap_err();
        assert!(e.contains("didn't compile") && e.contains("reserved word"), "{e}");
        std::fs::write(dir.join("render.log"), "APPLIED  96 frames\n").unwrap();
        assert!(unshaded(&dir).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
