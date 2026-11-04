//! Lines two tracks of one file share, and how many frames apart they are. A Signs/Songs track
//! repeats lines of its Full track (signs, songs, the OP and ED) but rarely has enough of them on
//! cuts for a verdict of its own. Where its lines are another track's, a whole number of frames
//! apart, it is timed as that track is.

use std::collections::{HashMap, HashSet};

use super::lines;
use super::{Analysis, MAX_OFFSET, Stretch, Verdict, Video, frame_index, wide};
use crate::text::Event;

/// How one track's lines sit against the same lines in another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shared {
    /// Distinct times (per style, start or end) of lines with a twin: a start when within reach of
    /// the twin's start, an end when within reach of its end.
    pub times: usize,
    /// How many of them sit at each gap, this track's frame less the other's.
    pub counts: wide::Hits,
}

impl Shared {
    /// The gap the shared times agree on: at least MIN_AT_OFFSET of them, and twice as many as at
    /// any other gap, however near. Two groups' karaoke, each timed to the music, sits a frame or
    /// three apart and smears over neighbouring gaps; a gap read off such a smear would contradict
    /// a track's own verdict.
    pub fn gap(&self) -> Option<i64> {
        wide::decided_by(&self.counts, |g, gap| g != gap).map(|(gap, _)| gap)
    }

    /// How many shared times sit at `gap`.
    pub fn at(&self, gap: i64) -> usize {
        self.counts.get(&gap).copied().unwrap_or(0)
    }
}

/// How many frames apart a line and its twin may be. A video with too few keyframes to scan wide
/// has its tracks looked at within ±MAX_OFFSET only, so a gap between twins is held to that too:
/// further off, it would give a track following its twins a shift its own times are never
/// measured for.
fn reach(video: &Video) -> i64 {
    if video.judged {
        wide::reach(video)
    } else {
        MAX_OFFSET
    }
}

/// Each event's twin in `other`: the one event there with the same raw text starting within
/// MAX_OFFSET frames of it or, when none is that near, within [`reach`] frames. `None` when there
/// is none, or two or more within the nearer bound that holds any: a sign repeated frame by frame
/// would match its neighbour as well as itself, and a shift would read as no shift. A chorus line
/// or a sign that comes back within reach is told from its repeat by the nearer bound.
pub fn twins(video: &Video, track: &[Event], other: &[Event]) -> Vec<Option<usize>> {
    let bounds = [MAX_OFFSET, reach(video)].map(|b| u128::from(b.unsigned_abs()));
    let mut by_raw: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, e) in other.iter().enumerate() {
        by_raw.entry(e.raw.as_str()).or_default().push(i);
    }
    track
        .iter()
        .map(|e| {
            let at = frame_index(video, e.start_ms);
            let namesakes = by_raw.get(e.raw.as_str())?;
            let apart = |i: usize| (frame_index(video, other[i].start_ms) - at).unsigned_abs();
            bounds
                .iter()
                .find_map(|&bound| {
                    let mut near = namesakes.iter().copied().filter(|&i| apart(i) <= bound);
                    let one = near.next()?;
                    Some(near.next().is_none().then_some(one))
                })
                .flatten()
        })
        .collect()
}

/// The gaps between `track`'s lines and their twins in `other`.
pub fn shared(video: &Video, track: &[Event], other: &[Event], twins: &[Option<usize>]) -> Shared {
    let reach = reach(video);
    let mut seen = HashSet::new();
    let mut counts = wide::Hits::new();
    for (e, twin) in track.iter().zip(twins) {
        let Some(t) = twin.map(|i| &other[i]) else {
            continue;
        };
        for (is_end, mine, theirs) in [(false, e.start_ms, t.start_ms), (true, e.end_ms, t.end_ms)]
        {
            if !seen.insert((e.style.as_str(), is_end, mine)) {
                continue;
            }
            let gap = i64::try_from(frame_index(video, mine) - frame_index(video, theirs))
                .ok()
                .filter(|g| g.unsigned_abs() <= reach.unsigned_abs());
            if let Some(gap) = gap {
                *counts.entry(gap).or_default() += 1;
            }
        }
    }
    Shared {
        times: counts.values().sum(),
        counts,
    }
}

/// A track timed as another is, `gap` frames after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Follow {
    /// The other track's mkvmerge ID.
    pub source: usize,
    pub gap: i64,
    /// Over the twins the other track's verdict places.
    pub shared: Shared,
    /// The other track's verdict, `gap` frames later, for this track's events.
    pub verdict: Verdict,
    /// As [`Analysis::each`], for this track's events.
    pub each: Vec<Option<usize>>,
    /// As [`Analysis::unshifted`].
    pub unshifted: Vec<(i64, i64)>,
}

/// What a track's shared lines say about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lines {
    /// It has no verdict of its own and takes another track's.
    Follows(Follow),
    /// Its own whole-track verdict is not what its lines shared with `source` say.
    Disagrees { source: usize, says: Verdict },
}

