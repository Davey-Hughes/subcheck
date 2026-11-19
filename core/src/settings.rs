//! The settings file: the LLMs a user defines for `--llm` to pick by name, each a command run as
//! `--llm-cmd`'s is, and which LLM `--llm` means when it is left out. It is `config.toml` in
//! subcheck's config directory, unless `SUBCHECK_CONFIG` names another file ([`config_path`]).
//! No file is the same as an empty one.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::llm::{Backend, Output};

/// Where the settings file is, from explicit inputs: `SUBCHECK_CONFIG` (`config`) when it is set
/// and not empty, which must be absolute, as a relative one would resolve against the cwd; else
/// `subcheck/config.toml` in the config directory: `XDG_CONFIG_HOME` when it is absolute (empty
/// or relative is ignored, as the XDG spec says), else `~/Library/Application Support` on macOS,
/// beside the store, else `~/.config`. `HOME` is needed only for those last two.
///
/// An absolute `XDG_CONFIG_HOME` wins on macOS too, as `XDG_STATE_HOME` does for the store
/// ([`crate::store::state_dir_from`]): macOS has no such convention, so one that is set was set
/// on purpose.
pub fn config_path_from(
    config: Option<&Path>,
    home: Option<&Path>,
    xdg_config: Option<&Path>,
    macos: bool,
) -> Result<PathBuf, String> {
    if let Some(config) = config.filter(|p| !p.as_os_str().is_empty()) {
        if !config.is_absolute() {
            return Err("SUBCHECK_CONFIG must be an absolute path".into());
        }
        return Ok(config.to_path_buf());
    }
    let home = home.filter(|h| h.is_absolute());
    let dir = match (xdg_config.filter(|p| p.is_absolute()), home) {
        (Some(xdg), _) => xdg.to_path_buf(),
        (None, Some(home)) if macos => home.join("Library/Application Support"),
        (None, Some(home)) => home.join(".config"),
        (None, None) => return Err("HOME is not set to an absolute path".into()),
    };
    Ok(dir.join("subcheck/config.toml"))
}

/// Where this process's settings file is.
pub fn config_path() -> Result<PathBuf, String> {
    let var = |name| std::env::var_os(name).map(PathBuf::from);
    config_path_from(
        var("SUBCHECK_CONFIG").as_deref(),
        var("HOME").as_deref(),
        var("XDG_CONFIG_HOME").as_deref(),
        cfg!(target_os = "macos"),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Where the file is, or would be when there is none: what a message about it names.
    pub path: PathBuf,
    /// What `--llm` is when it is left out. Anything `--llm` takes, a preset as well as an entry,
    /// so it is not checked against `entries` here.
    pub default: Option<String>,
    /// The `[llm.<name>]` tables, by name.
    pub entries: BTreeMap<String, Entry>,
}

/// An `[llm.<name>]` table: a command that reads the prompt on stdin and prints the reply on
/// stdout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub command: String,
    /// `Text` when the file does not say.
    pub output: Output,
    /// Calls in flight to this LLM at once; `None`, when the file does not say, leaves the default
    /// to the caller.
    pub jobs: Option<usize>,
    /// Lines per call; `None` as for `jobs`.
    pub chunk_lines: Option<usize>,
}

/// The file as written. Unknown keys are refused here and in [`EntryShape`], so a misspelt key is
/// an error rather than a setting that silently does nothing.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileShape {
    default: Option<String>,
    /// Values, read into entries one at a time, so that a mistake in one is reported with its
    /// name, and a name subcheck refuses is reported as that before anything about its keys.
    #[serde(default)]
    llm: BTreeMap<String, toml::Value>,
}

/// An entry as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryShape {
    command: String,
    #[serde(default)]
    output: OutputShape,
    jobs: Option<usize>,
    chunk_lines: Option<usize>,
}

/// `output`'s values, as the file spells them.
#[derive(Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum OutputShape {
    #[default]
    Text,
    OpenAi,
    Claude,
    Codex,
}

impl From<OutputShape> for Output {
    fn from(output: OutputShape) -> Output {
        match output {
            OutputShape::Text => Output::Text,
            OutputShape::OpenAi => Output::OpenAi,
            OutputShape::Claude => Output::Claude,
            OutputShape::Codex => Output::Codex,
        }
    }
}

impl Settings {
    /// This process's settings file ([`config_path`]), read.
    pub fn load() -> Result<Settings, String> {
        Settings::read(&config_path()?)
    }

