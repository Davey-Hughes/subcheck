//! subcheck's terminal UI: review's screen ([`tui`]) and the mpv it opens a finding in ([`mpv`]).
//! A library beside the `subcheck` binary, so that the integration tests can reach them.

pub mod mpv;
pub mod tui;