/// What the file's tracks' shared lines say, track by track. Each track is compared with the
/// other tracks that have a verdict, and the one it shares the most times with at an agreed gap
/// is used (on a tie, the smaller gap, then the lower ID). A verdict says nothing of a line it
/// leaves where it is, so only twins the other track's verdict places count and, for a shifted or
/// in-sync track, only its lines its own verdict places. A track with too few lines or an unclear
/// verdict follows it, unless part of the track is flagged at another shift. A shifted or in-sync
/// track is checked against it when that track is shifted or in sync too; a partial one is not
/// checked.
///
/// `tracks`: each track's mkvmerge ID, events and analysis.
pub fn lines_of(video: &Video, tracks: &[(usize, &[Event], &Analysis)]) -> Vec<Option<Lines>> {
    tracks
        .iter()
        .map(|&(id, mine, own)| {
            // A partial track's verdict is not checked against shared lines, and neither is a
            // track flagged for part of its length at another shift: one gap cannot speak for both
            // parts, and following its twins would move the whole track by the shift of whichever
            // part they share. A track torn between two whole-track shifts is one track, and its
            // twins settle which way it points.
            if matches!(own.verdict, Verdict::Partial(_))
                || own.rival.is_some_and(|r| r.span.is_some())
            {
                return None;
            }
            // A track with no verdict of its own places none of its lines: its twins' verdict
            // alone says which of them count.
            let own_places = matches!(own.verdict, Verdict::InSync | Verdict::Shifted(_));
            let (source, theirs, twins, shared, gap) = tracks
                .iter()
                .filter(|&&(other, _, a)| other != id && is_decided(&a.verdict))
                .filter_map(|&(other, events, a)| {
                    let twins = twins(video, mine, events);
                    // A line a verdict leaves where it is sits at a shift that verdict does not
                    // see: neither it nor its twin says anything of the gap between the two
                    // tracks. `follow` still gets every twin: one its source leaves where it is
                    // leaves this track's copy where it is too.
                    let counts = |j: usize, i: usize| {
                        a.each[i].is_some() && (!own_places || own.each[j].is_some())
                    };
                    let placed: Vec<Option<usize>> = twins
                        .iter()
                        .enumerate()
                        .map(|(j, t)| t.filter(|&i| counts(j, i)))
                        .collect();
                    let shared = shared(video, mine, events, &placed);
                    let gap = shared.gap()?;
                    Some((other, a, twins, shared, gap))
                })
                .max_by_key(|(other, _, _, s, gap)| {
                    (
                        s.times,
                        std::cmp::Reverse(gap.abs()),
                        std::cmp::Reverse(*other),
                    )
                })?;
            match (&own.verdict, offset(&theirs.verdict)) {
                (Verdict::TooFew | Verdict::Unclear, _) => {
                    follow(video, source, mine, theirs, &twins, gap, shared).map(Lines::Follows)
                }
                (mine @ (Verdict::InSync | Verdict::Shifted(_)), Some(theirs)) => {
                    let says = super::whole(theirs + gap);
                    (says != *mine).then_some(Lines::Disagrees { source, says })
                }
                _ => None,
            }
        })
        .collect()
}

fn is_decided(v: &Verdict) -> bool {
    matches!(
        v,
        Verdict::InSync | Verdict::Shifted(_) | Verdict::Partial(_)
    )
}

/// A whole-track verdict's offset.
fn offset(v: &Verdict) -> Option<i64> {
    match v {
        Verdict::InSync => Some(0),
        Verdict::Shifted(frames) => Some(*frames),
        _ => None,
    }
}

