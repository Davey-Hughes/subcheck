//! The answer key a bench episode ships with, and how an LLM's findings are scored against it.

use std::collections::HashSet;

use serde::Deserialize;
use subcheck_core::store::Finding;

/// A line with a mistake in it, and the fixes that count as right.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Typo {
    pub kind: String,
    pub line: String,
    pub fixed: Vec<String>,
}

/// A line that is correct as written, though it looks like a mistake.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trap {
    pub kind: String,
    pub line: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Key {
    #[serde(default, rename = "typo")]
    pub typos: Vec<Typo>,
    #[serde(default, rename = "trap")]
    pub traps: Vec<Trap>,
}

impl Key {
    /// Reads a key, refusing one the scoring could not make sense of: a line two entries share
    /// would be scored twice, and a fix that changes nothing could never be told from no finding.
    pub fn parse(text: &str) -> Result<Key, String> {
        let key: Key = toml::from_str(text).map_err(|e| format!("the answer key: {e}"))?;
        let mut seen = HashSet::new();
        let lines = key
            .typos
            .iter()
            .map(|t| &t.line)
            .chain(key.traps.iter().map(|t| &t.line));
        for line in lines {
            if line.trim().is_empty() {
                return Err(format!(
                    "the answer key: an entry has an empty line ({line:?})"
                ));
            }
            if !seen.insert(line.trim()) {
                return Err(format!("the answer key: line {line:?} named twice"));
            }
        }
        for typo in &key.typos {
            if typo.fixed.is_empty() {
                return Err(format!("the answer key: typo {:?} has no fix", typo.line));
            }
            if typo.fixed.iter().any(|f| f.trim() == typo.line.trim()) {
                return Err(format!(
                    "the answer key: typo {:?} lists a fix that changes nothing",
                    typo.line
                ));
            }
        }
        Ok(key)
    }
}

/// One finding as the bench shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Flag {
    pub line: String,
    pub suggestion: String,
    pub reason: String,
}

