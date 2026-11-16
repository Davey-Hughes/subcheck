//! What a UI shows of the selected finding beside the list ([`Review::detail`]), as data: each UI
//! words it, the terminal in its detail pane, so that no UI shows a thing of it the others cannot.

use std::ops::Range;

use super::model::{Entry, FileNote, Fix, Marked, Review, changed};
use super::progress::Checking;
use crate::list::{self, TimingMoment};
use crate::mkv::{self, Truncated};
use crate::store::{State, TrackRecord};

/// The finding a UI shows beside the list: the one the list has selected, or the first its
/// selected row shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detail {
    /// Its file, as the store names it.
    pub file: String,
    /// The name the list gives that file.
    pub label: String,
    /// What the row is when the list has a row of several selected.
    pub group: Option<Group>,
    /// Where its line starts, on the occurrence the list goes by; none for a timing finding, whose
    /// fix is a whole track's.
    pub at_ms: Option<i64>,
    /// The track that occurrence is on, or a timing finding's track, when the record of its file
    /// names that track.
    pub track: Option<Track>,
    /// Its line's other occurrences; none are counted for a timing finding.
    pub more: usize,
    pub note: Option<FileNote>,
    /// Where the scan checking its file again is with that file.
    pub checking: Option<Checking>,
    /// The scan read a copy of its file that was cut short.
    pub truncated: Option<Truncated>,
    pub state: State,
    /// The wording is the user's, not the suggestion.
    pub edited: bool,
    pub mark: Option<Marked>,
    pub original: String,
    pub wording: String,
    /// The part of `original` that differs from `wording`, in characters; for a timing finding,
    /// all of it, as its two sides are descriptions, not lines of the file.
    pub original_changed: Range<usize>,
    /// The part of `wording` that differs from `original`, in characters; for a timing finding,
    /// all of it, as for `original_changed`.
    pub wording_changed: Range<usize>,
    /// Why it was found; empty when nothing says.
    pub reason: String,
    pub kind: Kind,
}

/// A row of several: the members the filter shows, in the row's order, and how many it hides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    /// Never empty: a row whose members the filter all hides is not in the list
    /// ([`Review::entries`]), so it is never selected and has no detail ([`Review::focus`]).
    pub members: Vec<Member>,
    pub hidden: usize,
}

/// A member of a row: the name its file goes by in the list, and a timing finding's fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub label: String,
    pub fix: Option<Fix>,
}

/// A track of a finding's file, as its record has it: mkvmerge's ID, mpv's `sid`, its own name if
/// it has one, and its language (the BCP 47 tag, when it has one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub id: usize,
    pub sid: usize,
    pub name: Option<String>,
    pub language: String,
}

impl Track {
    fn of(t: &TrackRecord) -> Track {
        Track {
            id: t.id,
            sid: t.sid,
            name: mkv::own_name(t.name.as_deref()).map(str::to_string),
            language: mkv::shown_language(&t.language, t.language_bcp47.as_deref()).to_string(),
        }
    }
}

/// What differs by kind of finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// A line: the lines around it, in time order.
    Line {
        before: Vec<String>,
        after: Vec<String>,
    },
    /// A track's timing: each stretch of it, the lines its fix leaves where they are, and where
    /// `Play` opens it.
    Timing {
        stretches: Vec<Stretch>,
        unshifted: Vec<Times>,
        opens: Option<TimingMoment>,
    },
}

/// Part of a track at one offset: how many frames late (early when negative; 0 in sync), from its
/// first line's start to its last line's end; none from the track's start, or to its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stretch {
    pub frames: i64,
    pub from_ms: Option<i64>,
    pub to_ms: Option<i64>,
}

/// When a line starts and ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Times {
    pub start_ms: i64,
    pub end_ms: i64,
}