    /// The settings file at `path`; no file there reads as an empty one.
    pub fn read(path: &Path) -> Result<Settings, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Settings::parse(path, &text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Settings::parse(path, ""),
            Err(e) => Err(format!("cannot read {}: {e}", path.display())),
        }
    }

    /// The settings in `text`, the contents of the file at `path`. Each error is one line, as the
    /// review screen's status line is one row: `path`, then the entry when the mistake is in one,
    /// else the line it is on when toml says.
    pub fn parse(path: &Path, text: &str) -> Result<Settings, String> {
        let file: FileShape = toml::from_str(text).map_err(|e| match e.span() {
            // The span is a byte offset into `text`.
            Some(span) => {
                let before = text.bytes().take(span.start);
                let line = before.filter(|&b| b == b'\n').count() + 1;
                format!("{}: line {line}: {}", path.display(), one_line(e))
            }
            None => format!("{}: {}", path.display(), one_line(e)),
        })?;
        let mut entries = BTreeMap::new();
        for (name, value) in file.llm {
            // A name is what `--llm` takes and a run records. TOML's bare-key characters, so every
            // entry is written `[llm.<name>]` without quotes; with no `:` or `@`, a name never
            // reads as `ollama:<model>`, `cmd:<command>` or another recorded label. Debug-quoted
            // in the message, since a name refused here can be empty or hold a space.
            let allowed = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
            if name.is_empty() || !name.chars().all(allowed) {
                return Err(format!(
                    "{}: [llm.{name:?}]: an entry's name is letters, digits, - and _",
                    path.display()
                ));
            }
            let at = |e: &str| format!("{}: [llm.{name}]: {e}", path.display());
            // `--llm` looks up the presets before the file's entries, so an entry with a preset's
            // name could never be picked.
            if Backend::preset(&name).is_ok() {
                return Err(at(&format!(
                    "{name} is subcheck's own; name the entry something else"
                )));
            }
            // serde's derived structs read an array too, as the keys in their order; and
            // `[[llm.<name>]]` is an array of tables.
            if !value.is_table() {
                return Err(at(&format!(
                    "an entry is a table: [llm.{name}] then its keys"
                )));
            }
            let shape: EntryShape = value.try_into().map_err(|e| at(&one_line(e)))?;
            if shape.jobs == Some(0) {
                return Err(at("jobs must be at least 1"));
            }
            if shape.chunk_lines == Some(0) {
                return Err(at("chunk_lines must be at least 1"));
            }
            let entry = Entry {
                command: shape.command,
                output: shape.output.into(),
                jobs: shape.jobs,
                chunk_lines: shape.chunk_lines,
            };
            entries.insert(name, entry);
        }
        Ok(Settings {
            path: path.to_path_buf(),
            default: file.default,
            entries,
        })
    }
}