impl Flag {
    fn of(finding: &Finding) -> Flag {
        Flag {
            line: finding.original.clone(),
            suggestion: finding.suggestion.clone(),
            reason: finding.reason.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Score {
    /// Typos fixed as the key lists.
    pub right: Vec<Typo>,
    /// Typos the LLM found but changed some other way, with its first finding on the line.
    pub other_fix: Vec<(Typo, Flag)>,
    pub missed: Vec<Typo>,
    /// Traps flagged, with the first finding on the line.
    pub false_alarms: Vec<(Trap, Flag)>,
    /// Findings on lines the key does not name.
    pub other: Vec<Flag>,
}

/// Scores the LLM's findings (a timing finding is not the LLM's) against `key`.
pub fn score(key: &Key, findings: &[Finding]) -> Score {
    let llm: Vec<&Finding> = findings.iter().filter(|f| f.source == "llm").collect();
    let on = |line: &str| -> Vec<&Finding> {
        llm.iter()
            .copied()
            .filter(|f| f.original.trim() == line.trim())
            .collect()
    };

    let mut score = Score {
        right: Vec::new(),
        other_fix: Vec::new(),
        missed: Vec::new(),
        false_alarms: Vec::new(),
        other: Vec::new(),
    };
    for typo in &key.typos {
        let hits = on(&typo.line);
        let right = hits.iter().any(|f| {
            typo.fixed
                .iter()
                .any(|fix| fix.trim() == f.suggestion.trim())
        });
        if right {
            score.right.push(typo.clone());
        } else if let Some(first) = hits.first() {
            score.other_fix.push((typo.clone(), Flag::of(first)));
        } else {
            score.missed.push(typo.clone());
        }
    }
    for trap in &key.traps {
        if let Some(first) = on(&trap.line).first() {
            score.false_alarms.push((trap.clone(), Flag::of(first)));
        }
    }
    let named = |line: &str| {
        let line = line.trim();
        key.typos.iter().any(|t| t.line.trim() == line)
            || key.traps.iter().any(|t| t.line.trim() == line)
    };
    score.other = llm
        .iter()
        .filter(|f| !named(&f.original))
        .map(|f| Flag::of(f))
        .collect();
    score
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = r#"
[[typo]]
kind = "misspelling"
line = "We leave befor dawn."
fixed = ["We leave before dawn."]

[[typo]]
kind = "apostrophe"
line = "Thats mine."
fixed = ["That's mine.", "That is mine."]

[[trap]]
kind = "honorific"
line = "Morning, Aoi-senpai!"
"#;

    /// An LLM finding on `line` with the fix `suggestion`.
    fn found(line: &str, suggestion: &str) -> Finding {
        Finding {
            id: 0,
            file: String::new(),
            style: "Default".into(),
            original: line.into(),
            suggestion: suggestion.into(),
            reason: "why".into(),
            source: "llm".into(),
            occurrences: Vec::new(),
            before: Vec::new(),
            after: Vec::new(),
            timing: None,
        }
    }

    fn key() -> Key {
        Key::parse(KEY).unwrap()
    }

    fn flag(line: &str, suggestion: &str) -> Flag {
        Flag {
            line: line.into(),
            suggestion: suggestion.into(),
            reason: "why".into(),
        }
    }

    #[test]
    fn a_typo_fixed_as_the_key_says_is_right() {
        let key = key();
        let got = score(
            &key,
            &[found("We leave befor dawn.", "We leave before dawn.")],
        );
        assert_eq!(got.right, vec![key.typos[0].clone()]);
        assert_eq!(got.missed, vec![key.typos[1].clone()]);
        assert_eq!(got.other_fix, Vec::new());
        assert_eq!(got.false_alarms, Vec::new());
        assert_eq!(got.other, Vec::new());
    }

    #[test]
    fn any_listed_fix_is_right_whitespace_at_the_ends_aside() {
        let got = score(&key(), &[found("Thats mine.", " That is mine. ")]);
        assert_eq!(got.right.len(), 1);
    }

    #[test]
    fn another_change_on_a_typo_line_is_another_fix() {
        let key = key();
        let got = score(&key, &[found("Thats mine.", "That\u{2019}s mine.")]);
        assert_eq!(
            got.other_fix,
            vec![(
                key.typos[1].clone(),
                flag("Thats mine.", "That\u{2019}s mine.")
            )]
        );
        assert!(!got.right.contains(&key.typos[1]));
        assert!(!got.missed.contains(&key.typos[1]));
    }

    #[test]
    fn a_right_fix_beside_another_counts_right_alone() {
        let got = score(
            &key(),
            &[
                found("Thats mine.", "That's mine."),
                found("Thats mine.", "That's mine!"),
            ],
        );
        assert_eq!(got.right.len(), 1);
        assert_eq!(got.other_fix, Vec::new());
    }

    #[test]
    fn a_trap_flagged_twice_is_one_false_alarm() {
        let key = key();
        let got = score(
            &key,
            &[
                found("Morning, Aoi-senpai!", "first"),
                found("Morning, Aoi-senpai!", "second"),
            ],
        );
        assert_eq!(
            got.false_alarms,
            vec![(key.traps[0].clone(), flag("Morning, Aoi-senpai!", "first"))]
        );
    }

    #[test]
    fn a_finding_on_a_line_the_key_does_not_name_is_other() {
        let got = score(&key(), &[found("Plain line.", "Plain line!")]);
        assert_eq!(got.other.len(), 1);
    }

    #[test]
    fn timing_findings_are_not_scored() {
        let mut finding = found("Morning, Aoi-senpai!", "x");
        finding.source = "timing".into();
        let got = score(&key(), &[finding]);
        assert_eq!(got.false_alarms, Vec::new());
        assert_eq!(got.other, Vec::new());
    }

    #[test]
    fn a_line_named_twice_is_refused() {
        let text = format!("{KEY}\n[[trap]]\nkind = \"x\"\nline = \"Thats mine.\"\n");
        let err = Key::parse(&text).unwrap_err();
        assert!(err.contains("\"Thats mine.\""), "{err}");
        assert!(err.contains("twice"), "{err}");
    }

    #[test]
    fn a_fix_that_changes_nothing_is_refused() {
        let text = "[[typo]]\nkind = \"x\"\nline = \"Same line.\"\nfixed = [\"Same line.\"]\n";
        let err = Key::parse(text).unwrap_err();
        assert!(err.contains("\"Same line.\""), "{err}");
    }

    #[test]
    fn a_typo_with_no_fix_is_refused() {
        let text = "[[typo]]\nkind = \"x\"\nline = \"Some line.\"\nfixed = []\n";
        let err = Key::parse(text).unwrap_err();
        assert!(err.contains("\"Some line.\""), "{err}");
    }

    #[test]
    fn an_unknown_field_is_refused() {
        let text = "[[typo]]\nkind = \"x\"\nline = \"Some line.\"\nfix = [\"x\"]\n";
        let err = Key::parse(text).unwrap_err();
        assert!(err.contains("fix"), "{err}");
    }
}
