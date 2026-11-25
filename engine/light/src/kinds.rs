//! What moves in a held frame, told apart (steady.rs measures how much). One number hid four things that have
//! different causes and different fixes (MALL RAT, 11-25: take 1 failed on edges, take 2 on patches):
//!
//! - **settling**: a square whose level climbs or falls steadily through the window. The light is still building (a
//!   world cache lit a ray a cell a frame takes hundreds of frames in a closed court): hold longer before measuring,
//!   and give a render the same lead-in.
//! - **boiling**: a square that wanders slowly about its level (each frame close to the last: lag-1 correlation over
//!   SLOW). Traced light's patches: too few rays behind a denoiser. More paths, a cache that converges.
//! - **jitter**: a square that repeats on a short cycle (the camera's sub-pixel jitter is 8 frames long). An edge the
//!   temporal resolve isn't holding: a silhouette against the sky or a clipped-bright doorway steps a pixel a frame.
//!   Doors, walls and beams look as if they shake.
//! - **flicker**: fast and without a cycle. A firefly, a sparkle in a reflection, two faces fighting for one depth.
//!
//! Each is each LARGE square's deviation in time over its mean, with its straight-line trend taken out first, counted
//! to the kind the square's own series shows, and reported as the rms over every lit square (the others counting as
//! zero, so the four add in squares to the whole), with the worst square and where it is.

use crate::metric::Luma;
use serde_json::{json, Value};

/// A square whose deviation is under this (a fraction of its level) isn't moving: no kind.
const QUIET: f64 = 0.0015;
/// Lag-1 correlation from which a square's movement is slow.
const SLOW: f64 = 0.5;
/// The correlation with itself a cycle later from which a square repeats.
const REPEATS: f64 = 0.6;
/// The longest cycle looked for, frames (the jitter's is 8; a 16-frame sequence is the longest raea uses).
const CYCLE: usize = 16;

/// Below this level (linear, a twentieth of white: about 5 cd/m2 on a 100 cd/m2 display) a change shows less than its
/// ratio to the level says. The eye's contrast threshold is constant in bright light (Weber) and rises as the square
/// root of falling luminance in dim light (de Vries-Rose; Barten's contrast sensitivity model has both). Measured as a
/// plain ratio, MALL RAT's worst squares were its darkest corners (t5 1440, t8 510: 12% and 6% of nearly nothing).
const DIM: f64 = 0.05;

/// The level a square's deviation is a fraction of: its mean in bright light, and sqrt(mean x DIM) below DIM, so a
/// dark square's movement counts by how much of it shows.
pub fn shown_level(mean: f64, lit: f64) -> f64 {
    let m = mean.max(lit);
    if m >= DIM { m } else { (m * DIM).sqrt() }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Still,
    Boiling,
    Jitter,
    Flicker,
}

/// One square's series read: its kind, its deviation over its level (trend out), its trend across the window over its
/// level, and the cycle it repeats on (0: none).
pub fn read(series: &[f64], lit: f64) -> (Kind, f64, f64, usize) {
    let n = series.len();
    let mean = series.iter().sum::<f64>() / n as f64;
    let mid = (n as f64 - 1.0) / 2.0;
    let slope = series.iter().enumerate().map(|(t, v)| (t as f64 - mid) * v).sum::<f64>() / (0..n).map(|t| (t as f64 - mid).powi(2)).sum::<f64>().max(1e-12);
    let r: Vec<f64> = series.iter().enumerate().map(|(t, v)| v - mean - slope * (t as f64 - mid)).collect();
    let var = r.iter().map(|v| v * v).sum::<f64>() / n as f64;
    let level = shown_level(mean, lit);
    let (dev, trend) = (var.sqrt() / level, slope.abs() * (n as f64 - 1.0) / level);
    if dev < QUIET {
        return (Kind::Still, dev, trend, 0);
    }
    // the series against itself p frames later, as a correlation
    let corr = |p: usize| r[p..].iter().zip(&r).map(|(a, b)| a * b).sum::<f64>() / (n - p) as f64 / var.max(1e-18);
    let slow = corr(1) >= SLOW;
    let (cycle, best) = (2..=CYCLE.min(n / 3)).map(|p| (p, corr(p))).fold((0, 0.0), |a, b| if b.1 > a.1 { b } else { a });
    if !slow && best >= REPEATS {
        (Kind::Jitter, dev, trend, cycle)
    } else if slow {
        (Kind::Boiling, dev, trend, 0)
    } else {
        (Kind::Flicker, dev, trend, 0)
    }
}

