//! The invented episode the bench scans (`script.ass`), held to its answer key and to the fixture
//! muxed from it.

use std::collections::HashSet;

use crate::score::Key;

const SCRIPT: &str = include_str!("script.ass");
const ANSWERS: &str = include_str!("answers.toml");
/// What mkvextract makes of `bench.mkv`'s track; `oracle.rs` ties it to the MKV byte for byte.
const EXTRACTED: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/expected/bench.track0.ass"
));

/// The Text field of each `Dialogue:` line of an ASS script, in order: everything after the ninth
/// comma, since the Text field is the last and may hold commas of its own.
pub fn dialogue_texts(ass: &str) -> Vec<String> {
    dialogue_lines(ass)
        .into_iter()
        .filter_map(|line| line.splitn(10, ',').nth(9))
        .map(str::to_string)
        .collect()
}

fn dialogue_lines(ass: &str) -> Vec<&str> {
    ass.lines()
        .filter(|line| line.starts_with("Dialogue:"))
        .collect()
}

/// The key names lines by their text, as the LLM sees them: a line of the key with markup in it
/// could never match a finding.
#[test]
fn every_answer_is_the_text_of_one_line() {
    let key = Key::parse(ANSWERS).unwrap();
    let texts = dialogue_texts(SCRIPT);
    let lines = key
        .typos
        .iter()
        .map(|t| &t.line)
        .chain(key.traps.iter().map(|t| &t.line));
    for line in lines {
        let found = texts.iter().filter(|t| *t == line).count();
        assert_eq!(
            found, 1,
            "{line:?} is the text of {found} lines of script.ass"
        );
        assert!(
            !line.contains(['{', '\\']),
            "{line:?} holds markup, which the LLM never sees"
        );
    }
}

/// A finding and the key both name a line by its text alone, so two lines of one text could not be
/// told apart.
#[test]
fn no_two_lines_share_their_text() {
    let mut seen = HashSet::new();
    for text in dialogue_texts(SCRIPT) {
        assert!(
            seen.insert(text.clone()),
            "two lines of script.ass read {text:?}"
        );
    }
}

/// Every line, the header and the styles as well as the events: the bench scans bench.mkv, and
/// the key is checked against script.ass, so the two must be one script.
#[test]
fn the_fixture_holds_the_script_as_it_is() {
    // mkvextract may write a BOM or CRLF line ends, and neither changes a line of the script.
    let lines = |ass: &'static str| -> Vec<&'static str> {
        ass.strip_prefix('\u{feff}')
            .unwrap_or(ass)
            .lines()
            .collect()
    };
    let (script, extracted) = (lines(SCRIPT), lines(EXTRACTED));
    let differs =
        (0..script.len().max(extracted.len())).find(|&i| script.get(i) != extracted.get(i));
    if let Some(i) = differs {
        panic!(
            "script.ass changed since bench.mkv was made, from line {}: rebuild bench.mkv as \
             README.md's \"Measuring a model\" says",
            i + 1
        );
    }
}