/// toml's error as one line. Given the input, toml writes the line the mistake is on and a diagram
/// of it above the message, where [`Settings::parse`] names the line itself; without it, toml
/// writes the message, then the key the mistake is in, when it knows it, on a line of its own.
fn one_line(mut e: toml::de::Error) -> String {
    e.set_input(None);
    e.to_string().trim_end().replace('\n', ", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Result<Settings, String> {
        Settings::parse(Path::new("/c/config.toml"), text)
    }

    #[test]
    fn the_file_is_where_subcheck_config_or_xdg_config_home_says_else_under_home() {
        let (home, x) = (Some(Path::new("/h")), Some(Path::new("/x")));
        let p = |s: &str| Ok(PathBuf::from(s));
        assert_eq!(
            config_path_from(Some(Path::new("/s/c.toml")), home, x, false),
            p("/s/c.toml")
        );
        assert_eq!(
            config_path_from(Some(Path::new("c.toml")), home, x, false),
            Err("SUBCHECK_CONFIG must be an absolute path".into())
        );
        assert_eq!(
            config_path_from(Some(Path::new("")), home, x, false),
            p("/x/subcheck/config.toml")
        );
        assert_eq!(
            config_path_from(None, home, x, true),
            p("/x/subcheck/config.toml")
        );
        for ignored in ["", "rel"] {
            assert_eq!(
                config_path_from(None, home, Some(Path::new(ignored)), false),
                p("/h/.config/subcheck/config.toml")
            );
        }
        assert_eq!(
            config_path_from(None, home, None, true),
            p("/h/Library/Application Support/subcheck/config.toml")
        );
        assert_eq!(
            config_path_from(None, None, None, false),
            Err("HOME is not set to an absolute path".into())
        );
        assert_eq!(
            config_path_from(None, Some(Path::new("h")), None, false),
            Err("HOME is not set to an absolute path".into())
        );
    }

    #[test]
    fn no_file_or_one_with_nothing_in_it_has_no_entries_and_no_default() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("config.toml");
        let s = Settings::read(&missing).unwrap();
        assert_eq!((s.path, s.default, s.entries.len()), (missing, None, 0));
        for text in ["", "# nothing yet\n", "[llm]\n"] {
            let s = at(text).unwrap();
            assert_eq!((s.default, s.entries.len()), (None, 0), "{text:?}");
        }
        assert!(
            Settings::read(dir.path())
                .unwrap_err()
                .starts_with("cannot read ")
        );
    }

    #[test]
    fn a_command_entry_reads_every_key_and_defaults_the_rest() {
        let s = at("default = \"mine\"\n[llm.mine]\ncommand = \"my-llm --fast\"\noutput = \"openai\"\njobs = 4\nchunk_lines = 200\n").unwrap();
        assert_eq!(s.default.as_deref(), Some("mine"));
        assert_eq!(
            s.entries["mine"],
            Entry {
                command: "my-llm --fast".into(),
                output: Output::OpenAi,
                jobs: Some(4),
                chunk_lines: Some(200)
            }
        );
        assert_eq!(
            at("[llm.e]\ncommand = \"x\"\n").unwrap().entries["e"],
            Entry {
                command: "x".into(),
                output: Output::Text,
                jobs: None,
                chunk_lines: None
            }
        );
        for (name, output) in [
            ("text", Output::Text),
            ("openai", Output::OpenAi),
            ("claude", Output::Claude),
            ("codex", Output::Codex),
        ] {
            let s = at(&format!("[llm.e]\ncommand = \"x\"\noutput = \"{name}\"\n")).unwrap();
            assert_eq!(s.entries["e"].output, output, "{name}");
        }
        // Names: letters, digits, - and _; `ollama` alone is an entry, `ollama:<model>` the preset.
        assert!(at("[llm.My_llm-2]\ncommand = \"x\"\n[llm.ollama]\ncommand = \"x\"\n").is_ok());
    }

    #[test]
    fn every_mistake_is_one_line_naming_the_file_then_the_entry_or_the_line() {
        let err = |text: &str| at(text).unwrap_err();
        let has = |text: &str, start: &str, word: &str| {
            let e = err(text);
            assert!(
                e.starts_with(start) && e.contains(word) && !e.contains('\n'),
                "{text:?} gave {e:?}"
            );
        };
        has(
            "[llm.mine]\ncomand = \"x\"\n",
            "/c/config.toml: [llm.mine]: ",
            "comand",
        );
        has(
            "[llm.mine]\noutput = \"text\"\n",
            "/c/config.toml: [llm.mine]: ",
            "command",
        );
        has(
            "[llm.mine]\ncommand = \"x\"\noutput = \"json\"\n",
            "/c/config.toml: [llm.mine]: ",
            "json",
        );
        has(
            "[llm.mine]\ncommand = \"x\"\njobs = \"x\"\n",
            "/c/config.toml: [llm.mine]: ",
            "`jobs`",
        );
        has(
            "[llm.a.b]\ncommand = \"x\"\n",
            "/c/config.toml: [llm.a]: ",
            "`b`",
        );
        for text in [
            "[llm]\nmine = \"x\"\n",
            "[llm]\nmine = ['cmd', \"text\", 2, 400]\n",
            "[[llm.mine]]\ncommand = \"x\"\n",
        ] {
            assert_eq!(
                err(text),
                "/c/config.toml: [llm.mine]: an entry is a table: [llm.mine] then its keys",
                "{text:?}"
            );
        }
        // A mistake outside an entry: the line it is on, counted from the file's start.
        has("defualt = \"x\"\n", "/c/config.toml: line 1: ", "`defualt`");
        has("default = 2\n", "/c/config.toml: line 1: ", "`default`");
        has(
            "default = \"x\"\n\nllm = \"x\"\n",
            "/c/config.toml: line 3: ",
            "`llm`",
        );
        has("# settings\n[llm\n", "/c/config.toml: line 2: ", "");
        assert_eq!(
            err("[llm.mine]\ncommand = \"x\"\njobs = 0\n"),
            "/c/config.toml: [llm.mine]: jobs must be at least 1"
        );
        assert_eq!(
            err("[llm.mine]\ncommand = \"x\"\nchunk_lines = 0\n"),
            "/c/config.toml: [llm.mine]: chunk_lines must be at least 1"
        );
        for name in ["my llm", "a.b", ""] {
            assert_eq!(
                err(&format!("[llm.\"{name}\"]\ncommand = \"x\"\n")),
                format!(
                    "/c/config.toml: [llm.{name:?}]: an entry's name is letters, digits, - and _"
                )
            );
        }
        for name in ["claude", "gpt"] {
            assert_eq!(
                err(&format!("[llm.{name}]\ncommand = \"x\"\n")),
                format!(
                    "/c/config.toml: [llm.{name}]: {name} is subcheck's own; name the entry something else"
                )
            );
        }
    }
}
