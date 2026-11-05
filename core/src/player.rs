//! The player port: what review asks of a player, and where the player says it is. Review decides
//! what to play, and [`Effect::Player`](crate::review::update::Effect::Player) hands it to the UI.

/// A moment to open: a file, where to start, and which subtitle track to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moment {
    pub file: String,
    pub start_ms: i64,
    /// mpv's `--sid`.
    pub sid: Option<usize>,
    /// Stay on the frame at `start_ms` (a timing finding's), rather than play from it (a line's).
    pub paused: bool,
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