impl Review {
    /// The selected entry's finding as a UI shows it beside the list ([`Review::focus`]); none
    /// with nothing selected. The findings list's selection, whichever list a UI shows.
    pub fn detail(&self) -> Option<Detail> {
        let i = self.focus()?;
        let entry = self.selected_entry()?;
        let f = self.finding(i);
        // A timing finding is a whole track's, not one line's.
        let (occurrence, track) = match &f.timing {
            Some(t) => (None, list::track_of(&self.run, f, t.track)),
            None => list::shown_at(&self.run, f),
        };
        let wording = self.wording(i);
        let (original_changed, wording_changed) = if f.timing.is_some() {
            (0..f.original.chars().count(), 0..wording.chars().count())
        } else {
            let (a, b) = changed(&f.original, wording);
            (chars(&f.original, &a), chars(wording, &b))
        };
        let kind = match &f.timing {
            Some(t) => Kind::Timing {
                stretches: t
                    .stretches
                    .iter()
                    .map(|s| Stretch {
                        frames: s.frames,
                        from_ms: s.from_ms.or(s.first.as_ref().map(|l| l.start_ms)),
                        to_ms: s.to_ms.or(s.last.as_ref().map(|l| l.end_ms)),
                    })
                    .collect(),
                unshifted: t
                    .unshifted
                    .iter()
                    .map(|l| Times {
                        start_ms: l.start_ms,
                        end_ms: l.end_ms,
                    })
                    .collect(),
                opens: list::timing_moment(&self.run, f),
            },
            None => Kind::Line {
                before: f.before.clone(),
                after: f.after.clone(),
            },
        };
        Some(Detail {
            file: f.file.clone(),
            label: self.label(&f.file),
            group: self.group(entry),
            at_ms: occurrence.map(|o| o.start_ms),
            track: track.map(Track::of),
            more: if f.timing.is_some() {
                0
            } else {
                f.occurrences.len().saturating_sub(1)
            },
            note: self.notes.get(&f.file).copied(),
            checking: self.progress.checking(&f.file),
            // The newest record, as `list::shown_at` reads the tracks.
            truncated: self
                .run
                .files
                .iter()
                .rev()
                .find(|r| r.path == f.file)
                .and_then(|r| r.truncated),
            state: self.state(i),
            edited: self
                .run
                .decisions
                .get(&f.id)
                .is_some_and(|d| d.edited.is_some()),
            mark: self.marked(i),
            original: f.original.clone(),
            wording: wording.to_string(),
            original_changed,
            wording_changed,
            reason: f.reason.clone(),
            kind,
        })
    }

    /// `entry`'s members when it is a row of several.
    fn group(&self, entry: Entry) -> Option<Group> {
        let Entry::Row(r) = entry else {
            return None;
        };
        let all = self.rows[r].members.len();
        if all < 2 {
            return None;
        }
        let shown = self.shown_members(r);
        Some(Group {
            hidden: all - shown.len(),
            members: shown
                .into_iter()
                .map(|j| {
                    let f = self.finding(j);
                    Member {
                        label: self.label(&f.file),
                        fix: Fix::of(f),
                    }
                })
                .collect(),
        })
    }
}

