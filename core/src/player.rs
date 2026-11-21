//! The player port: what review asks of a player, and where the player says it is. Review decides
//! what to play, and [`Effect::Player`](crate::review::update::Effect::Player) hands it to the UI.

/// A moment to open on a frame: a file, where to seek, and which subtitle track to show. Whether
/// mpv plays there is the player's: it keeps mpv playing or paused as it was, and paused when no
/// file is open, as mpv's own state says, which review does not see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moment {
    pub file: String,
    /// Where mpv seeks, in µs: a line's first frame needs a target between two milliseconds, as
    /// `PAST_A_LINE_US` says. Never below 0, which mpv counts back from the end.
    pub start_us: i64,
    /// mpv's `--sid`.
    pub sid: Option<usize>,
}

/// How far past a line's start mpv seeks. Its exact seek shows the first frame it decodes from 5 ms
/// before the target on (`player/video.c` in mpv's source), so a frame up to 5 ms before the line,
/// which shows none of it, would be the one shown. Half a millisecond short of that, the boundary
/// falls between the milliseconds Matroska stores frames and lines to, so a frame on the line's
/// start shows and one before it does not, with no time on the boundary for floating point to put
/// on the wrong side.
const PAST_A_LINE_US: i64 = 4_500;

/// Where mpv seeks, in µs, to show the first frame of a line that starts at `start_ms`.
pub fn line_start_us(start_ms: i64) -> i64 {
    start_ms.saturating_mul(1000).saturating_add(PAST_A_LINE_US)
}

/// The time, in ms, that a moment's `start_us` was made from: a line's carries the
/// `PAST_A_LINE_US` and a frame's, always whole milliseconds, does not.
pub fn start_ms(start_us: i64) -> i64 {
    if start_us % 1000 == 0 {
        start_us / 1000
    } else {
        (start_us - PAST_A_LINE_US) / 1000
    }
}

/// What a `Playback` action asks of mpv.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Playback {
    /// Pause, or resume.
    Pause,
    /// Seek this many seconds; back when negative.
    Seek(i32),
    /// One frame on, or back; mpv pauses there.
    NextFrame,
    PreviousFrame,
}

/// Where playback is, as mpv reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayState {
    pub paused: bool,
    pub at_ms: i64,
}

/// How a player reaches a moment: in the file its mpv has open, or by loading the file. Review
/// says which ([`PlayerCommand::Open`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    Seek,
    Load,
}

/// What review asks of the UI's player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerCommand {
    /// Open `moment`, in these steps, each once mpv has answered the one before; how a move
    /// lands is each player's. Whether mpv plays is read first, as the pause makes every mpv
    /// paused: it plays when it has a file open and its `pause` is off. mpv refusing the read of
    /// `path` counts as no file open, and refusing that of `pause`, or giving no flag, as paused;
    /// no answer at all fails the open.
    ///
    /// 1. mpv paused, so that a playing mpv does not go on past the frame while it gets there. A
    ///    refusal fails the open: mpv refuses no pause while it answers at all.
    /// 2. [`How::Seek`]: in the file mpv has open, the moment's subtitle track (`sid`, when it
    ///    names one) and subtitles on, then an exact seek to it, then pause off if mpv played.
    ///    With another file open or none, or with any of the first three refused, the player
    ///    loads the file instead. A refused unpause leaves mpv paused on the frame, still opened.
    ///    Review says seek only in a file it had loaded, so `options` hold for it still.
    /// 3. [`How::Load`], or in place of a seek: the file, with options for that file alone: its
    ///    start at the moment, the subtitle track and subtitles on, every one of `options`, and
    ///    pause as mpv is to end up; nothing after it. None of `options` names one the player
    ///    sets itself, so no player need keep their order.
    ///
    /// The player says whether mpv was left paused ([`Opened`]).
    Open {
        moment: Moment,
        how: How,
        options: Vec<FileOption>,
    },
    /// Pause or seek the open mpv.
    Playback(Playback),
    /// Close mpv, and leave review open.
    Close,
}

/// One of mpv's options for the one file a load opens, as `--name=value` names it. The name and
/// the value are plain words, lowercase letters, digits and hyphens: the window joins them into
/// one `name=value,…` argument, where a comma would split one and mpv reads a value that starts
/// with a quote, a bracket or a percent sign as quoted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOption {
    pub name: String,
    pub value: String,
}

/// The options review gives every file a player loads, beyond the moment's start and track,
/// subtitles on and the pause, which the player sets itself and which none of these names: here
/// once, so that no player holds a copy to drift from the others'. Each is that file's alone, set
/// as it starts, after `reset-on-next-file` in a user's config and the file before have put back
/// what they changed, so neither can undo it. `hr-seek=yes` lands on the frame where a user's
/// `hr-seek=no` would land on the keyframe before it, and makes mpv's own relative seeks exact
/// while the file is open. `rebase-start-time=no` keeps the file's own clock, which review's times
/// are on, where mpv would start it at its first cluster.
pub fn load_options() -> Vec<FileOption> {
    [("hr-seek", "yes"), ("rebase-start-time", "no")]
        .into_iter()
        .map(|(name, value)| FileOption {
            name: name.into(),
            value: value.into(),
        })
        .collect()
}

/// How an open went: where mpv was left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opened {
    pub paused: bool,
}

/// What went wrong with a player command, as the player says it. Review makes it its message
/// ([`Status`](crate::review::status::Status)), which each UI words; an error with text keeps the
/// words of whoever made it, mpv's, the OS's or the player's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerError {
    /// No mpv to ask, or nothing open in it to close.
    NotOpen,
    /// mpv has no file open.
    NoFile,
    /// mpv is still opening the file: it names it, but has no position yet.
    StillOpening,
    /// mpv could not open this file: its load ended on an error.
    CouldNotOpen { file: String },
    /// mpv did not answer a request within the player's time.
    NoAnswer,
    /// mpv closed the connection without answering.
    HungUp,
    /// The window's video pane could not show video within the player's time.
    NoVideo,
    /// mpv refused a request: its words.
    Mpv(String),
    /// Anything else: starting mpv, or reaching it.
    Failed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    // The players' own tests give them lists core never gives, so that a player that named these
    // itself would fail them; the pair every load gets is pinned here.
    #[test]
    fn every_load_lands_exactly_on_the_files_own_clock() {
        let options: Vec<(String, String)> = load_options()
            .into_iter()
            .map(|o| (o.name, o.value))
            .collect();
        assert_eq!(
            options,
            [
                ("hr-seek".to_string(), "yes".to_string()),
                ("rebase-start-time".to_string(), "no".to_string())
            ]
        );
    }

    // A name the player sets itself would be set twice, and only the later would hold; a word
    // that is not plain would split the window's one argument or be read by mpv as quoted.
    #[test]
    fn every_load_option_is_plain_words_and_none_the_players_own() {
        let plain = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        };
        for option in load_options() {
            assert!(plain(&option.name) && plain(&option.value), "{option:?}");
            assert!(
                !["start", "sid", "sub-visibility", "pause"].contains(&option.name.as_str()),
                "{option:?}"
            );
        }
    }

    // Times read from a corrupt file reach both ends of i64: they stay there, not wrap round.
    #[test]
    fn a_line_start_at_either_end_of_time_stays_there() {
        assert_eq!(line_start_us(i64::MAX), i64::MAX);
        assert_eq!(line_start_us(i64::MIN), i64::MIN + 4_500);
        assert_eq!(line_start_us(41_250), 41_254_500);
    }
}