/// `source`'s verdict for a track whose events are `mine`, `gap` frames after it. That verdict does
/// not see the shift of a line the source leaves where it is, nor of its twin, nor of any line in
/// one of the source's far windows. So a block with a line starting in a far window, `gap` frames
/// on, stays where it is, whatever its twins; any other block goes with its twins' stretch, and
/// stays where it is when its twins do or disagree; a block without twins goes with the stretch it
/// starts in. `None` when no block goes with a stretch.
fn follow(
    video: &Video,
    source: usize,
    mine: &[Event],
    theirs: &Analysis,
    twins: &[Option<usize>],
    gap: i64,
    shared: Shared,
) -> Option<Follow> {
    let whole_track = |frames: i64| Stretch {
        from_ms: None,
        to_ms: None,
        frames: frames + gap,
    };
    let stretches: Vec<Stretch> = match &theirs.verdict {
        Verdict::InSync => vec![whole_track(0)],
        Verdict::Shifted(frames) => vec![whole_track(*frames)],
        Verdict::Partial(stretches) => stretches
            .iter()
            .map(|s| Stretch {
                frames: s.frames + gap,
                ..*s
            })
            .collect(),
        Verdict::TooFew | Verdict::Unclear => return None,
    };
    let blocks = lines::blocks(mine);
    // A line is in the window it starts in by the source's times, `gap` frames earlier. In a far
    // window it stays where it is even when its twin is placed: a twin can sit up to `reach` away,
    // in the next window, and moved with it the line would move by the wrong amount, unreported,
    // where held it is listed as left where it is. In ns as an i128, as a corrupt time near either
    // end of i64 ms is past an i64 in ns; `text::ms` truncates toward 0:00, and a time before 0:00
    // is in the first window either way.
    let far = |i: usize| {
        let ns =
            i128::from(mine[i].start_ms) * 1_000_000 - i128::from(gap) * i128::from(video.frame_ns);
        theirs.far.contains(&super::window_of(crate::text::ms(ns)))
    };
    let of: Vec<Option<usize>> = blocks
        .iter()
        .map(|b| {
            if b.events.iter().any(|&i| far(i)) {
                return None;
            }
            let mut theirs_of = b
                .events
                .iter()
                .filter_map(|&i| twins[i])
                .map(|t| theirs.each[t]);
            match theirs_of.next() {
                Some(first) => theirs_of.all(|s| s == first).then_some(first).flatten(),
                None => stretches
                    .iter()
                    .rposition(|s| s.from_ms.is_none_or(|from| from <= b.start_ms)),
            }
        })
        .collect();
    let m = lines::moves(&blocks, &of, mine.len(), &stretches);
    // Every line left where it is: the source says nothing of this track's timing, not that it is
    // in sync.
    if m.each.iter().all(Option::is_none) {
        return None;
    }
    // None of this track's lines in a stretch that moves: the track is in sync.
    let shifts = |k: usize| m.stretches[k].frames != 0;
    let (verdict, each, unshifted) = if !m.each.iter().flatten().any(|&k| shifts(k)) {
        (Verdict::InSync, vec![Some(0); mine.len()], Vec::new())
    } else if let [one] = m.stretches.as_slice() {
        (super::whole(one.frames), m.each, m.unshifted)
    } else {
        (Verdict::Partial(m.stretches), m.each, m.unshifted)
    };
    Some(Follow {
        source,
        gap,
        shared,
        verdict,
        each,
        unshifted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timing::synthetic;
    use crate::timing::wide::Hits;
    use crate::timing::{OFFSETS, analyse};

    /// 25 fps, a keyframe every 2 s.
    fn video() -> Video {
        Video::unjudged(40_000_000, (0..=900).map(|i| i * 2_000_000_000).collect())
    }

    /// [`video`], scanned wide: twins are looked for up to ten seconds apart.
    fn judged() -> Video {
        Video::all_cuts(40_000_000, video().keyframes_ns)
    }

    fn event(text: &str, start_ms: i64, end_ms: i64) -> Event {
        Event {
            track: 0,
            start_ms,
            end_ms,
            layer: "0".into(),
            style: "Sign".into(),
            text: text.into(),
            pos: None,
            karaoke: false,
            raw: format!("0,Sign,,0,0,0,,{text}"),
        }
    }

    /// `n` signs, one every 10 s, each 3 s long, `frames` frames after those of `signs(n, 0)`.
    fn signs(n: i64, frames: i64) -> Vec<Event> {
        (0..n)
            .map(|i| {
                let start = i * 10_000 + 500 + frames * 40;
                event(&format!("Sign {i}"), start, start + 3000)
            })
            .collect()
    }

    #[test]
    fn a_twin_is_the_one_line_with_the_same_raw_text_within_ten_seconds() {
        let other = [
            event("A", 1000, 2000),
            event("B", 5000, 6000),
            // The same sign on two frames in a row: neither is anyone's only twin.
            event("C", 9000, 9040),
            event("C", 9040, 9080),
            event("E", 30_000, 31_000),
            event("F", 50_000, 51_000),
            event("F", 55_000, 56_000),
        ];
        let track = [
            event("A", 1040, 2040),
            event("B", 5160, 6160),
            event("C", 9040, 9080),
            event("D", 1000, 2000),
            // Eleven seconds from its namesake: too far.
            event("E", 41_000, 42_000),
            // Two namesakes within reach, 2 s and 3 s off: neither is its only twin.
            event("F", 52_000, 53_000),
        ];
        assert_eq!(
            twins(&judged(), &track, &other),
            [Some(0), Some(1), None, None, None, None]
        );
    }

    #[test]
    fn a_namesake_within_three_frames_is_the_twin_however_many_are_within_ten_seconds() {
        let other = [
            // A chorus line sung twice, 6 s apart.
            event("Chorus", 10_000, 12_000),
            event("Chorus", 16_000, 18_000),
            event("Sign", 30_000, 31_000),
            event("Sign", 37_000, 38_000),
        ];
        let track = [
            // A frame after the first chorus line.
            event("Chorus", 10_040, 12_040),
            // 3 s after one sign and 4 s before the other: neither is its only twin.
            event("Sign", 33_000, 34_000),
        ];
        assert_eq!(twins(&judged(), &track, &other), [Some(0), None]);
    }

    #[test]
    fn on_a_clip_too_short_to_scan_wide_a_twin_is_within_three_frames() {
        // 24 frames apart: a clip's track is looked at within 3 frames, a longer video's within
        // ten seconds.
        let other = [event("A", 10_000, 12_000)];
        let track = [event("A", 10_960, 12_960)];
        let (short, long) = (video(), judged());
        assert_eq!(twins(&short, &track, &other), [None]);
        assert_eq!(twins(&long, &track, &other), [Some(0)]);
        // Nor is a pair that far apart counted at its gap.
        assert_eq!(shared(&short, &track, &other, &[Some(0)]).times, 0);
        assert_eq!(shared(&long, &track, &other, &[Some(0)]).at(24), 2);
    }

    #[test]
    fn a_gap_needs_twice_the_times_of_any_other_however_near() {
        let split = Shared {
            times: 249,
            counts: Hits::from([(23, 51), (24, 198)]),
        };
        assert_eq!((split.gap(), split.at(24)), (Some(24), 198));
        let torn = Shared {
            times: 300,
            counts: Hits::from([(0, 150), (24, 150)]),
        };
        assert_eq!(torn.gap(), None);
        // Karaoke timed to the music a frame or three apart in each track.
        let smeared = Shared {
            times: 95,
            counts: Hits::from([(2, 30), (3, 40), (4, 25)]),
        };
        assert_eq!(smeared.gap(), None);
    }

    #[test]
    fn a_track_a_second_after_its_twins_follows_them_and_a_two_frame_verdict_is_blocked() {
        let cuts = synthetic::cuts(500);
        let video = synthetic::yuru(&cuts);
        let late = synthetic::track(&cuts, 24);
        let on = synthetic::track(&cuts, 0);
        let in_sync = analysis(Verdict::InSync, vec![Some(0); on.len()]);
        // With no verdict of its own, it takes the other track's, 24 frames on.
        let unclear = analysis(Verdict::Unclear, vec![None; late.len()]);
        let out = lines_of(&video, &[(3, &late, &unclear), (5, &on, &in_sync)]);
        let Some(Lines::Follows(f)) = &out[0] else {
            panic!("{out:?}")
        };
        assert_eq!(
            (f.source, f.gap, f.verdict.clone()),
            (5, 24, Verdict::Shifted(24))
        );
        // Read as two frames early, its twins say otherwise: no fix.
        let early = analysis(Verdict::Shifted(-2), vec![Some(0); late.len()]);
        let out = lines_of(&video, &[(3, &late, &early), (5, &on, &in_sync)]);
        assert_eq!(
            out[0],
            Some(Lines::Disagrees {
                source: 5,
                says: Verdict::Shifted(24)
            })
        );
    }

    #[test]
    fn a_track_torn_between_two_whole_track_shifts_follows_its_twins() {
        let cuts = synthetic::cuts(500);
        // Every keyframe taken for a cut: a track a second late reads two frames early as well.
        let video = Video::all_cuts(
            synthetic::FRAME_NS,
            synthetic::keyframes_ns(&synthetic::keyframes(&cuts, 26, &[24])),
        );
        let late = synthetic::track(&cuts, 24);
        let on = synthetic::track(&cuts, 0);
        let torn = analyse(&video, &late);
        let r = torn.rival.expect("a rival");
        assert_eq!(
            (torn.verdict.clone(), r.ours.0, r.theirs.0, r.span),
            (Verdict::Unclear, -2, 24, None)
        );
        // Read off the same keyframes, the twins' own times are unclear too: their verdict is set.
        let in_sync = analysis(Verdict::InSync, vec![Some(0); on.len()]);
        let out = lines_of(&video, &[(3, &late, &torn), (5, &on, &in_sync)]);
        let Some(Lines::Follows(f)) = &out[0] else {
            panic!("{out:?}")
        };
        assert_eq!(
            (f.source, f.gap, f.verdict.clone()),
            (5, 24, Verdict::Shifted(24))
        );
    }

    #[test]
    fn shared_lines_agree_on_a_gap_with_ten_times_and_twice_the_runner_up() {
        let v = video();
        let theirs = signs(5, 0);
        // Five signs a frame later: 10 times at +1.
        let later = signs(5, 1);
        let s = shared(&v, &later, &theirs, &twins(&v, &later, &theirs));
        assert_eq!((s.times, s.at(1), s.gap()), (10, 10, Some(1)));
        // Four of them: 8 times, too few.
        let fewer = signs(4, 1);
        let s = shared(&v, &fewer, &theirs, &twins(&v, &fewer, &theirs));
        assert_eq!((s.times, s.gap()), (8, None));
        // The same signs: gap 0.
        let same = signs(5, 0);
        let s = shared(&v, &same, &theirs, &twins(&v, &same, &theirs));
        assert_eq!(s.gap(), Some(0));
    }

    fn analysis(verdict: Verdict, each: Vec<Option<usize>>) -> Analysis {
        Analysis {
            verdict,
            times: 0,
            pool: [0; OFFSETS],
            windows: 0,
            agreeing: 0,
            styles: Vec::new(),
            frame_ns: 40_000_000,
            each,
            unshifted: Vec::new(),
            far: Vec::new(),
            centre: 0,
            rival: None,
            peaks: Vec::new(),
        }
    }

    #[test]
    fn an_undecided_track_follows_the_track_it_shares_most_with() {
        let v = video();
        let full = signs(20, 0);
        let few = signs(6, 0);
        let thin = signs(20, 0);
        let late = analysis(Verdict::Shifted(1), vec![Some(0); 20]);
        let in_sync = analysis(Verdict::InSync, vec![Some(0); 6]);
        let unclear = analysis(Verdict::Unclear, vec![None; 20]);
        let out = lines_of(
            &v,
            &[(2, &full, &late), (3, &few, &in_sync), (5, &thin, &unclear)],
        );
        // Track 5 shares 40 times with track 2 and 12 with track 3 (both at gap 0): it follows 2.
        let Some(Lines::Follows(f)) = &out[2] else {
            panic!("{out:?}")
        };
        assert_eq!((f.source, f.gap, f.shared.times), (2, 0, 40));
        assert_eq!(f.verdict, Verdict::Shifted(1));
        assert_eq!(f.each, vec![Some(0); 20]);
        // Track 3's own verdict, in sync, is not what its lines shared with track 2 say.
        assert_eq!(
            out[1],
            Some(Lines::Disagrees {
                source: 2,
                says: Verdict::Shifted(1)
            })
        );
        // And track 2's, a frame late, is not what its lines shared with track 3 say: track 5 has
        // no verdict to compare with. Neither track's own verdict stands.
        assert_eq!(
            out[0],
            Some(Lines::Disagrees {
                source: 3,
                says: Verdict::InSync
            })
        );
    }

    #[test]
    fn a_track_a_frame_apart_from_its_source_gets_a_shift_of_its_own() {
        let v = video();
        let full = signs(20, 0);
        let signs_late = signs(20, 1);
        let late = analysis(Verdict::Shifted(1), vec![Some(0); 20]);
        let unclear = analysis(Verdict::TooFew, vec![None; 20]);
        let out = lines_of(&v, &[(5, &full, &late), (6, &signs_late, &unclear)]);
        let Some(Lines::Follows(f)) = &out[1] else {
            panic!("{out:?}")
        };
        assert_eq!((f.gap, f.verdict.clone()), (1, Verdict::Shifted(2)));
    }

    #[test]
    fn off_a_partial_source_a_track_with_no_line_in_a_stretch_that_moves_is_in_sync() {
        let v = video();
        // The source: late to 0:40, in sync after. This track's signs are all after 0:40.
        let full = signs(20, 0);
        let mut each: Vec<Option<usize>> = vec![Some(0); 4];
        each.extend(vec![Some(1); 16]);
        let partial = analysis(
            Verdict::Partial(vec![
                Stretch {
                    from_ms: None,
                    to_ms: Some(33_500),
                    frames: 1,
                },
                Stretch {
                    from_ms: Some(40_500),
                    to_ms: None,
                    frames: 0,
                },
            ]),
            each,
        );
        let thin: Vec<Event> = full[4..].to_vec();
        let unclear = analysis(Verdict::Unclear, vec![None; 16]);
        let out = lines_of(&v, &[(5, &full, &partial), (6, &thin, &unclear)]);
        let Some(Lines::Follows(f)) = &out[1] else {
            panic!("{out:?}")
        };
        assert_eq!(f.verdict, Verdict::InSync);
    }

    #[test]
    fn off_a_partial_source_each_block_goes_with_its_twins_stretch() {
        let v = video();
        // Ten signs late, then ten in sync: the source's stretches end on lines.
        let full = signs(20, 0);
        let mut each: Vec<Option<usize>> = vec![Some(0); 10];
        each.extend(vec![Some(1); 10]);
        // One sign the source left where it was.
        each[15] = None;
        let partial = analysis(
            Verdict::Partial(vec![
                Stretch {
                    from_ms: None,
                    to_ms: Some(93_500),
                    frames: 1,
                },
                Stretch {
                    from_ms: Some(100_500),
                    to_ms: None,
                    frames: 0,
                },
            ]),
            each,
        );
        // This track has the signs and one line of its own at 0:55, between the 6th and 7th.
        let mut thin = full.clone();
        thin.push(event("Own", 55_000, 56_000));
        let unclear = analysis(Verdict::Unclear, vec![None; 21]);
        let out = lines_of(&v, &[(5, &full, &partial), (6, &thin, &unclear)]);
        let Some(Lines::Follows(f)) = &out[1] else {
            panic!("{out:?}")
        };
        assert_eq!(
            f.verdict,
            Verdict::Partial(vec![
                Stretch {
                    from_ms: None,
                    to_ms: Some(93_500),
                    frames: 1,
                },
                Stretch {
                    from_ms: Some(100_500),
                    to_ms: None,
                    frames: 0,
                },
            ])
        );
        let mut want: Vec<Option<usize>> = vec![Some(0); 10];
        want.extend(vec![Some(1); 10]);
        want[15] = None;
        // The line of its own starts in the first stretch.
        want.push(Some(0));
        assert_eq!(f.each, want);
        assert_eq!(f.unshifted, [(150_500, 153_500)]);
    }

    /// `events` a frame late, as [`analyse`] makes it when the wide scan puts the window from 3:00
    /// to 6:00 far from the centre: the lines starting there stay where they are.
    fn late_but_3_to_6(events: &[Event]) -> Analysis {
        let far = |e: &Event| (180_000..360_000).contains(&e.start_ms);
        let each = events.iter().map(|e| (!far(e)).then_some(0)).collect();
        let left: Vec<(i64, i64)> = events
            .iter()
            .filter(|e| far(e))
            .map(|e| (e.start_ms, e.end_ms))
            .collect();
        let mut a = analysis(Verdict::Shifted(1), each);
        a.unshifted = lines::blocks_of(&left)
            .into_iter()
            .map(|b| (b.start_ms, b.end_ms))
            .collect();
        a.far = vec![1];
        a
    }

    #[test]
    fn off_a_shifted_source_a_line_whose_twin_stays_where_it_is_stays_too() {
        let v = video();
        let full = signs(20, 0);
        let late = late_but_3_to_6(&full);
        // This track has the signs and one line of its own at 0:55, between the 6th and 7th.
        let mut thin = full.clone();
        thin.push(event("Own", 55_000, 56_000));
        let unclear = analysis(Verdict::Unclear, vec![None; 21]);
        let out = lines_of(&v, &[(5, &full, &late), (6, &thin, &unclear)]);
        let Some(Lines::Follows(f)) = &out[1] else {
            panic!("{out:?}")
        };
        assert_eq!(f.verdict, Verdict::Shifted(1));
        let mut want = vec![Some(0); 21];
        want[18] = None;
        want[19] = None;
        assert_eq!(f.each, want);
        assert_eq!(f.unshifted, [(180_500, 183_500), (190_500, 193_500)]);
    }

    #[test]
    fn off_a_shifted_source_a_block_whose_twins_disagree_stays_where_it_is() {
        let v = video();
        // A line from 2:55 to 3:05 the source moves, under the 19th sign, which starts after 3:00
        // and stays where it is.
        let mut full = signs(20, 0);
        full.push(event("Long", 175_000, 185_000));
        let late = late_but_3_to_6(&full);
        let thin = full.clone();
        let unclear = analysis(Verdict::Unclear, vec![None; 21]);
        let out = lines_of(&v, &[(5, &full, &late), (6, &thin, &unclear)]);
        let Some(Lines::Follows(f)) = &out[1] else {
            panic!("{out:?}")
        };
        assert_eq!(f.verdict, Verdict::Shifted(1));
        // The two show together: neither moves.
        let mut want = vec![Some(0); 21];
        want[18] = None;
        want[19] = None;
        want[20] = None;
        assert_eq!(f.each, want);
        assert_eq!(f.unshifted, [(175_000, 185_000), (190_500, 193_500)]);
    }

    #[test]
    fn a_track_whose_twins_all_stay_where_they_are_follows_no_one() {
        let v = video();
        let full = signs(40, 0);
        let late = late_but_3_to_6(&full);
        // This track has only the signs from 3:00 to 6:00: its twins say nothing of its timing.
        let thin = full[18..36].to_vec();
        let unclear = analysis(Verdict::Unclear, vec![None; 18]);
        let out = lines_of(&v, &[(5, &full, &late), (6, &thin, &unclear)]);
        assert_eq!(out[1], None);
    }

    #[test]
    fn off_a_shifted_source_a_line_of_its_own_in_a_far_window_stays_where_it_is() {
        let v = video();
        let full = signs(20, 0);
        let late = late_but_3_to_6(&full);
        // This track has the signs and one line of its own at 3:20, after the last sign.
        let mut thin = full.clone();
        thin.push(event("Own", 200_000, 201_000));
        let unclear = analysis(Verdict::Unclear, vec![None; 21]);
        let out = lines_of(&v, &[(5, &full, &late), (6, &thin, &unclear)]);
        let Some(Lines::Follows(f)) = &out[1] else {
            panic!("{out:?}")
        };
        assert_eq!(f.verdict, Verdict::Shifted(1));
        let mut want = vec![Some(0); 21];
        want[18] = None;
        want[19] = None;
        want[20] = None;
        assert_eq!(f.each, want);
        assert_eq!(
            f.unshifted,
            [(180_500, 183_500), (190_500, 193_500), (200_000, 201_000)]
        );
    }

    #[test]
    fn off_an_analysed_source_a_line_of_its_own_in_a_far_window_stays_where_it_is() {
        let cuts = synthetic::cuts(500);
        let video = synthetic::yuru(&cuts);
        // A second late but for shots from 9:00 to 12:00, on the cuts.
        let full = synthetic::track_by(&cuts, |cut| {
            if (540_000..720_000).contains(&synthetic::ms(cut)) {
                0
            } else {
                24
            }
        });
        let late = analyse(&video, &full);
        assert_eq!(
            (late.verdict.clone(), late.far.clone()),
            (Verdict::Shifted(24), vec![3])
        );
        // This track has the lines before 9:00 and one of its own at 10:00.
        let mut thin: Vec<Event> = full
            .iter()
            .filter(|e| e.start_ms < 540_000)
            .cloned()
            .collect();
        let own = thin.len();
        thin.push(event("Own", 600_000, 601_000));
        let unclear = analysis(Verdict::Unclear, vec![None; thin.len()]);
        let out = lines_of(&video, &[(5, &full, &late), (6, &thin, &unclear)]);
        let Some(Lines::Follows(f)) = &out[1] else {
            panic!("{out:?}")
        };
        assert_eq!(f.verdict, Verdict::Shifted(24));
        let mut want = vec![Some(0); thin.len()];
        want[own] = None;
        assert_eq!(f.each, want);
        assert_eq!(f.unshifted, [(600_000, 601_000)]);
    }

    #[test]
    fn a_track_whose_twins_all_stay_where_they_are_and_a_line_of_its_own_follows_no_one() {
        let v = video();
        let full = signs(40, 0);
        let late = late_but_3_to_6(&full);
        // The signs from 3:00 to 6:00, which say nothing of its timing, and one line of its own at
        // 0:55, which says nothing either.
        let mut thin = full[18..36].to_vec();
        thin.push(event("Own", 55_000, 56_000));
        let unclear = analysis(Verdict::Unclear, vec![None; 19]);
        let out = lines_of(&v, &[(5, &full, &late), (6, &thin, &unclear)]);
        assert_eq!(out[1], None);
    }

    #[test]
    fn a_track_every_block_of_which_reaches_a_far_window_follows_no_one() {
        let v = video();
        let full = signs(40, 0);
        let late = late_but_3_to_6(&full);
        // The signs from 1:40 to 3:00 the source moves, and the first one after 3:00, all under
        // a line from 1:35 to 3:05: one block, starting a line in the far window.
        let mut thin = full[10..19].to_vec();
        thin.push(event("Long", 95_000, 185_000));
        let unclear = analysis(Verdict::Unclear, vec![None; 10]);
        let out = lines_of(&v, &[(5, &full, &late), (6, &thin, &unclear)]);
        assert_eq!(out[1], None);
    }

    #[test]
    fn a_track_whose_twins_all_stay_where_they_are_does_not_disagree_with_their_verdict() {
        let v = video();
        let full = signs(40, 0);
        let late = late_but_3_to_6(&full);
        // In sync by its own times, and sharing only the signs from 3:00 to 6:00, which the
        // source's verdict says nothing of.
        let thin = full[18..36].to_vec();
        let in_sync = analysis(Verdict::InSync, vec![Some(0); 18]);
        let out = lines_of(&v, &[(5, &full, &late), (6, &thin, &in_sync)]);
        assert_eq!(out[1], None);
    }

    #[test]
    fn a_track_follows_the_track_whose_verdict_places_its_twins_over_one_sharing_more() {
        let v = video();
        let full = signs(40, 0);
        let late = late_but_3_to_6(&full);
        // Track 7 has six of the signs from 3:00 to 6:00, in sync; track 5 has all of them, but
        // leaves them where they are.
        let few = full[18..24].to_vec();
        let in_sync = analysis(Verdict::InSync, vec![Some(0); 6]);
        let thin = full[18..36].to_vec();
        let unclear = analysis(Verdict::Unclear, vec![None; 18]);
        let out = lines_of(
            &v,
            &[(5, &full, &late), (6, &thin, &unclear), (7, &few, &in_sync)],
        );
        let Some(Lines::Follows(f)) = &out[1] else {
            panic!("{out:?}")
        };
        assert_eq!(
            (f.source, f.gap, f.shared.times, f.verdict.clone()),
            (7, 0, 12, Verdict::InSync)
        );
        // Nor does track 5's verdict disagree with track 7's: the lines they share are ones track
        // 5 leaves where they are.
        assert_eq!((&out[0], &out[2]), (&None, &None));
    }

    #[test]
    fn a_line_in_a_far_window_stays_where_it_is_though_the_source_places_its_twin() {
        let v = video();
        // A line ending the window before 3:00, a frame before it starts, which the source moves.
        let mut full = signs(20, 0);
        full.push(event("Edge", 179_960, 180_400));
        let late = late_but_3_to_6(&full);
        // This track has the signs, and the line a frame later, a frame into the far window.
        let mut thin = signs(20, 0);
        thin.push(event("Edge", 180_000, 180_440));
        let unclear = analysis(Verdict::Unclear, vec![None; 21]);
        let out = lines_of(&v, &[(5, &full, &late), (6, &thin, &unclear)]);
        let Some(Lines::Follows(f)) = &out[1] else {
            panic!("{out:?}")
        };
        assert_eq!((f.gap, f.verdict.clone()), (0, Verdict::Shifted(1)));
        let mut want = vec![Some(0); 21];
        want[18] = None;
        want[19] = None;
        want[20] = None;
        assert_eq!(f.each, want);
        assert_eq!(
            f.unshifted,
            [(180_000, 180_440), (180_500, 183_500), (190_500, 193_500)]
        );
    }

    #[test]
    fn a_line_is_in_a_far_window_by_its_time_in_the_source() {
        let v = judged();
        let full = signs(20, 0);
        let late = late_but_3_to_6(&full);
        // This track has the signs a second (25 frames) later, and two lines of its own: one at
        // 3:00.5, at 2:59.5 in the source's times, and one at 6:00.5, at 5:59.5 there.
        let mut thin = signs(20, 25);
        thin.push(event("Own", 180_500, 181_000));
        thin.push(event("Own", 360_500, 361_000));
        let unclear = analysis(Verdict::Unclear, vec![None; 22]);
        let out = lines_of(&v, &[(5, &full, &late), (6, &thin, &unclear)]);
        let Some(Lines::Follows(f)) = &out[1] else {
            panic!("{out:?}")
        };
        assert_eq!((f.gap, f.verdict.clone()), (25, Verdict::Shifted(26)));
        let mut want = vec![Some(0); 22];
        want[18] = None;
        want[19] = None;
        want[21] = None;
        assert_eq!(f.each, want);
        assert_eq!(
            f.unshifted,
            [(181_500, 184_500), (191_500, 194_500), (360_500, 361_000)]
        );
    }

    #[test]
    fn off_an_analysed_source_a_line_before_0_00_is_in_its_first_window() {
        let cuts = synthetic::cuts(500);
        let video = synthetic::yuru(&cuts);
        // A second late but for shots to 3:00, on the cuts.
        let full = synthetic::track_by(
            &cuts,
            |cut| {
                if synthetic::ms(cut) < 180_000 { 0 } else { 24 }
            },
        );
        let late = analyse(&video, &full);
        assert_eq!(
            (late.verdict.clone(), late.far.clone()),
            (Verdict::Shifted(24), vec![0])
        );
        // This track has the lines from 3:00 and one of its own before 0:00.
        let mut thin: Vec<Event> = full
            .iter()
            .filter(|e| e.start_ms >= 180_000)
            .cloned()
            .collect();
        let own = thin.len();
        thin.push(event("Own", -1000, -500));
        let unclear = analysis(Verdict::Unclear, vec![None; thin.len()]);
        let out = lines_of(&video, &[(5, &full, &late), (6, &thin, &unclear)]);
        let Some(Lines::Follows(f)) = &out[1] else {
            panic!("{out:?}")
        };
        assert_eq!(f.verdict, Verdict::Shifted(24));
        let mut want = vec![Some(0); thin.len()];
        want[own] = None;
        assert_eq!(f.each, want);
        assert_eq!(f.unshifted, [(-1000, -500)]);
    }

    #[test]
    fn a_line_at_the_end_of_time_is_in_the_last_window_by_the_sources_times_too() {
        // A line of its own at the end of i64 ms, 3 frames after its place in the source's times:
        // past the end of i64 there, and in the window holding the end of i64.
        let mine = [event("Own", i64::MAX - 10, i64::MAX)];
        let mut theirs = analysis(Verdict::Shifted(1), Vec::new());
        theirs.far = vec![crate::timing::window_of(i64::MAX)];
        let shared = Shared {
            times: 0,
            counts: Hits::new(),
        };
        assert_eq!(
            follow(&video(), 5, &mine, &theirs, &[None], -3, shared),
            None
        );
    }

    #[test]
    fn frames_at_either_end_of_time_are_far_apart_not_an_overflow() {
        // At 1 ns a frame, a line at the end of i64 ms is more frames in than an i64 counts.
        let video = Video::unjudged(1, (0..=10).map(|i| i * 1_000_000_000).collect());
        let mine = [event("Sign", -1000, -500)];
        let theirs = [event("Sign", i64::MAX, i64::MAX)];
        assert_eq!(twins(&video, &mine, &theirs), [None]);
        assert_eq!(shared(&video, &mine, &theirs, &[Some(0)]).times, 0);
        // Two such lines 20 s apart are as far apart as they are, not both at the last frame an
        // i64 counts.
        let late = [event("Sign", i64::MAX - 20_000, i64::MAX - 19_000)];
        assert_eq!(twins(&video, &theirs, &late), [None]);
    }
}
