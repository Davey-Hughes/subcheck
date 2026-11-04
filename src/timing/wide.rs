//! The wide scan: how many of a track's times land exactly on a cut once moved back by each whole
//! number of frames, up to REACH_MS either way. [`super::analyse`] centres its ±MAX_OFFSET
//! analysis on the shift the scan decides, leaves out of it the windows the scan puts far from
//! that shift, and calls a track unclear when the scan's evidence points two ways, unless the
//! track has too few times for a far peak to doubt the rest.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashSet};

use super::{
    MAX_OFFSET, MIN_AT_OFFSET, OFFSETS, Video, frames_from, is_timed, window_of, window_start,
};
use crate::text::Event;

/// How far either way a shift is looked for.
pub const REACH_MS: i64 = 10_000;

/// The most frames either way a shift is looked for: REACH_MS at 240 fps. A shorter frame is no
/// video's, and reaching REACH_MS in it would have the scan pair every time with every cue.
const MAX_REACH: i64 = 2_400;

/// How many times hit each shift, in frames.
pub type Hits = BTreeMap<i64, usize>;

/// REACH_MS in whole frames of `video`, within MAX_OFFSET and MAX_REACH.
pub fn reach(video: &Video) -> i64 {
    let frame = i128::from(video.frame_ns.max(1));
    let frames = (i128::from(REACH_MS) * 1_000_000 + frame / 2) / frame;
    i64::try_from(frames)
        .unwrap_or(i64::MAX)
        .clamp(MAX_OFFSET, MAX_REACH)
}

/// The shifts a time at `ms` hits, ascending: each `s` within `reach` that puts it on the frame `s`
/// after a cut.
pub fn shifts_of(video: &Video, ms: i64, reach: i64) -> Vec<i64> {
    let x = i128::from(ms) * 1_000_000;
    let span = (i128::from(reach) + 1) * i128::from(video.frame_ns);
    let lo = video.cuts_ns.partition_point(|&c| i128::from(c) < x - span);
    let hi = video
        .cuts_ns
        .partition_point(|&c| i128::from(c) <= x + span);
    let mut out: Vec<i64> = video.cuts_ns[lo..hi]
        .iter()
        .filter_map(|&c| i64::try_from(frames_from(video, ms, i128::from(c))).ok())
        .filter(|s| s.unsigned_abs() <= reach.unsigned_abs())
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// A track's times against the cuts at every shift within reach.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scan {
    /// Hits at each shift, summed over the styles timed at their own best shift.
    pub hits: Hits,
    /// Those styles' times that hit any shift, in ms, each with the shifts it hits; in time order.
    pub times: Vec<(i64, Vec<i64>)>,
}

/// The wide scan of a track. Its times are counted as [`super::styles`] counts them (a style's
/// distinct starts and distinct ends, karaoke left out), and a style counts when it is timed at
/// its own best shift, with MIN_AT_OFFSET hits there: a style a second late has its peak a second
/// off, and the best of several hundred shifts always stands out over a handful of times.
pub fn scan(video: &Video, events: &[Event]) -> Scan {
    let reach = reach(video);
    let mut seen = HashSet::new();
    let mut by_style: BTreeMap<&str, Vec<(i64, Vec<i64>)>> = BTreeMap::new();
    for e in events.iter().filter(|e| !e.karaoke) {
        for (is_end, ms) in [(false, e.start_ms), (true, e.end_ms)] {
            if !seen.insert((e.style.as_str(), is_end, ms)) {
                continue;
            }
            let shifts = shifts_of(video, ms, reach);
            if !shifts.is_empty() {
                by_style
                    .entry(e.style.as_str())
                    .or_default()
                    .push((ms, shifts));
            }
        }
    }
    let mut out = Scan::default();
    for times in by_style.into_values() {
        let mut hits = Hits::new();
        for (_, shifts) in &times {
            for &s in shifts {
                *hits.entry(s).or_default() += 1;
            }
        }
        let timed = best_where(&hits, |_| true)
            .is_some_and(|(top, n)| n >= MIN_AT_OFFSET && is_timed(&around(&hits, top)));
        if timed {
            for (s, n) in hits {
                *out.hits.entry(s).or_default() += n;
            }
            out.times.extend(times);
        }
    }
    out.times.sort_unstable();
    out
}