/// The kinds of a window of held frames: the report's fields, and a picture at the frame's size (red: jitter and
/// flicker, green: boiling, blue: settling; each white at `shown` percent).
pub fn kinds(frames: &[Luma], k: usize, lit: f64, shown: f64) -> (Value, image::RgbImage) {
    let blocks: Vec<Luma> = frames.iter().map(|f| f.blocks(k)).collect();
    let (w, h) = (blocks[0].w, blocks[0].h);
    let pct = |v: f64| (v * 1e4).round() / 100.0;
    let mut sums = [0.0f64; 4];
    let mut counts = [0usize; 4];
    let mut worst = [(0.0f64, 0usize); 4];
    let mut cycles = vec![0usize; CYCLE + 1];
    let (mut trends, mut lit_n) = (Vec::new(), 0usize);
    let mut px = vec![[0u8; 3]; w * h];
    let level = |v: f64| (v * 100.0 / shown * 255.0).min(255.0) as u8;
    for i in 0..w * h {
        let series: Vec<f64> = blocks.iter().map(|b| b.y[i] as f64).collect();
        if series.iter().sum::<f64>() / series.len() as f64 <= lit {
            continue;
        }
        lit_n += 1;
        let (kind, dev, trend, cycle) = read(&series, lit);
        trends.push(trend);
        let j = kind as usize;
        sums[j] += dev * dev;
        counts[j] += 1;
        if dev > worst[j].0 {
            worst[j] = (dev, i);
        }
        cycles[cycle] += usize::from(kind == Kind::Jitter);
        px[i] = match kind {
            Kind::Still => [0, 0, level(trend)],
            Kind::Boiling => [0, level(dev), level(trend)],
            Kind::Jitter | Kind::Flicker => [level(dev), 0, level(trend)],
        };
    }
    trends.sort_by(|a, b| a.total_cmp(b));
    let at = |q: f64| trends.get(((trends.len() as f64 - 1.0) * q).round() as usize).copied().unwrap_or(0.0);
    let one = |j: usize| json!({ "rms": pct((sums[j] / lit_n.max(1) as f64).sqrt()), "squares": counts[j], "worst": pct(worst[j].0), "worst_at": [worst[j].1 % w * k, worst[j].1 / w * k] });
    let cycle = cycles.iter().enumerate().max_by_key(|c| *c.1).filter(|c| *c.1 > 0).map_or(0, |c| c.0);
    let report = json!({
        "boiling": one(Kind::Boiling as usize), "jitter": one(Kind::Jitter as usize), "flicker": one(Kind::Flicker as usize),
        "cycle": cycle, "settling": { "median": pct(at(0.5)), "p95": pct(at(0.95)) },
    });
    let picture = image::RgbImage::from_fn((w * k) as u32, (h * k) as u32, |x, y| image::Rgb(px[y as usize / k * w + x as usize / k]));
    (report, picture)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rnd(seed: &mut u32) -> f64 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (*seed >> 8) as f64 / (1u32 << 24) as f64 * 2.0 - 1.0
    }

    #[test]
    fn each_kind_of_movement_is_told_from_the_others() {
        let mut s = 3u32;
        // a level that climbs 10% and nothing else: still, with its trend
        let climb: Vec<f64> = (0..75).map(|t| 0.3 * (1.0 + 0.1 * t as f64 / 74.0)).collect();
        let (kind, dev, trend, _) = read(&climb, 0.004);
        assert_eq!(kind, Kind::Still);
        assert!(dev < 1e-6 && (trend - 0.0952).abs() < 0.01, "{dev} {trend}");
        // a slow wander: each frame a small step from the last
        let mut v = 0.0;
        let wander: Vec<f64> = (0..75).map(|_| { v = 0.92 * v + 0.01 * rnd(&mut s); 0.3 * (1.0 + v) }).collect();
        assert_eq!(read(&wander, 0.004).0, Kind::Boiling);
        // the jitter's cycle: eight levels, again and again, with a little grain on top
        let phase = [7.5, 1.7, 2.6, 10.4, 1.7, 5.6, 11.3, 1.7];
        let cycle: Vec<f64> = (0..75).map(|t| 0.02 * phase[t % 8] * (1.0 + 0.02 * rnd(&mut s))).collect();
        let (kind, dev, _, p) = read(&cycle, 0.004);
        assert_eq!((kind, p), (Kind::Jitter, 8));
        assert!(dev > 0.5, "{dev}");
        // new every frame: flicker
        let fast: Vec<f64> = (0..75).map(|_| 0.3 * (1.0 + 0.05 * rnd(&mut s))).collect();
        assert_eq!(read(&fast, 0.004).0, Kind::Flicker);
        // and a climb under a wander is still boiling, its trend kept apart
        let both: Vec<f64> = wander.iter().enumerate().map(|(t, v)| v + 0.03 * t as f64 / 74.0).collect();
        let (kind, _, trend, _) = read(&both, 0.004);
        assert_eq!(kind, Kind::Boiling);
        assert!(trend > 0.05, "{trend}");
    }

    #[test]
    fn a_dark_squares_movement_counts_for_less_than_its_ratio() {
        let mut s = 5u32;
        let series = |level: f64, s: &mut u32| -> Vec<f64> { (0..75).map(|_| level * (1.0 + 0.05 * rnd(s))).collect() };
        let (lit, dark) = (read(&series(0.3, &mut s), 0.004).1, read(&series(0.005, &mut s), 0.004).1);
        // the same 5% ratio: counted whole at a lit level, and by sqrt(0.005 / 0.05) at a dark one
        assert!((dark / lit - (0.005f64 / 0.05).sqrt()).abs() < 0.05, "{dark} {lit}");
        assert_eq!(shown_level(0.05, 0.004), 0.05);
    }
}
