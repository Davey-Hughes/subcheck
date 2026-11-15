//! The player port: what review asks of a player, and where the player says it is. Review decides
//! what to play, and [`Effect::Player`](crate::review::update::Effect::Player) hands it to the UI.

/// A moment to open on a frame: a file, where to seek, and which subtitle track to show. Whether
/// mpv plays there is the player's: it keeps mpv playing or paused as it was, and paused when no
/// file is open, as mpv's own state says, which review does not see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moment {
    pub file: String,
    /// Where mpv seeks, in µs: a line's first frame needs a target between two milliseconds, as
    /// `PAST_A_LINE_US` says.
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

/// What a playback key asks of mpv.
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

/// What review asks of the UI's player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerCommand {
    Open(Moment),
    /// Pause or seek the open mpv.
    Playback(Playback),
    /// Close mpv, and leave review open.
    Close,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Times read from a corrupt file reach both ends of i64: they stay there, not wrap round.
    #[test]
    fn a_line_start_at_either_end_of_time_stays_there() {
        assert_eq!(line_start_us(i64::MAX), i64::MAX);
        assert_eq!(line_start_us(i64::MIN), i64::MIN + 4_500);
        assert_eq!(line_start_us(41_250), 41_254_500);
    }
}