/// The hits at the OFFSETS shifts around `centre`, much as the ±MAX_OFFSET analysis there counts
/// them.
fn around(hits: &Hits, centre: i64) -> [usize; OFFSETS] {
    let mut out = [0; OFFSETS];
    for (slot, d) in (-MAX_OFFSET..=MAX_OFFSET).enumerate() {
        out[slot] = hits.get(&(centre + d)).copied().unwrap_or(0);
    }
    out
}

/// The shift with the most hits among those `keep` allows; on a tie, the one nearest 0, then the
/// lower.
fn best_where(hits: &Hits, keep: impl Fn(i64) -> bool) -> Option<(i64, usize)> {
    hits.iter()
        .map(|(&s, &n)| (s, n))
        .filter(|&(s, _)| keep(s))
        .max_by_key(|&(s, n)| (n, Reverse(s.unsigned_abs()), Reverse(s)))
}

/// Whether two shifts are more than MAX_OFFSET apart: nearer, they are one peak, split by a stretch
/// a frame off or by rounding.
fn apart(a: i64, b: i64) -> bool {
    a.abs_diff(b) > MAX_OFFSET.unsigned_abs()
}

/// The shift `hits` agree on, with its hits: the best, with at least MIN_AT_OFFSET, and twice as
/// many as any shift `rival(s, best)` says competes with it.
pub fn decided_by(hits: &Hits, rival: impl Fn(i64, i64) -> bool) -> Option<(i64, usize)> {
    let (top, n) = best_where(hits, |_| true)?;
    let second = best_where(hits, |s| rival(s, top)).map_or(0, |(_, r)| r);
    (n >= MIN_AT_OFFSET && n >= 2 * second).then_some((top, n))
}

/// The shift `hits` agree on: at least MIN_AT_OFFSET, and twice any shift apart from it.
pub fn decisive(hits: &Hits) -> Option<(i64, usize)> {
    decided_by(hits, apart)
}

/// A track's times pointing two ways, (shift, hits) each way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rival {
    /// The whole track: the best shift within MAX_OFFSET of 0, or the best of all when that is
    /// far, no shift within MAX_OFFSET of 0 has MIN_AT_OFFSET hits, and the best apart from it
    /// rivals it. Part of it: the centre.
    pub ours: (i64, usize),
    /// The whole track: the best shift apart from `ours`; with `ours` within MAX_OFFSET of 0,
    /// outside ±MAX_OFFSET as well. Part of it: the shift the first flagged window decides.
    pub theirs: (i64, usize),
    /// For part of the track: from the first flagged window's start to the last one's end, in ms.
    pub span: Option<(i64, i64)>,
}

/// Where the ±MAX_OFFSET analysis is centred, or the rival that can leave the track unclear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Centre {
    At(i64),
    Rival(Rival),
}

/// A decisive shift outside ±MAX_OFFSET is the centre, any other decisive one leaves it at 0.
/// Undecided, a shift outside ±MAX_OFFSET and apart from the best inside, with at least twice
/// MIN_AT_OFFSET hits and at least three quarters of that one's, is a rival: the analysis at 0
/// would read an alias of it as a shift of a frame or two. A GOP alias sits close to the true
/// peak, holding most of its hits; a peak well short of the best, or with few hits, is a repeated
/// sequence or chance; a shift just past the edge, next to a best at it, is that peak's shoulder.
/// When the best is far and no shift inside has MIN_AT_OFFSET hits, nothing near 0 speaks for
/// the track: the best and the best apart from it, when they are rivals by the same rule, are the
/// two ways it points, and 0 is neither.
/// Otherwise the scan has too little to say, and the analysis runs at 0.
pub fn centre(hits: &Hits) -> Centre {
    if let Some((top, _)) = decisive(hits) {
        return Centre::At(if top.abs() > MAX_OFFSET { top } else { 0 });
    }
    let near = best_where(hits, |s| s.abs() <= MAX_OFFSET).unwrap_or((0, 0));
    let far = best_where(hits, |_| true)
        .filter(|&(top, _)| top.abs() > MAX_OFFSET && near.1 < MIN_AT_OFFSET)
        .and_then(|top| rival(hits, top, |s| apart(s, top.0)));
    match far.or_else(|| rival(hits, near, |s| s.abs() > MAX_OFFSET && apart(s, near.0))) {
        Some(r) => Centre::Rival(r),
        None => Centre::At(0),
    }
}

