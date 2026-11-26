//! A delivered cut's length and frame size, read from its own MP4 or MOV boxes (moov/mvhd and the video track's
//! tkhd), so `raea make status` holds a cut's shape with no decoder: a file re-encoded after its render (a loudness
//! pass) is read as it is now.

use std::path::Path;

/// Seconds, and the video track's width and height as displayed (a rotated phone take reads upright).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    pub seconds: f64,
    pub w: u32,
    pub h: u32,
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn be64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// The boxes in `b`: (kind, body).
fn boxes(b: &[u8]) -> Vec<(&[u8], &[u8])> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 8 <= b.len() {
        let Some(size) = be32(b, at) else { break };
        let kind = &b[at + 4..at + 8];
        let (head, len) = match size {
            1 => (16, be64(b, at + 8).unwrap_or(0) as usize),
            0 => (8, b.len() - at),
            n => (8, n as usize),
        };
        if len < head || at + len > b.len() {
            break;
        }
        out.push((kind, &b[at + head..at + len]));
        at += len;
    }
    out
}

/// The moov box's bytes: read from the file's top level, skipping mdat (the media) without reading it.
fn moov(path: &Path) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let end = f.metadata().ok()?.len();
    let mut at = 0u64;
    while at + 8 <= end {
        let mut head = [0u8; 16];
        f.seek(SeekFrom::Start(at)).ok()?;
        f.read_exact(&mut head[..8]).ok()?;
        let size = u32::from_be_bytes(head[..4].try_into().ok()?) as u64;
        let (skip, len) = match size {
            1 => {
                f.read_exact(&mut head[8..16]).ok()?;
                (16, u64::from_be_bytes(head[8..16].try_into().ok()?))
            }
            0 => (8, end - at),
            n => (8, n),
        };
        if len < skip {
            return None;
        }
        if &head[4..8] == b"moov" {
            let mut body = vec![0u8; (len - skip) as usize];
            f.seek(SeekFrom::Start(at + skip)).ok()?;
            f.read_exact(&mut body).ok()?;
            return Some(body);
        }
        at += len;
    }
    None
}

/// The file's length and its video's displayed size; None when it isn't an MP4 or MOV with a video track.
pub fn shape(path: &Path) -> Option<Shape> {
    let moov = moov(path)?;
    let top = boxes(&moov);
    let mvhd = top.iter().find(|(k, _)| *k == b"mvhd")?.1;
    // version 1 has 64-bit times: timescale at 20, duration at 24; version 0: 12 and 16
    let seconds = if mvhd.first() == Some(&1) { be64(mvhd, 24)? as f64 / be32(mvhd, 20)?.max(1) as f64 } else { be32(mvhd, 16)? as f64 / be32(mvhd, 12)?.max(1) as f64 };
    for (_, trak) in top.iter().filter(|(k, _)| *k == b"trak") {
        let inner = boxes(trak);
        let Some((_, tkhd)) = inner.iter().find(|(k, _)| *k == b"tkhd") else { continue };
        // width and height are 16.16 fixed point at the end of tkhd; the matrix before them says a turn
        let n = tkhd.len();
        let (w, h) = (be32(tkhd, n - 8)? >> 16, be32(tkhd, n - 4)? >> 16);
        if w == 0 || h == 0 {
            continue;
        }
        let a = be32(tkhd, n - 44)? as i32;
        let turned = a == 0;
        return Some(if turned { Shape { seconds, w: h, h: w } } else { Shape { seconds, w, h } });
    }
    None
}
