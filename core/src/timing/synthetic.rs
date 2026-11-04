//! A synthetic encode for tests: cuts at irregular gaps, and the keyframes an encoder that forces
//! one when a GOP runs long adds between them. Irregular, so that no shift folds the cuts onto
//! themselves the way a keyframe every 2 s would.

use super::Video;
use crate::text::Event;

/// 23.976 fps, as mkvmerge stores it.
pub const FRAME_NS: u64 = 41_708_333;

/// Frame `n`'s time in ms, as a Cue or a subtitle event stores it.
pub fn ms(n: i64) -> i64 {
    let ns = i128::from(n) * i128::from(FRAME_NS);
    i64::try_from((ns + 500_000).div_euclid(1_000_000)).expect("a test's frames fit")
}

/// `count` cut frames from frame 240, at gaps of 8 to 119 frames, the same each run.
pub fn cuts(count: usize) -> Vec<i64> {
    cuts_at_gaps(count, 8, 112)
}

/// `count` cut frames from frame 240, at gaps of 30 to 45 frames, the same each run: every shot
/// under two seconds, so a time a second after its cut is nearer the next one.
pub fn short_cuts(count: usize) -> Vec<i64> {
    cuts_at_gaps(count, 30, 16)
}

/// `count` cut frames from frame 240, at gaps of `least` to `least + spread - 1` frames.
fn cuts_at_gaps(count: usize, least: i64, spread: u64) -> Vec<i64> {
    let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
    let mut at = 240;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(at);
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        at += least + i64::try_from((seed >> 33) % spread).expect("a test's gaps fit");
    }
    out
}

/// The keyframe frames of an encode with one on each cut and, after a cut, a forced one `first`
/// frames on, then every `then[i % then.len()]` frames until the next cut.
pub fn keyframes(cuts: &[i64], first: i64, then: &[i64]) -> Vec<i64> {
    let mut out = Vec::new();
    for (i, &cut) in cuts.iter().enumerate() {
        out.push(cut);
        let next = cuts.get(i + 1).copied().unwrap_or(cut + 120);
        let mut step = then.iter().cycle();
        let mut k = cut + first;
        while k < next {
            out.push(k);
            k += step.next().expect("then is not empty");
        }
    }
    out
}

/// Keyframe frames as Cue times, in ns.
pub fn keyframes_ns(frames: &[i64]) -> Vec<u64> {
    frames
        .iter()
        .map(|&n| u64::try_from(ms(n)).expect("after 0") * 1_000_000)
        .collect()
}

/// An encode forcing a keyframe 26 frames after a cut and every 24 after that.
pub fn yuru(cuts: &[i64]) -> Video {
    Video::new(FRAME_NS, keyframes_ns(&keyframes(cuts, 26, &[24])))
}

pub fn event(style: &str, start_ms: i64, end_ms: i64, text: &str) -> Event {
    Event {
        track: 0,
        start_ms,
        end_ms,
        layer: "0".into(),
        style: style.into(),
        text: text.into(),
        pos: None,
        karaoke: false,
        raw: format!("0,{style},,0,0,0,,{text}"),
    }
}

/// Dialogue timed to `cuts`, `late` frames late (early when negative).
pub fn track(cuts: &[i64], late: i64) -> Vec<Event> {
    track_by(cuts, |_| late)
}

/// Dialogue timed to `cuts`, each shot's lines `late(cut)` frames late: a line from the cut to the
/// middle of the shot and one from just after the middle to the next cut, so half its times are
/// on cuts and half are not. Each line's text is its own.
pub fn track_by(cuts: &[i64], late: impl Fn(i64) -> i64) -> Vec<Event> {
    cuts.windows(2)
        .enumerate()
        .flat_map(|(i, w)| {
            let (cut, next) = (w[0], w[1]);
            let l = late(cut);
            let mid = cut + (next - cut) / 2;
            [
                event("D", ms(cut + l), ms(mid + l), &format!("Line {i}a")),
                event("D", ms(mid + 3 + l), ms(next + l), &format!("Line {i}b")),
            ]
        })
        .collect()
}