/// `ours` against the best shift `keep` allows, when that one has at least twice MIN_AT_OFFSET
/// hits and three quarters of ours.
fn rival(hits: &Hits, ours: (i64, usize), keep: impl Fn(i64) -> bool) -> Option<Rival> {
    let theirs = best_where(hits, keep)?;
    (theirs.1 >= 2 * MIN_AT_OFFSET && 4 * theirs.1 >= 3 * ours.1).then_some(Rival {
        ours,
        theirs,
        span: None,
    })
}

/// One window of the scan: its hits, and the shift they decide.
pub struct Window {
    hits: Hits,
    shift: Option<i64>,
}

/// The scan's windows that hold a time, by index.
pub type Windows = BTreeMap<usize, Window>;

/// The scan window by window.
pub fn windows(scan: &Scan) -> Windows {
    let mut by_window: BTreeMap<usize, Hits> = BTreeMap::new();
    for (ms, shifts) in &scan.times {
        let hits = by_window.entry(window_of(*ms)).or_default();
        for &s in shifts {
            *hits.entry(s).or_default() += 1;
        }
    }
    by_window
        .into_iter()
        .map(|(w, hits)| {
            let shift = decisive(&hits).map(|(s, _)| s);
            (w, Window { hits, shift })
        })
        .collect()
}

/// The shift each window's hits decide, (window, shift), in window order; a window that decides
/// none is left out.
fn decided_windows(windows: &Windows) -> impl Iterator<Item = (usize, i64)> + '_ {
    windows
        .iter()
        .filter_map(|(&w, window)| window.shift.map(|s| (w, s)))
}

/// The windows whose hits decide on a shift apart from `centre`, in window order: the analysis at
/// `centre` cannot see their shift, and would read their times, measured from a keyframe forced a
/// GOP into a shot, as a shift of a frame or two.
pub fn far_windows(windows: &Windows, centre: i64) -> impl Iterator<Item = usize> + '_ {
    decided_windows(windows)
        .filter(move |&(_, s)| apart(s, centre))
        .map(|(w, _)| w)
}

/// Far windows in a row, as [`flag`] chains them.
struct FarRun {
    /// The first window's shift.
    shift: i64,
    first: usize,
    last: usize,
    windows: usize,
}

/// Part of the track at a shift the analysis at `centre` cannot see: two or more windows whose
/// hits decide on shifts apart from it, each within MAX_OFFSET of the first one's. A window that
/// decides nothing is skipped over, as a quiet stretch or a song says nothing of the shift; one
/// that decides near `centre` ends the run. One window alone is as likely a song timed to the
/// beat as a shift.
pub fn flag(windows: &Windows, centre: i64) -> Option<Rival> {
    let mut runs: Vec<FarRun> = Vec::new();
    let mut open = false;
    for (w, shift) in decided_windows(windows) {
        if !apart(shift, centre) {
            open = false;
            continue;
        }
        match runs.last_mut() {
            Some(run) if open && !apart(shift, run.shift) => {
                run.last = w;
                run.windows += 1;
            }
            _ => {
                runs.push(FarRun {
                    shift,
                    first: w,
                    last: w,
                    windows: 1,
                });
                open = true;
            }
        }
    }
    let run = runs.into_iter().find(|r| r.windows >= 2)?;
    let mut both = Hits::new();
    for window in windows.range(run.first..=run.last).map(|(_, w)| w) {
        for (&s, &n) in &window.hits {
            *both.entry(s).or_default() += n;
        }
    }
    let at = |s: i64| both.get(&s).copied().unwrap_or(0);
    Some(Rival {
        ours: (centre, at(centre)),
        theirs: (run.shift, at(run.shift)),
        span: Some((window_start(run.first), window_start(run.last + 1))),
    })
}