/// `bytes` of `s`, which start and end on its characters, as characters (Rust's `char`s).
fn chars(s: &str, bytes: &Range<usize>) -> Range<usize> {
    s[..bytes.start].chars().count()..s[..bytes.end].chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::list::Filter;
    use crate::review::model::Mark;
    use crate::review::model::tests_support::{file, path, sample, with_timing};
    use crate::review::update::tests_support::decide_as;
    use crate::scan;
    use crate::store::FileRecord;
    use crate::text::Occurrence;

    #[test]
    fn nothing_selected_has_no_detail() {
        let mut m = sample();
        m.filter = Filter::Only(State::Rejected);
        assert_eq!(m.detail(), None);
    }

    #[test]
    fn a_line_findings_detail_is_its_line_where_it_is_shown_and_the_lines_around_it() {
        let mut m = sample();
        // The snacks line, alone in its row, on two occurrences: the default track's first.
        m.selected = 1;
        m.run.findings[2].occurrences.insert(
            0,
            Occurrence {
                track: 6,
                start_ms: 3_000,
                end_ms: 5_000,
            },
        );
        let d = m.detail().unwrap();
        assert_eq!(d.file, path(2));
        assert_eq!(d.label, "E02");
        assert_eq!(d.group, None, "a row of one");
        assert_eq!(d.at_ms, Some(894_190));
        assert_eq!(
            d.track,
            Some(Track {
                id: 5,
                sid: 1,
                name: Some("English Full".into()),
                language: "eng".into(),
            })
        );
        assert_eq!(d.more, 1);
        // A name of blanks names nothing: the track goes by its language.
        m.run.files[1].tracks[0].name = Some(" \t".into());
        assert_eq!(m.detail().unwrap().track.unwrap().name, None);
        assert_eq!((d.note, d.checking, d.truncated), (None, None, None));
        assert_eq!((d.state, d.edited, d.mark), (State::Pending, false, None));
        assert_eq!(d.original, "Let's have snacks together everyday.");
        assert_eq!(d.wording, "Let's have snacks together every day.");
        // `everyday.` and `every day.`, after the words both start with.
        assert_eq!((d.original_changed, d.wording_changed), (27..36, 27..37));
        assert_eq!(d.reason, "\"everyday\" should be \"every day\"");
        assert_eq!(
            d.kind,
            Kind::Line {
                before: vec!["The line before.".into()],
                after: vec!["The line after.".into()],
            }
        );
    }

    #[test]
    fn a_rows_detail_names_the_members_the_filter_shows_and_counts_those_it_hides() {
        let mut m = sample();
        decide_as(&mut m, &[2], State::Accepted, None);
        m.filter = Filter::Only(State::Pending);
        let d = m.detail().unwrap();
        let member = |label: &str| Member {
            label: label.into(),
            fix: None,
        };
        assert_eq!(
            d.group,
            Some(Group {
                members: vec![member("E01"), member("E03")],
                hidden: 1,
            })
        );
        // A member of it has no group of its own.
        m.expanded.insert(0);
        m.selected = 1;
        assert_eq!(m.detail().unwrap().group, None);
    }

    #[test]
    fn a_timing_findings_detail_is_its_track_its_stretches_and_where_play_opens_it() {
        let mut m = with_timing();
        m.selected = 1;
        let d = m.detail().unwrap();
        assert_eq!(
            d.group.unwrap().members,
            [
                Member {
                    label: "E01".into(),
                    fix: Some(Fix::Shift(1)),
                },
                Member {
                    label: "E02".into(),
                    fix: Some(Fix::Partial),
                },
                Member {
                    label: "E03".into(),
                    fix: Some(Fix::Shift(1)),
                },
            ]
        );
        assert_eq!(d.at_ms, None, "the whole track");
        assert_eq!(d.track.map(|t| (t.id, t.sid)), Some((5, 1)));
        assert_eq!((d.original_changed, d.wording_changed), (0..12, 0..21));
        // The partial one: its stretches from their lines where they name no time, the line it
        // leaves where it is, and the frame that shows the first stretch's error.
        m.expanded.insert(1);
        m.selected = 3;
        let d = m.detail().unwrap();
        // Each side whole, in characters: the wording's dashes take more than a byte each.
        assert_eq!((d.original_changed, d.wording_changed), (0..44, 0..59));
        assert_eq!(d.more, 0, "its two occurrences are its evidence");
        let Kind::Timing {
            stretches,
            unshifted,
            opens,
        } = d.kind
        else {
            panic!("not a timing finding");
        };
        let stretch = |frames, from_ms, to_ms| Stretch {
            frames,
            from_ms,
            to_ms,
        };
        assert_eq!(
            stretches,
            [
                stretch(1, Some(1000), Some(475_780)),
                stretch(0, Some(483_510), Some(939_000)),
                stretch(1, Some(940_470), Some(1_402_000)),
            ]
        );
        assert_eq!(
            unshifted,
            [Times {
                start_ms: 1_300_000,
                end_ms: 1_302_000,
            }]
        );
        assert_eq!(
            opens,
            Some(TimingMoment {
                at_ms: 246_788,
                sid: Some(1),
                on_frame: true,
            })
        );
    }

    #[test]
    fn a_detail_says_what_is_under_way_with_its_file_and_what_was_decided() {
        let mut m = sample();
        m.notes.insert(path(1), FileNote::Missing);
        m.progress.start("r", "subcheck scan --run r".into(), false);
        m.progress
            .scan_event(&scan::Event::Queued(vec![path(1).into()]));
        m.progress.scan_event(&scan::Event::Asking {
            path: path(1).into(),
            answered: 1,
            chunks: 3,
        });
        let cut = Truncated {
            declared_end: 2000,
            file_size: 1000,
        };
        // Its file's newest record cut short: the one before, read whole, is not what it is now.
        m.run.files.push(FileRecord {
            truncated: Some(cut),
            ..file(1)
        });
        decide_as(&mut m, &[1], State::Accepted, Some("My own."));
        m.marks.insert(1, Mark::ByHand("a karaoke row".into()));
        let d = m.detail().unwrap();
        assert_eq!(d.note, Some(FileNote::Missing));
        assert_eq!(
            d.checking,
            Some(Checking::Asking {
                answered: 1,
                chunks: 3,
            })
        );
        assert_eq!(d.truncated, Some(cut));
        assert_eq!((d.state, d.edited), (State::Accepted, true));
        assert_eq!(d.wording, "My own.");
        assert_eq!(
            d.mark,
            Some(Marked::Said(Mark::ByHand("a karaoke row".into())))
        );
        // Its fix in its file, and nothing apply said since: applied.
        m.marks.clear();
        m.run.applied.insert(1);
        assert_eq!(m.detail().unwrap().mark, Some(Marked::Applied));
    }

    #[test]
    fn what_changed_is_counted_in_characters_of_each_wording() {
        let mut m = sample();
        let f = &mut m.run.findings[0];
        f.original = "Ça va trés bien, 字幕.".into();
        f.suggestion = "Ça va très bien, 字幕.".into();
        let d = m.detail().unwrap();
        // `trés` and `très`, after six characters, of which `Ç` is two bytes.
        assert_eq!((d.original_changed, d.wording_changed), (6..10, 6..10));
    }
}