/// Up to `n` shifts with the most hits, each apart from those before it.
pub fn peaks(hits: &Hits, n: usize) -> Vec<(i64, usize)> {
    let mut out: Vec<(i64, usize)> = Vec::new();
    while out.len() < n {
        let Some(p) = best_where(hits, |s| out.iter().all(|&(o, _)| apart(s, o))) else {
            break;
        };
        out.push(p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::Event;
    use crate::timing::synthetic::{self, FRAME_NS, ms};
    use crate::timing::{Verdict, analyse};
    use crate::timing::{record, shared};

    /// Yuru Camp Season 2 E08's video and tracks 3 and 5, both in sync; track 3 with every time
    /// 1001 ms (24 frames) later is the late track.
    fn e08() -> (Video, Vec<Event>, Vec<Event>) {
        let mut frame_ns = 0;
        let mut keyframes = Vec::new();
        let (mut three, mut five) = (Vec::new(), Vec::new());
        let data = include_str!("testdata/yuru-camp-s2-e08.tsv");
        for line in data.lines().filter(|l| !l.starts_with('#')) {
            let f: Vec<&str> = line.split('\t').collect();
            match f[0] {
                "f" => frame_ns = f[1].parse().unwrap(),
                "k" => keyframes.push(f[1].parse().unwrap()),
                "e" => {
                    let e = Event {
                        track: f[1].parse().unwrap(),
                        start_ms: f[2].parse().unwrap(),
                        end_ms: f[3].parse().unwrap(),
                        layer: "0".into(),
                        style: f[5].into(),
                        text: String::new(),
                        pos: None,
                        karaoke: f[4] == "1",
                        raw: f[6].into(),
                    };
                    if e.track == 3 {
                        three.push(e);
                    } else {
                        five.push(e);
                    }
                }
                other => panic!("unknown row {other}"),
            }
        }
        (Video::new(frame_ns, keyframes), three, five)
    }

    #[test]
    fn e08s_track_as_released_reads_24_frames_late_and_as_fixed_in_sync() {
        let (video, three, five) = e08();
        assert!(
            video.forced_gaps.contains(&24) && video.forced_gaps.contains(&26),
            "{:?}",
            video.forced_gaps
        );
        let released: Vec<Event> = three
            .iter()
            .map(|e| Event {
                start_ms: e.start_ms + 1001,
                end_ms: e.end_ms + 1001,
                ..e.clone()
            })
            .collect();
        assert_eq!(analyse(&video, &released).verdict, Verdict::Shifted(24));
        assert_eq!(analyse(&video, &three).verdict, Verdict::InSync);
        assert_eq!(analyse(&video, &five).verdict, Verdict::InSync);
        let twins = shared::twins(&video, &released, &five);
        let s = shared::shared(&video, &released, &five, &twins);
        assert_eq!(s.gap(), Some(24), "{:?}", s.counts);
    }

    fn yuru() -> (Vec<i64>, Video) {
        let cuts = synthetic::cuts(500);
        let video = synthetic::yuru(&cuts);
        (cuts, video)
    }

    fn hits(pairs: &[(i64, usize)]) -> Hits {
        pairs.iter().copied().collect()
    }

    #[test]
    fn ten_seconds_is_240_frames_at_23_976_fps() {
        assert_eq!(reach(&yuru().1), 240);
    }

    #[test]
    fn a_frame_shorter_than_any_videos_reaches_no_further_than_240_fps_would() {
        // A corrupt DefaultDuration of 1 ns.
        assert_eq!(reach(&Video::unjudged(1, vec![0, 1_000_000])), 2400);
    }

    #[test]
    fn a_time_hits_each_shift_that_puts_it_on_the_frame_after_a_cut() {
        let video = Video::unjudged(FRAME_NS, synthetic::keyframes_ns(&[0, 100, 130, 500]));
        // Frame 110: 110 after frame 0, 10 after 100, 20 before 130; 500 is out of reach.
        assert_eq!(shifts_of(&video, ms(110), 240), [-20, 10, 110]);
    }

    #[test]
    fn a_style_of_a_dozen_lines_off_the_cuts_counts_for_nothing() {
        let (_, video) = yuru();
        // A line every 7.3 s, a few ms on, nothing to do with the cuts.
        let events: Vec<Event> = (0..12)
            .map(|i| {
                let start = 60_000 + i * 7_300 + i % 5;
                synthetic::event("Sign", start, start + 2_000 + i, &format!("Sign {i}"))
            })
            .collect();
        assert_eq!(scan(&video, &events).hits, Hits::new());
    }

    #[test]
    fn a_decisive_shift_needs_ten_hits_and_twice_any_more_than_three_frames_off() {
        assert_eq!(
            decisive(&hits(&[(24, 20), (23, 15), (0, 10)])),
            Some((24, 20))
        );
        assert_eq!(decisive(&hits(&[(24, 20), (0, 11)])), None);
        assert_eq!(decisive(&hits(&[(24, 9)])), None);
        assert_eq!(decisive(&hits(&[])), None);
    }

    #[test]
    fn the_centre_is_a_decisive_shift_outside_three_frames_and_a_close_rival_refuses() {
        assert_eq!(centre(&hits(&[(24, 100), (-2, 40)])), Centre::At(24));
        assert_eq!(centre(&hits(&[(2, 100), (30, 40)])), Centre::At(0));
        assert_eq!(
            centre(&hits(&[(-2, 135), (24, 134)])),
            Centre::Rival(Rival {
                ours: (-2, 135),
                theirs: (24, 134),
                span: None
            })
        );
        // A rival under ten hits is no evidence.
        assert_eq!(centre(&hits(&[(0, 15), (40, 9)])), Centre::At(0));
        // Nor is one with about half the peak's hits, or a dozen: a repeated sequence, or chance.
        assert_eq!(centre(&hits(&[(0, 73), (231, 39)])), Centre::At(0));
        assert_eq!(centre(&hits(&[(0, 13), (28, 11)])), Centre::At(0));
        // Two peaks within the window are the ±3 analysis's to sort out.
        assert_eq!(centre(&hits(&[(3, 20), (-3, 11)])), Centre::At(0));
        // A peak at the window's edge spills just past it: that is its shoulder, not a rival.
        assert_eq!(
            centre(&hits(&[(3, 100), (4, 80), (120, 70)])),
            Centre::At(0)
        );
        // With next to nothing near 0, two far peaks contest the track, and are named.
        assert_eq!(
            centre(&hits(&[(24, 100), (-168, 80), (0, 3)])),
            Centre::Rival(Rival {
                ours: (24, 100),
                theirs: (-168, 80),
                span: None
            })
        );
        assert_eq!(centre(&hits(&[])), Centre::At(0));
    }

    #[test]
    fn a_track_on_the_cuts_is_in_sync_and_one_a_second_off_is_found() {
        let (cuts, video) = yuru();
        for (late, verdict) in [
            (0, Verdict::InSync),
            (1, Verdict::Shifted(1)),
            (24, Verdict::Shifted(24)),
            (-24, Verdict::Shifted(-24)),
            (120, Verdict::Shifted(120)),
        ] {
            let a = analyse(&video, &synthetic::track(&cuts, late));
            assert_eq!((a.verdict, a.rival), (verdict, None), "{late}");
        }
    }

    #[test]
    fn a_track_a_second_late_is_analysed_around_that_second() {
        let (cuts, video) = yuru();
        let a = analyse(&video, &synthetic::track(&cuts, 24));
        assert_eq!(a.centre, 24);
        assert_eq!(a.peaks.first().map(|p| p.0), Some(24));
        assert_eq!(a.peaks.len(), 3);
    }

    #[test]
    fn with_the_forced_keyframes_taken_for_cuts_a_second_late_is_unclear_not_two_frames_early() {
        let cuts = synthetic::cuts(500);
        let all = Video::all_cuts(
            FRAME_NS,
            synthetic::keyframes_ns(&synthetic::keyframes(&cuts, 26, &[24])),
        );
        let a = analyse(&all, &synthetic::track(&cuts, 24));
        assert_eq!(a.verdict, Verdict::Unclear);
        let r = a.rival.expect("a rival");
        assert_eq!((r.ours.0, r.theirs.0, r.span), (-2, 24, None));
        assert!(a.each.iter().all(Option::is_none));
    }

    #[test]
    fn two_windows_in_a_row_at_another_shift_leave_the_track_unclear_and_say_where() {
        let (cuts, video) = yuru();
        // Shots from 9:00 to 15:00 five seconds late.
        let late = |cut: i64| {
            if (540_000..900_000).contains(&ms(cut)) {
                120
            } else {
                0
            }
        };
        let a = analyse(&video, &synthetic::track_by(&cuts, late));
        assert_eq!(a.verdict, Verdict::Unclear);
        let r = a.rival.expect("a rival");
        assert_eq!(
            (r.ours.0, r.theirs.0, r.span),
            (0, 120, Some((540_000, 900_000)))
        );
    }

    #[test]
    fn far_windows_a_frame_apart_with_a_window_of_no_lines_between_are_one_stretch() {
        let (cuts, video) = yuru();
        // Shots from 9:00 to 12:00 five seconds late, none from 12:00 to 15:00, and from 15:00 to
        // 18:00 five seconds and a frame late.
        let late = |cut: i64| match ms(cut) {
            540_000..720_000 => 120,
            900_000..1_080_000 => 121,
            _ => 0,
        };
        let from = cuts.partition_point(|&c| ms(c) < 720_000);
        let to = cuts.partition_point(|&c| ms(c) < 900_000);
        let events = [
            synthetic::track_by(&cuts[..=from], late),
            synthetic::track_by(&cuts[to..], late),
        ]
        .concat();
        let a = analyse(&video, &events);
        assert_eq!(a.verdict, Verdict::Unclear);
        let r = a.rival.expect("a rival");
        assert_eq!(
            (r.ours.0, r.theirs.0, r.span),
            (0, 120, Some((540_000, 1_080_000)))
        );
    }

    #[test]
    fn one_window_at_another_shift_is_not_enough() {
        let (cuts, video) = yuru();
        let late = |cut: i64| {
            if (540_000..720_000).contains(&ms(cut)) {
                120
            } else {
                0
            }
        };
        let a = analyse(&video, &synthetic::track_by(&cuts, late));
        assert_eq!((a.verdict, a.rival), (Verdict::InSync, None));
    }

    #[test]
    fn a_thin_track_raises_no_rival_however_far_off_its_song_peaks() {
        let (cuts, video) = yuru();
        // A song from 9:00 timed to the music, its lines five seconds after the cuts.
        let song = cuts.partition_point(|&c| ms(c) < 540_000);
        let mut events: Vec<Event> = (song..song + 11)
            .map(|i| {
                let (from, to) = (cuts[i] + 120, cuts[i + 1] + 120);
                synthetic::event("Song", ms(from), ms(to), &format!("Song {i}"))
            })
            .collect();
        // Six signs on the cuts, two of them during the song.
        let signs = [60_000, 240_000, 960_000, 1_140_000]
            .map(|at| cuts.partition_point(|&c| ms(c) < at))
            .into_iter()
            .chain([song + 3, song + 7]);
        events.extend(
            signs.map(|i| {
                synthetic::event("Sign", ms(cuts[i]), ms(cuts[i + 1]), &format!("Sign {i}"))
            }),
        );
        let hits = scan(&video, &events).hits;
        let Centre::Rival(r) = centre(&hits) else {
            panic!("{:?}", peaks(&hits, 3))
        };
        assert_eq!((r.ours, r.theirs.0, r.span), ((0, 12), 120, None), "{r:?}");
        // The song's window is far and left out: the four signs outside it decide nothing.
        let a = analyse(&video, &events);
        assert_eq!((a.verdict, a.rival, a.times), (Verdict::TooFew, None, 8));
    }

    #[test]
    fn a_thin_track_whose_song_spans_two_windows_raises_no_rival_and_follows_its_twins() {
        let (cuts, video) = yuru();
        // A song timed to the music, its lines five seconds after the cuts, across 3:00: half its
        // times in each window.
        let song = cuts.partition_point(|&c| ms(c + 120) < 180_000) - 6;
        let mut events: Vec<Event> = (song..song + 11)
            .map(|i| {
                let (from, to) = (cuts[i] + 120, cuts[i + 1] + 120);
                synthetic::event("Song", ms(from), ms(to), &format!("Song {i}"))
            })
            .collect();
        // Six signs on the cuts, two of them during the song.
        let signs = [420_000, 600_000, 960_000, 1_140_000]
            .map(|at| cuts.partition_point(|&c| ms(c) < at))
            .into_iter()
            .chain([song + 3, song + 7]);
        events.extend(
            signs.map(|i| {
                synthetic::event("Sign", ms(cuts[i]), ms(cuts[i + 1]), &format!("Sign {i}"))
            }),
        );
        let a = analyse(&video, &events);
        // The song's windows are far and left out; before they leave, its times and the signs'
        // are too few for a far peak to doubt the rest.
        assert_eq!(
            (a.verdict.clone(), a.rival, a.times),
            (Verdict::TooFew, None, 8),
            "{:?}",
            a.peaks
        );
        // Its twins, in a track in sync, settle it.
        let mut on = synthetic::track(&cuts, 0);
        on.extend(events.iter().cloned());
        let a5 = analyse(&video, &on);
        assert_eq!(a5.verdict, Verdict::InSync);
        let said = shared::lines_of(&video, &[(3, &events, &a), (5, &on, &a5)]);
        let Some(shared::Lines::Follows(f)) = &said[0] else {
            panic!("{said:?}")
        };
        assert_eq!(
            (f.source, f.gap, f.verdict.clone()),
            (5, 0, Verdict::InSync)
        );
    }

    #[test]
    fn a_track_thin_only_once_its_far_window_leaves_still_points_two_ways() {
        let (cuts, video) = yuru();
        // Seven lines on the cuts from 3:00, and thirteen a second late from 12:00.
        let line = |i: usize, late: i64| {
            let (from, to) = (cuts[i] + late, cuts[i + 1] + late);
            synthetic::event("D", ms(from), ms(to), &format!("Line {i}"))
        };
        let on = cuts.partition_point(|&c| ms(c) < 180_000);
        let late = cuts.partition_point(|&c| ms(c) < 720_000);
        let events: Vec<Event> = (on..on + 7)
            .map(|i| line(i, 0))
            .chain((late..late + 13).map(|i| line(i, 24)))
            .collect();
        let a = analyse(&video, &events);
        // Once the late window leaves the pool, 14 times are left; before it leaves, its times,
        // read as 2 frames early, count too.
        let r = a.rival.expect("a rival");
        assert_eq!(
            (a.verdict, a.times, r.ours.0, r.theirs.0, r.span),
            (Verdict::Unclear, 14, 0, 24, None)
        );
    }

    #[test]
    fn one_window_a_second_off_is_not_read_as_two_frames_early() {
        let (cuts, video) = yuru();
        // Shots from 9:00 to 12:00 a second late: measured from the keyframe forced 26 frames
        // after each cut, their times sit 2 frames before it.
        let late = |cut: i64| {
            if (540_000..720_000).contains(&ms(cut)) {
                24
            } else {
                0
            }
        };
        let a = analyse(&video, &synthetic::track_by(&cuts, late));
        assert_eq!((a.verdict, a.rival), (Verdict::InSync, None));
    }

    /// Asserts that the events starting in `window`, and only those, stay where they are, and that
    /// the analysis's unshifted blocks cover them.
    fn left_alone_in(a: &crate::timing::Analysis, events: &[Event], window: std::ops::Range<i64>) {
        for (i, e) in events.iter().enumerate() {
            assert_eq!(
                a.each[i].is_none(),
                window.contains(&e.start_ms),
                "{i}: {} – {}",
                e.start_ms,
                e.end_ms
            );
            if a.each[i].is_none() {
                assert!(
                    a.unshifted
                        .iter()
                        .any(|&(from, to)| from <= e.start_ms && e.end_ms <= to),
                    "{i}: {} – {} in {:?}",
                    e.start_ms,
                    e.end_ms,
                    a.unshifted
                );
            }
        }
    }

    #[test]
    fn a_whole_track_fix_leaves_a_window_at_another_shift_where_it_is() {
        let (cuts, video) = yuru();
        // A second late but for shots from 9:00 to 12:00, on the cuts.
        let late = |cut: i64| {
            if (540_000..720_000).contains(&ms(cut)) {
                0
            } else {
                24
            }
        };
        let events = synthetic::track_by(&cuts, late);
        let a = analyse(&video, &events);
        assert_eq!((a.verdict.clone(), a.rival), (Verdict::Shifted(24), None));
        left_alone_in(&a, &events, 540_000..720_000);
        // The finding apply carries out names them.
        let track = record::Track {
            id: 3,
            events: &events,
            analysis: &a,
            lines: None,
        };
        let (found, _) = record::findings("/m/a.mkv", &video, &[track]);
        let fix = found[0].timing.as_ref().expect("a timing fix");
        assert_eq!(
            fix.unshifted.len(),
            a.each.iter().filter(|s| s.is_none()).count()
        );
    }

    #[test]
    fn a_partial_fix_leaves_a_window_at_another_shift_where_it_is() {
        let (cuts, video) = yuru();
        // A second late, a frame more from halfway, but for shots from 3:00 to 6:00, on the cuts.
        let half = cuts[cuts.len() / 2];
        let late = |cut: i64| match ms(cut) {
            180_000..360_000 => 0,
            _ if cut < half => 24,
            _ => 25,
        };
        let events = synthetic::track_by(&cuts, late);
        let a = analyse(&video, &events);
        let Verdict::Partial(stretches) = &a.verdict else {
            panic!("{:?} {:?}", a.verdict, a.peaks)
        };
        let frames: Vec<i64> = stretches.iter().map(|s| s.frames).collect();
        assert_eq!(frames, [24, 25]);
        left_alone_in(&a, &events, 180_000..360_000);
    }

    #[test]
    fn a_far_windows_times_are_left_out_of_the_pooled_verdict() {
        let (cuts, video) = yuru();
        // Outside 9:00 – 12:00, a line from each 22nd cut to the next, on the cuts: too few times
        // for any window to decide, but enough to decide the track. From 9:00, thirteen shots in a
        // row a second late: enough for their window to decide, too few to rival the rest.
        let far = cuts.partition_point(|&c| ms(c) < 540_000);
        let line = |i: usize, late: i64| {
            let (from, to) = (cuts[i] + late, cuts[i + 1] + late);
            synthetic::event("D", ms(from), ms(to), &format!("Line {i}"))
        };
        let mut events: Vec<Event> = (0..cuts.len() - 1)
            .step_by(22)
            .filter(|&i| window_of(ms(cuts[i])) != 3)
            .map(|i| line(i, 0))
            .collect();
        events.extend((far..far + 13).map(|i| line(i, 24)));
        let a = analyse(&video, &events);
        // Decided by the pool, and the window a second late counted nowhere: its times, read as 2
        // frames early, would leave the pool undecided.
        assert_eq!(
            (a.verdict, a.agreeing, a.rival, a.times, a.pool),
            (Verdict::InSync, 0, None, 40, [0, 0, 0, 40, 0, 0, 0]),
            "{:?}",
            a.peaks
        );
    }

    #[test]
    fn a_second_late_with_a_frame_more_from_halfway_is_partial_at_24_and_25() {
        let (cuts, video) = yuru();
        let half = cuts[cuts.len() / 2];
        let a = analyse(
            &video,
            &synthetic::track_by(&cuts, |cut| if cut < half { 24 } else { 25 }),
        );
        let Verdict::Partial(stretches) = &a.verdict else {
            panic!("{:?}", a.verdict)
        };
        let frames: Vec<i64> = stretches.iter().map(|s| s.frames).collect();
        assert_eq!(frames, [24, 25]);
    }

    #[test]
    fn an_unjudged_video_is_not_scanned_wide() {
        // Every keyframe a cut, 2 s apart: a track a frame late could as well be 49 frames early
        // or 51 late.
        let video = Video::unjudged(40_000_000, (0..=900).map(|i| i * 2_000_000_000).collect());
        let events: Vec<_> = (0..300)
            .map(|n| synthetic::event("D", n * 6000 + 40, n * 6000 + 2040, &format!("L{n}")))
            .collect();
        let a = analyse(&video, &events);
        assert_eq!(
            (a.verdict, a.centre, a.rival),
            (Verdict::Shifted(1), 0, None)
        );
        assert_eq!(a.peaks, []);
    }
}
