# subcheck

Proofread the text subtitles embedded in MKV files with an LLM, then review the fixes.

- `subcheck extract` writes a subtitle track out the way mkvextract does, reading only the
  subtitle packets (through the file's Cues) instead of the whole file.
- `subcheck scan` sends the English lines to an LLM and records each suggested fix in its database
  ([Where findings are kept](#where-findings-are-kept)), with each track `timing` finds shifted,
  and what each LLM call used ([LLM costs](#llm-costs)). `--llm` picks the LLM: `claude` (the
  default), `gpt`, `ollama:<model>`, or an entry in the [settings file](#llm-settings).
  `ollama:<model>` calls Ollama's server, not the `ollama` command: the server at `OLLAMA_HOST`
  (`127.0.0.1:11434` when unset), with the model pulled (`ollama pull <model>`). A file unchanged
  since any earlier scan checked it the same way is skipped; `--force` checks it again.
  `--run <run>` adds the files to that run, making only the checks they need. On a terminal it opens
  review's screen as it starts, and each file's findings join the list when the file is done
  ([Review](#review)); `--no-tui` prints its log instead. `--karaoke` makes the screen's apply
  re-space per-letter karaoke as `apply --karaoke` does.
- `subcheck review` steps through a run's fixes, or those of the files under some paths, to
  accept, reject or reword each one. With no argument it opens the folders that hold the newest
  run's files.
- `subcheck list` prints a run's fixes (or those of the files under some paths) as text, under
  what their LLM calls cost; `--mpv` adds a command that opens each one.
- `subcheck apply` makes a run's accepted fixes in the MKV files, keeping each original as
  `<name>.mkv.orig`: a file's typos and timing fixes in one remux, each shifted line moved by whole
  frames on the video's frame grid. Needs mkvtoolnix.
- `subcheck timing` reports subtitle tracks shifted by whole frames against the video, up to 10 s
  either way (3 frames for a clip of 100 keyframes or fewer), with the `mkvmerge --sync` that fixes
  a whole-track shift. Keyframes stand in for cuts; those an encoder forces once a GOP runs long
  are left out when looking for a shift, and a track whose times point two ways reads `unclear`
  and says where. A track with too few lines on cuts is timed by the lines it shares with another
  track. No LLM, nothing recorded.
- `subcheck audio` reports whether audio tracks holding the same sound in different formats (a
  TrueHD and its AC-3, a FLAC and an AAC of it) line up, with the `mkvmerge --sync` that fixes a
  steady lag. Needs ffmpeg; nothing recorded.

## Install

```sh
cargo build --release
ln -s "$PWD/target/release/subcheck" ~/.local/bin/subcheck   # any directory on your PATH
```

The link follows the build, so a later `cargo build --release` updates the command.

To install a copy instead, `cargo install --locked --path cli` builds it into cargo's bin directory
(`~/.cargo/bin` by default); run it again after each update.

Building needs a C compiler (SQLite is compiled in). At run time:

| Needs | For |
|---|---|
| `claude` or `codex`, Ollama's server with the model pulled (for `ollama:<model>`), or a server or command of your own (`--llm-cmd`, or an entry in the [settings file](#llm-settings)) | `scan` |
| `mpv` | `o` in review |
| mkvtoolnix | `apply` |
| ffmpeg | `audio` |

## Development

`dev/check-all.sh` runs what CI gates on (rustfmt, clippy, the tests), with the `apply` and `audio`
tests failing rather than skipping when mkvmerge or ffmpeg is missing. Run it before pushing a
branch. The tests run under [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov), and the
gate fails when they reach too few of the lines. So the script needs llvm-tools (`rustup component
add llvm-tools`) and the cargo-llvm-cov version CI installs, and stops first, naming it, when either
is missing. The tests read small MKVs committed under `core/tests/fixtures/`, with
mkvextract's output for each as the oracle; `dev/make-fixtures.sh` rebuilds them, needed only to
add or change one.

`dev/check-gui.sh` does the same for the Qt app under `gui/qt/`, whose CI job does not gate yet:
clang-format, then a CMake build that generates the C++ bindings to `ffi/`, qmllint over the QML,
the app's tests, and the share of its C++ lines they reach, against CI's floor. Besides Rust from
rustup, which the build asks for the toolchain `rust-toolchain.toml` names, it needs cmake, ninja,
clang-format, Qt 6.12 (Quick, Test and qmllint), [MpvQt](https://invent.kde.org/libraries/mpvqt)
1.2.1 with mpv 0.38 or later, [gcovr](https://gcovr.com),
[Corrosion](https://github.com/corrosion-rs/corrosion) and
[uniffi-bindgen-cpp](https://github.com/NordSecurity/uniffi-bindgen-cpp), installed with
`cargo install` at the tag CI installs; it stops first, naming the command, when the bindgen is
missing or at another tag. On Linux, two tests run real mpv, each on an X display of its own, the
pane's and the app's end to end, so the script needs Xvfb and xdotool there too, and a font, which
mpv needs to draw the subtitle the pane's test looks for; those displays are the tests' alone, and
nothing they play makes a sound.

The app is `subcheck-qt [run | paths…]`, which takes the run or paths `subcheck review` takes, and
with none opens the newest check's folders, as bare `subcheck review` does. Its window has three
columns: a sidebar with the reviews (one per show folder the store holds, which a click opens), what
the list shows (each filter with its count) and the view's episodes (a click selects one and its
first finding, when it has one); the findings, two lines a row (what found it, its episode and time,
its decision, and its line with the change marked, or for a timing row whose members the filter
shows differ in their fixes, those fixes, counted); and the details panel, the video at its top, a
mark in the video's corner saying whether it plays or is paused, then the selected finding, its line
as the subtitles have it now in a block above the line with the fix, with Reject and Accept Fix. An
episode the check found nothing in is greyed in the sidebar and says `No findings`. A click on one
shows what its check said, alone under the video; the finding you were on stays greyed in the list,
as the episode has none to go to, and Accept, Reject, Back to Undecided, Show in Video and Enter
wait until you move to a finding again, by a move, a click or Undo. View ▸ Episodes Without Findings
Last lists those episodes after the rest, while `n` and `p` still go to the next and previous
episode in review's order. File ▸ Settings… (Ctrl+, or ⌘, on macOS) opens a window of its own that
chooses the window's colours, the default or a palette for deuteranomaly/deuteranopia,
protanomaly/protanopia, tritanomaly/tritanopia, or little or no colour vision, with a sample, and
the size of the text of the window, the findings list and the details panel, each the platform's
until set; each choice applies at once, and while Settings is open no key of the main window's
reaches review. The menus hold the rest, and the terminal's keys work too: `j` and `k` (or the up
and down arrows) move, Enter expands a row into its members, `a`, `r`, `x` and `u` accept, reject,
put back to undecided and undo, `f` steps through the filters, `n` and `p` go to the next and
previous episode, `v` switches between a check and its folders, `o` opens the selected finding in
the video pane as the terminal's `o` does in mpv, playing on if the pane plays and paused if not,
and `O` stops playback, leaving the pane black and review open; Space plays and pauses, ← and → seek
5 s, `,` and `.` 1 s, and `[` and `]` step a frame; `q` quits. While Show in Video (`o`) waits for a
file that is slow to open, a busy animation shows over the video once the wait has lasted 150 ms; it
goes once the video shows, or once Close Video (`O`) has closed the video. While a video plays, a
drag of a column or of the window's edge stays smooth. A column drag stretches the video's frame
until the drag ends, when mpv draws it again at its new size. The window keeps its size, its place
on X11 and macOS (Wayland lets no window place itself), its columns' widths, the episodes' order and
its settings for the next run, in `$XDG_CONFIG_HOME/subcheck/subcheck-qt.conf` on Linux and macOS
when `XDG_CONFIG_HOME` is an absolute path; otherwise in `~/.config/subcheck/subcheck-qt.conf` on
Linux and in the app's preferences on macOS. At the end it prints the counts as the terminal does,
but names a view of paths as its window's title does: by the first path's last part, and `+N` for
the others. It exits with 1 after an error, or when review has not ended 10 s after quitting, and
with 0 otherwise. The 10 s gives one decision's write time to wait out another process's lock on the
store, which the store waits up to 5 s for. It is not enough for several writes queued behind a lock
held longer, or for an open that waits on the lock more than once: the window then closes review,
which can lose a decision not yet written.

The video pane is mpv, inside the window through MpvQt. It reads its options from
`$XDG_CONFIG_HOME/mpvqt/mpvqt.conf` on Linux (by default `~/.config/mpvqt/mpvqt.conf`) and
`~/Library/Preferences/mpvqt/mpvqt.conf` on macOS, not from `~/.config/mpv/mpv.conf` as the
terminal's mpv does, so subtitle styling set there does not reach it; a line
`include=~/.config/mpv/mpv.conf` in `mpvqt.conf` brings it in. The window sets `video-timing-offset`
to 0 over any value set there, so that it keeps its own frame rate while a video plays. To build and
run the app, with the same tools (on Linux, Xvfb and xdotool among them: configuring adds the tests
that use them):

```sh
cmake -S gui/qt -B ~/.cache/subcheck-qt-build -G Ninja -DCMAKE_BUILD_TYPE=Debug
cmake --build ~/.cache/subcheck-qt-build --target subcheck-qt
~/.cache/subcheck-qt-build/subcheck-qt
```

On macOS, Homebrew has no MpvQt, so it is built from its release first, into a prefix of its
own, and the app's build is told where it is and where Homebrew's Qt and mpv are. The third
command must print `8a4d1ae9d1a1c5c94cd96bccb5330b83c1704b99`, the commit CI checks the tag
against:

```sh
brew install qtbase qtdeclarative mpv extra-cmake-modules pkgconf corrosion ninja cmake
git clone --depth 1 --branch v1.2.1 https://invent.kde.org/libraries/mpvqt.git ~/.cache/mpvqt-src
git -C ~/.cache/mpvqt-src rev-parse HEAD
cmake -S ~/.cache/mpvqt-src -B ~/.cache/mpvqt-build -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$HOME/.local/opt/mpvqt" -DCMAKE_PREFIX_PATH="$(brew --prefix)"
cmake --build ~/.cache/mpvqt-build
cmake --install ~/.cache/mpvqt-build
cmake -S gui/qt -B ~/.cache/subcheck-qt-build -G Ninja -DCMAKE_BUILD_TYPE=Debug -DCMAKE_PREFIX_PATH="$HOME/.local/opt/mpvqt;$(brew --prefix)"
cmake --build ~/.cache/subcheck-qt-build --target subcheck-qt
~/.cache/subcheck-qt-build/subcheck-qt
```

CI also checks `Cargo.lock` against the [RustSec](https://rustsec.org) advisory database, in a
workflow of its own, `.forgejo/workflows/advisories.yml`: weekly, after each merge to main, and on
a pull request that changes one of the files the workflow lists. A new advisory can turn it red
with nothing changed here, so no merge waits on it. To run it locally, install
[cargo-deny](https://github.com/EmbarkStudios/cargo-deny) and run
`cargo deny --locked check advisories`, which reads `deny.toml`. How to accept a finding is in the
workflow's header, the one place that describes it.

This repository's history before 2026-11-02 was in
[daveyutils](https://github.com/Davey-Hughes/daveyutils): a `(daveyutils#N)` in a commit message is
that repository's pull request N.

## Review

`subcheck scan` opens this screen on the scan it starts, when stdout and stderr are a terminal
(`--no-tui` keeps the log). Each file's findings join the list when the file is done, the
selection staying on its row as rows come in above it. A typo that joins a row whose findings are
all decided alike takes that decision, your wording too, and the status line says so; `u` takes it
back. A timing finding that joins a row stays pending: each episode's fix can differ.

`Tab` shows the files instead: each with where the scan is with it (`queued`, `reading`, `LLM 2/3`,
`✓ 412 lines · 3 findings`, `✓ timing only · 1 finding`, `✗ FAILED: …`), and in the detail pane
what the scan said of it. `⏎` on a file goes to its first finding, and `u` undoes there as it does
in the list. `c` checks the file again, as `subcheck scan --run <run> --force <file>` does: into
the run you are reviewing, or in a view of files and folders, the newest run that named it, with
that run's LLM and languages. It asks first, since it spends money, and while a scan runs it waits
for that to end. While the scan runs, the list's title and the status line say how far it is; once
it ends, the status line gives its summary. What the scan warns of as it starts (a folder it cannot
read, a server that did not say its context size) stays on the terminal above the screen, and the
first of it is on the status line as the screen opens. Files the scan skips as unchanged are in the
list from the start.

When a scan checks a file again, its earlier findings stay in the list until the new ones are saved,
and you can decide them meanwhile: a decision carries over to each finding that comes back as the
same fix ([Where findings are kept](#where-findings-are-kept)). While it is checked, the file's name
spins in the list, and the detail pane says where the check is (`checking again: LLM 1/3`). If the
check fails, the findings stay. A decision on a finding that another subcheck checked again since
the screen opened goes to each finding that came back in its place, and the status line says so;
when none came back, nothing is written, and the status line says that too.

`q` while the scan runs asks first, then stops it: the run keeps every file that was done, and the
terminal gets the command that finishes it (`subcheck scan --run <run> <paths>`; after a `c`,
`subcheck scan --run <run> --force <files>`).
Files `c` queued for after the scan are left unchecked: the question counts them, and the terminal
gets the command that checks them (`Not checked again: subcheck scan --run <run> --force <files>`).

A bare `subcheck review` opens on the folders that hold the newest run's files: every file
recorded there, whichever run checked it. The list's top border names what is shown
(`run 2026-10-29T21-30-49`, or `Spy x Family (2023)`), and `v` switches: from a run to its folders,
or from folders to the newest run that named the selected file. The selection, the filter and what
you can undo come along where the other view shows them.

`A` applies the accepted fixes of the view on screen, whatever the filter, as `subcheck apply`
would with the same view: it asks first (`apply 37 fixes in 9 files (30 applied already), keeping
each .orig? y/n`), then remuxes, checks and swaps each file in turn while you go on reviewing. `D`
is a dry run: it asks nothing and writes nothing. Both wait for a scan to end. While they run, the
list's top border and the status line say how far they are, the files view shows each file's
stage (`queued`, `remux`, `✓ swapped · 3 applied · 1 by hand`), and its detail pane what apply
wrote for it; `v` and `c` are refused until apply ends. Each row then says what became of its fix:
`applied` (from the database, so it shows in every later review), `by hand`, `not found` or
`failed` (its file failed) for this session, or after a dry run `would apply`, `already applied`,
`would be by hand`, `would not be found`, `would fail`; a row whose episodes differ counts them
(`applied 5 · by hand 1`), and the detail pane gives a by-hand fix's reason, or why its file failed.
`--karaoke` on `review` or `scan` re-spaces per-letter karaoke as `apply
--karaoke` does. `q` while apply runs asks first; the file it was on is left as it was, and the
terminal gets the command that makes the rest (`subcheck apply <view>`).

A fix repeated across episodes (an OP lyric, say) is one row; `⏎` expands it into its episodes. A
season's timing findings on one track are one row too, whatever each episode's fix (`−1 frame ×23 ·
partial ×1`): `o` opens one on the frame that shows the error, or with none, its first moved line's
first frame, playing on or paused as mpv was, as a line does. `[` `]` step from there once it is
paused (if mpv plays, Space and then `o` again open it there paused), and `e` does not apply (a
timing fix is accepted or rejected).

Decided rows stay in the list: green when accepted, red when rejected, yellow when a group's
episodes are all decided, differently. A row with anything still pending keeps the terminal's
colour (its mark `±` when some episodes are decided). Review opens on the first row with anything
pending, and after `a`, `r`, `e` or `x` moves to the next one below (under the accepted or rejected
filter, to the next row).

| Key | Does |
|---|---|
| `j` `k` ↑ ↓ `g` `G` PgUp PgDn | move |
| `n` `p` | first row of the next or previous file |
| `a` `r` | accept or reject (a row: the pending episodes the filter shows, or all it shows when none is pending; an episode decided on its own keeps its decision) |
| `x` | clear: back to pending (a row: every episode the filter shows) |
| `e` | reword, then accept |
| `u` | undo this session's last decision or clear |
| `f` | filter: all, pending, accepted, rejected |
| `Tab` | the files, or back to the findings |
| `v` | a run's folders, or from folders, the newest run that named the selected file (not while a scan or apply runs) |
| `c` | in the files view: check the file again (asks first; while a scan runs, once it ends; refused while apply runs) |
| `A` | apply the accepted fixes of the view (asks first; not while a scan runs) |
| `D` | dry run: what apply would do, writing nothing |
| `o` | open the finding in mpv with its subtitle track: a line on its first frame, a timing finding on the frame that shows the error, or with none, its first moved line's first frame; paused if mpv is paused or has no file open, playing on if it plays, as the status line then says; again on the same row goes back there |
| `O` | close mpv and stay in review; the next `o` opens it again |
| Space | pause or resume mpv |
| ← → | mpv back or forward 5 s |
| `,` `.` | mpv back or forward 1 s |
| `[` `]` | mpv back or forward one frame (it stays paused) |
| `?` | show every key, and what the run's LLM calls cost, in a box over the list; `q` quits from it, any other key closes it |
| `q` | quit, closing the mpv window review started; while a scan or apply runs, ask first, and stop it |

The mouse works too. Click a row to select it, and double-click to open it in mpv. The wheel moves
the selection over the list and scrolls the detail pane. `? help` at the end of the status line, and
the editor's `⏎ accept · Esc cancel` there, are clickable. Since review takes the mouse, select text
in the terminal with Shift+drag. A question on the status line (`… y/n`) takes the next key: `y`
answers it; Ctrl-C or Ctrl-D quits, but while a scan or apply runs asks to quit in the question's
place, unless it asks that already; Ctrl-U and the other Ctrl and Alt keys leave it open; and any
other key only lets it go. So do a click on a row, a file or `? help`, and a turn of the wheel over
the list; other clicks, and the wheel over the detail pane, leave it open. While the editor is open,
the mouse lets it go only by a click on `? help`; while the box of keys is open, a click only closes
the box.

Whatever `mpv.conf` says (or, for the window, `mpvqt.conf`), a file `o` loads gets, beside its
start, `sub-visibility=yes` and, when the finding names one, its subtitle track, three more options
of its own: `hr-seek=yes`, so that it lands on the frame and not on the keyframe before it, which
makes mpv's own relative seeks in that file exact too; `rebase-start-time=no`, so that mpv's clock
is the file's own, which subcheck's times are on, even for a file whose first cluster is not at 0;
and `pause`, as mpv is to end up, which `reset-on-next-file=pause` would otherwise undo. `o` on a
file that is not there (whose size and time cannot be read) says `file missing`, and asks nothing
of mpv.

## LLM settings

The settings file defines LLMs of your own for `--llm` to pick by name. It is
`~/.config/subcheck/config.toml` (under `$XDG_CONFIG_HOME` when that is set to an absolute path,
`~/Library/Application Support/subcheck/` on macOS), or the absolute path in `SUBCHECK_CONFIG`. No
file is the same as an empty one.

How many typos an LLM finds, and how much of what it flags is a real mistake, depends on the
model. subcheck's prompt was written with claude, and a small local model finds fewer typos and
flags more lines that need no fix. [Measuring a model](#measuring-a-model) compares LLMs on an
invented episode.

```toml
default = "home"

[llm.home]
url = "http://127.0.0.1:8000/v1"
jobs = 4
shared_context = false

[llm.home.request]
temperature = 1.0
top_k = 64
top_p = 0.95

[llm.openrouter]
url = "https://openrouter.ai/api/v1"
model = "google/gemma-4-12b-it"
api_key_cmd = "pass show openrouter | head -n 1"
thinking = "server"

[llm.mine]
command = "my-llm --fast"
output = "openai"
jobs = 4
chunk_lines = 200
```

An entry has a `url` (a server that speaks the OpenAI chat API) or a `command`.

- `url` is the server's address up to and including `/v1`.
- `model` is the model to ask for. Left out, it is the one model the server lists. Either way the
  server must list it. A name without a tag, such as `gemma4`, also matches the `gemma4:latest`
  Ollama lists for a model pulled without one.
- `api_key_env` names an environment variable that holds the key, and `api_key_cmd` is a command
  that prints it; see [Keys](#keys).
- `thinking` is `"off"` (the default), which sends `reasoning_effort: "none"` and
  `chat_template_kwargs.enable_thinking: false`, or `"server"`, which sends neither, for an API that
  refuses them.
- `request` is a table merged over every request body, last, each key replacing subcheck's. It
  cannot set `model`, `messages` or `stream`. A `max_tokens` or `max_completion_tokens` there is
  the reply's room in place of subcheck's cap ([Fitting the context](#fitting-the-context)).
- `shared_context` says whether the calls in flight share the server's context. `true` (the
  default) fits each chunk to the context divided by `jobs`: llama-server's slots share one cache,
  so two calls that each fill the context fail when they run at once. `false` fits each chunk to the
  whole context, for a server that gives each call all of it, as vLLM and Ollama do.
  `ollama:<model>` gives each call the whole context, as Ollama does; an entry whose `url` is
  Ollama's `/v1` can say `false` to do the same.
- `command` is run through `sh -c`, as `--llm-cmd`'s is: it reads the prompt on stdin and prints
  the reply on stdout. It has no terminal, as a key command has none ([Keys](#keys)): a command
  that asks for a password in the terminal fails at once, so the password must come from an agent
  or a window (`ssh` can ask through `SSH_ASKPASS`).
- `output` is what the command prints: `"text"` (the default) is the reply alone, and reports no
  usage; `"openai"` is an OpenAI chat completion, and reports its tokens; `"claude"` is
  `claude -p --output-format json`, and reports tokens and a price; `"codex"` is
  `codex exec --json`, and reports tokens.
- `jobs` is the calls in flight to this LLM at once (2 when left out).
- `chunk_lines` is the lines sent in each call, for a server at most as many as fit its context.
  Left out, it is as many as fit, at most 400 ([Fitting the context](#fitting-the-context)), or 400
  for a command.
- `timeout_minutes` is how long each call may take, and a model's load, in whole minutes from 1 to
  1440 (10 when left out). A slow model with a long reply can need more. The limit is for each
  request, so a call that fetches its key again ([Keys](#keys)) can take it twice, and the key
  command's 2 minutes between.
- `default` is the LLM `--llm` means when it is left out. It takes anything `--llm` takes, a
  preset as well as an entry.

`subcheck scan --llm mine` runs the entry. `-j`, `--chunk-lines` and `--llm-timeout` beat the
entry's `jobs`, `chunk_lines` and `timeout_minutes`; `--llm-timeout` is also the only way to give
`claude`, `gpt`, `ollama:<model>` or `--llm-cmd` more than 10 minutes. A recheck from the screen
(`c`) uses the entry as the file has it then, not as it was when the run began, and no `-j`,
`--chunk-lines` or `--llm-timeout` the scan was given.

An entry's name is letters, digits, `-` and `_`, and `claude` and `gpt` are taken. A run records
a command entry's name, not its command: after changing what a command runs, rename the entry or
scan with `--force`, or files checked by the old command are skipped as already checked.

subcheck asks `<url>/models` before a scan or a dry run, so a server that is down stops it before
any file is checked. llama-server lists the path given to `-m` unless it was started with
`--alias <name>`.

A run records `<entry>@<model>` for a server, so moving a server to another port checks nothing
again, and serving another model checks every file again. `scan --run` and a recheck refuse a
run whose model the server no longer serves.

A mistake in the file stops `scan` and review's rechecks, with a one-line message naming the file,
then the entry or the line the mistake is in. `list` and `apply` do not read it, and `review` reads
it only to recheck, so all else works with it broken.

### Fitting the context

A call to a server sends as many lines as fit the context the server reports, at most 400. With
`shared_context` (the default), the calls in flight share it, so each call fits the context divided
by `-j` or the entry's `jobs`. The last quarter of that, up to 4096 tokens, is left for the reply,
and by an estimate of its tokens, the prompt takes the rest. `--chunk-lines`, or the entry's
`chunk_lines`, sets the lines instead: at most as many as fit the context the server reports, with
no limit of 400, and exactly that many when it reports none. `ollama:<model>` gives each call the
whole context, as Ollama does.

Each reply is capped at that room (`max_tokens`, or `num_predict` for `ollama:<model>`): a reply
that outgrew its room would overflow a cache the calls in flight share, and a model that repeats
itself would run on until the context is full. 4096 tokens is far more than a reply to 400 lines
needs, as it names only the lines with a fix, and a hosted API refuses a call whose reply may take
more than it allows, however large its context. A reply cut off at the cap is taken as one cut off
at the server's own limit (below), and a chunk that fails for it names the cap and what sets it. An
entry's `request` can set its own `max_tokens`, or `max_completion_tokens`, the name OpenAI's
newer models take, as for a model whose thinking needs more room; subcheck then sends no cap of its
own beside it. That is then the reply's room (the larger, if it sets both), at any size up to all
of a call's share, and the prompt is fitted to what is left. A cap that leaves the prompt no room
for a line, as `max_tokens = 8192` does in a call's 8192 tokens, sends each line in a call of its
own, and the scan warns once as it starts (a recheck on the status line), naming the cap and the
share.

The size comes from the server's list of models: `meta.n_ctx` (llama-server), `max_model_len` (vLLM)
or `context_length` (OpenRouter). Ollama's list has none, so when a list gives no size, subcheck
asks Ollama's `/api/ps`, for `ollama:<model>` and for an entry whose `url` is plain `http://` and
ends in `/v1`, as Ollama's is on this machine or a LAN. An `https://` entry, a hosted API, is not
asked, and with no size in its list, it gets the warning below. `/api/ps` gives the context of a
model once it is loaded, so a scan with a file for the LLM has Ollama load the model first, saying
`loading <model> into Ollama at <address>` while it waits; a scan with nothing for the LLM loads
nothing. `OLLAMA_CONTEXT_LENGTH` sets the context Ollama's server loads a model with by default,
and a larger one means fewer calls.

A server that says no size gets chunks of 400 lines and no cap on its replies, and the scan warns
once, as it starts, here for an entry `lan` with the default 2 jobs:

```
warning: lan did not say its context size; set chunk_lines if it holds under 38k tokens
```

38k is the context that holds a 400-line chunk for each of the 2 calls in flight; with
`shared_context = false` it would be 19k, one chunk's. A recheck (`c`) gives the warning on the
status line. For `ollama:<model>` it names `--chunk-lines`.

A chunk can be too big anyway. It is halved when the server refuses it for the context, as
llama-server does with `500 Context size has been exceeded.` when the calls in flight outgrow the
cache they share, or when its prompt took 95% of a call's share of the context or more, and more
than the room left for the prompt, so the server may have cut it. Each half goes in turn, halved
again the same way. A reply cut off at its cap or the server's length limit halves its chunk once.
A single line the server refuses twice fails its chunk, as does a half that fails.

`ollama:<model>` calls Ollama's own API (`/api/chat`), which refuses a prompt too big for the
context, so its chunk is halved. An entry whose `url` is Ollama's `/v1` goes through Ollama's
OpenAI API, which cuts such a prompt to fit without saying so and answers for what is left; fitting
the chunks to the context guards against that, and a prompt it cut comes to 95% of a call's share
of the context or more, so its chunk is halved too.

`scan --dry-run` prints each chunk as the scan would send it. It asks the server for its models and
its context but loads no model, so for an Ollama model not loaded yet its chunks are 400 lines, or
the lines `--chunk-lines` or `chunk_lines` sets, which a scan cuts to what fits once the model is
loaded, and it says so:

```
dry run: ollama:gemma4's model is not loaded, so these chunks are 400 lines; a scan loads it and fits them to its context
```

### Keys

A key is never in the file: `api_key` is refused. `api_key_env` names a variable. `api_key_cmd`
runs through `sh -c` with no terminal, within 2 minutes, and prints the key alone on one line. The
key is kept while the server takes it, so a passphrase is usually asked once a session. When a
server answers 401 to a key that had worked (a call got an answer with it since the command
printed it), as an hour-long token does when it runs out mid-scan, subcheck runs the command
again, once: a new key carries the scan on, and the same key again stops its calls, each remaining
file failing with the server's refusal. A key that never worked is not fetched again, and neither
is a variable's. A 403 fails its call alone, since a server can refuse one request that way; the
next scan or recheck runs the command again, as after any refusal.

A prompt in the terminal is not supported. The command's stdin is empty and it runs in a session
of its own, so a script that opens `/dev/tty` to ask fails at once. `GPG_TTY` is unset, but
gpg-agent can still show `pinentry-curses` on the terminal the agent started from, which can be
the review screen's. A graphical pinentry, KWallet, GNOME Keyring, the 1Password app, the macOS
Keychain or an unlocked agent works.

A key sent over plain `http://` to another machine warns. A server on this machine is never
called through a proxy.

### Servers, by example

Each command below starts a server on this machine with a context of 16384 tokens, and the entry
after it is the one for that server. A larger context sends more lines in each call, up to 400
([Fitting the context](#fitting-the-context)).

llama-server's default port differs between builds, so the command names one:

```sh
llama-server -m <model>.gguf --alias local -c 16384 --parallel 2 --port 8080
```

```toml
[llm.local]
url = "http://127.0.0.1:8080/v1"
jobs = 2

[llm.local.request]
temperature = 1.0
top_k = 64
top_p = 0.95
```

`--alias local` has `/models` list `local` in place of the file's path, so a run records
`local@local`. `--parallel` sets the server's slots, and `jobs` is at most that many: a call
beyond them waits for a slot. The slots share the context, so `shared_context` stays `true`, which
fits each call to half of it here. The `request` table sets the sampling of every call in place of
the server's defaults, with the values the model's card gives.

vLLM:

```sh
vllm serve <org>/<model> --max-model-len 16384 --port 8000
```

```toml
[llm.vllm]
url = "http://127.0.0.1:8000/v1"
shared_context = false
jobs = 4
```

vLLM gives each call in flight all of `--max-model-len`, hence `shared_context = false`, and runs
the calls in flight as one batch, so four at once finish a scan sooner than two.

`ollama:<model>` needs no entry. The context is `OLLAMA_CONTEXT_LENGTH`
([Fitting the context](#fitting-the-context)), set where the server starts, or in its service's
environment where Ollama runs as one:

```sh
OLLAMA_CONTEXT_LENGTH=16384 ollama serve
subcheck scan --llm ollama:<model> <path>
```

An entry whose `url` is Ollama's `/v1` goes through Ollama's OpenAI API instead, for a `request`
table of your own. Ollama lists every model pulled, so the entry names one, and
`shared_context = false` gives each call the whole context, as `ollama:<model>` does:

```toml
[llm.ollama-v1]
url = "http://127.0.0.1:11434/v1"
model = "<model>"
shared_context = false

[llm.ollama-v1.request]
temperature = 0.5
```

A hosted API needs only its entry. `api_key_env` names the variable that holds its key; the
`openrouter` entry above gets its key from a command instead, with `api_key_cmd` ([Keys](#keys)).
`thinking = "server"` is for an API that refuses the switches that turn thinking off:

```toml
[llm.hosted]
url = "https://openrouter.ai/api/v1"
model = "<org>/<model>"
api_key_env = "OPENROUTER_API_KEY"
thinking = "server"
```

A command entry reads the prompt on stdin and prints the reply, and `output` says how to read what
it prints, one entry here for each:

```toml
[llm.ask]
command = "llm -m <model>"

[llm.post]
command = "post-chat <model>"
output = "openai"

[llm.claude-cmd]
command = "claude -p --output-format json --model <model>"
output = "claude"

[llm.codex-cmd]
command = "codex exec --json --skip-git-repo-check --model <model>"
output = "codex"
```

`ask` keeps the default `output`, `"text"`, for a command that prints the reply alone, as
[llm](https://llm.datasette.io) does. `post-chat` stands for a script of your own that posts the
prompt to a chat API and prints the JSON the API returns. `claude-cmd` and `codex-cmd` run claude
and Codex with flags of your own, here `--model`. A command entry runs its command as written,
without the other flags the `claude` and `gpt` presets pass, such as those that keep each call from
loading your hooks or saving a session. Codex refuses to run outside a git repository without
`--skip-git-repo-check`, and the command runs in the directory subcheck was started in.

### Measuring a model

From a checkout, the bench scans an invented episode with each LLM named and scores its findings
against the episode's answer key:

```sh
cargo run -p subcheck-core --example bench -- [-v] <llm>...
```

Each `<llm>` is a name `--llm` takes, a preset or an entry in the settings file `scan` reads
(`SUBCHECK_CONFIG` names another). The episode is `core/examples/bench/script.ass`: dialogue, an
opening, an insert song, an ending and signs, with typos planted in it. Its key, `answers.toml`
beside it, lists each planted typo, with the fixes that count as right, and each trap: a line with
nothing to fix that looks as if it needs one, such as a name, an honorific, slang, a stutter or a
lyric. Each LLM scans it into a store of its own in a scratch directory, which the bench deletes
when it ends, by Ctrl-C too. A second Ctrl-C can end the bench while it deletes, and leaves what
remains in `$TMPDIR` or `/tmp`: a `.tmp…` directory, its name ending in `.removing-<pid>` when a
Ctrl-C began the deletion. Your store is never touched, and an LLM named twice is scanned twice,
not skipped as checked.

The first line it prints counts what the key holds, `bench.mkv: <n> planted typos, <n> traps`,
and a table follows, with a row for each LLM, such as this one for a llama-server entry:

```
llm          jobs  lines/call  right  other fix  missed  false alarms  other   time  calls                tokens
local@local     2         155     27          0       3             3      0  21.8s      3  12.9k in / 1.12k out
```

The columns:

- `llm` is the LLM as its run records it, `<entry>@<model>` for a server or `ollama:<model>` as
  named, or as named when it failed before that.
- `jobs` is the calls in flight, and `lines/call` the lines sent in each, as fitted to the context.
- `right` is the typos whose corrected line is one the key lists.
- `other fix` is the typos found but corrected some other way. A curly apostrophe where the key has
  a straight one counts here, as `apply` would write it.
- `missed` is the typos with no finding.
- `false alarms` is the traps flagged, one for each line however many findings it has.
- `other` is the findings on lines the key does not name. Those lines have nothing to fix, so these
  are false alarms too, on plain lines; a name or honorific flagged in a line that is not a trap
  counts here.
- `time` is how long the scan took from start to end. It leaves out the server check and the load
  subcheck has Ollama make (`loading <model> into Ollama at <address>`). A server that loads its
  model on the first call counts that load in the first row of its LLM.
- `calls` and `tokens` are the calls made and the tokens in and out, as [LLM costs](#llm-costs)
  counts them, with `-` for an LLM that reports no tokens.

A row whose LLM failed ends after `lines/call` with `failed: <why>`, and has 0 for a column it
failed before learning. The other LLMs still run, and the bench exits with 1.

Two of the planted typos, `befor` and `trough`, are the examples the scan's prompt gives, so every
model is shown those two. Four are a missing apostrophe alone (`Thats`, `dont`, `its`, `were`),
which a model that takes the prompt's rule against reporting punctuation literally may skip, and
a model that obeys its rule against reporting names may skip `Kurumya`, the bakery `Kurumiya`
misspelt. A model samples at random, so its rows differ from one run to the next; name an LLM
several times (`local local local`) to see by how much. An LLM's first row may run with the
server's cache cold, where later rows find the same prompts cached, and may hold the load of a
server that loads its model on the first call. So compare first rows only between servers that load
the same way, or name the LLM once more and leave its first row out, which works when that LLM's
rows run back to back.

Warnings, such as the one for a server that says no context size, go to stderr, as does the
line saying Ollama is loading a model. `-v` also shows on stderr what the scans say of each file
as they run, and after the table, under each row's number and LLM, a line for each typo it missed
or fixed another way, each trap it flagged and each other finding:

```
1. <llm>
  missed [<kind>] <line>
  other fix [<kind>] <line> -> <fix> (<reason>)
  false alarm [<kind>] <line> -> <fix> (<reason>)
  other <line> -> <fix> (<reason>)
```

`<kind>` is the key's, such as `misspelling`, `apostrophe` or `honorific`; `<fix>` and
`<reason>` are the LLM's. A line break in a failed row's reason, or in an LLM's label, line, fix or
reason, shows as `; `, so each row and each of these lines stays one line.

When a real scan finds a kind of typo the episode lacks, or a model flags a line it should not, an
invented line of the same pattern joins `script.ass`, never the real line: no line of the episode
comes from a real show. Its entry in `answers.toml` is a `[[typo]]`, with its `kind`, its `line`
and each `fixed` line that counts as right, or a `[[trap]]`, with its `kind` and `line`.
`dev/make-fixtures.sh` builds `bench.mkv` from the script, and every other fixture with it, which
another version of mkvmerge or ffmpeg can make differently from the committed ones. So build them
into a scratch directory, `<dir>` here, and copy back only the bench's two files, from the
checkout's root:

```sh
dev/make-fixtures.sh <dir>
cp <dir>/bench.mkv core/tests/fixtures/
cp <dir>/expected/bench.track0.ass core/tests/fixtures/expected/
```

The tests check that each answer is the text of exactly one line, with no markup in it, that no two
lines read the same, and that the fixture holds the script as it is.

## LLM costs

A scan records what each LLM call used, retries and failed calls too, and after its summary says
what its calls cost. `list` says it for the whole run under its first line (for the files under
some paths, the calls of the scans that made their current LLM checks), and
review in `?`'s box and on the status line as it opens, until the first key or click (as much as
fits beside `? help`: the tokens go first, then the calls, keeping the price):

```
LLM: 98 calls, 1.23M in / 31.2k out tokens, $3.21 at API prices
```

- `claude` reports tokens and a price. The price is what the calls would cost at Anthropic's API
  rates, not what a Claude subscription bills.
- `gpt` (Codex) reports tokens only, so no price is shown: subcheck keeps no price list of its own
  to go stale.
- `--llm-cmd` and entries that print text report nothing, and nothing is shown; nor for a run made
  before calls were recorded.
- An entry with a `url`, and `ollama:<model>`, report tokens, and no price.
- An entry with `output = "openai"`, `"claude"` or `"codex"` reports what its JSON says: tokens,
  and for `"claude"` a price too.

The tokens in are all of each call's input, read from the prompt cache or not.

Every call claude or codex started is counted, failed ones too. One that said nothing of what it
used (killed at its time limit, failing with nothing on stdout, printing something other
than its JSON, or JSON with no usage in it) is counted without numbers, and the sums leave it out;
any call without a price makes the price a floor:

```
LLM: 99 calls (1 reported no usage), 1.21M in / 30.5k out tokens, at least $3.15 at API prices
LLM: 200 calls (none reported usage)
```

A failed call's error is the exit status and stderr, then what the backend's JSON says of it:
`claude exited with exit status: 1; claude said: You've hit your session limit`. A server's is
`HTTP 401: Incorrect API key provided; check the entry's key`. A call that failed on a refused or
unresolvable connection is not counted.

## Where findings are kept

Everything subcheck records is in one SQLite database: `~/.local/state/subcheck/subcheck.db`
(under `$XDG_STATE_HOME` when that is set, `~/Library/Application Support/subcheck/` on macOS), or
the absolute path in `SUBCHECK_DB`. Findings and decisions belong to files: each file has at most
one current LLM check and one current timing check, and a check made again (the file changed, or
`--force`) replaces the one before, whichever scan made it. A decision carries over to each finding
that comes back as the same fix. For a typo, that is the same line and the same correction: a typo
you rejected stays rejected when the file is checked again after `apply` fixed other lines.
A timing fix is the same fix when it moves the same track's stretches by the same frames: timing
rules that change what a finding rests on keep your decision on it.

A run is the record of one scan, and a view of the files it named as they are now: two runs that
share a file show it the same. `review`, `list` and `apply` take a run name, or files and
directories instead, for every file recorded under them:

```
subcheck list ~/Videos/K-On!
subcheck review 2026-10-28T19-04-11
```

With neither, they take the folders that hold the newest run's files: after
`subcheck scan --force E04.mkv`, the whole season E04 is in.

A scan with nothing to check makes no run. On a terminal it opens the screen on its files; in a
pipe it says how to (`Nothing to check: 12 unchanged (subcheck review '/m/K-On!')`).

The database is in WAL mode, so to back it up copy it with
`sqlite3 ~/.local/state/subcheck/subcheck.db ".backup 'subcheck-backup.db'"`, not `cp`, which can
miss what is still in `subcheck.db-wal`.

To look inside, `sqlite3 ~/.local/state/subcheck/subcheck.db`: `scans` and `scan_files` are the
runs, `checks` each attempt to check a file (the current one has no `replaced_by` and no `error`),
then `findings`, `decisions` (a finding's latest is its current one), `applied` and `usage`.

Run files from before the database (`runs/` and `cache.jsonl` in the state directory) are no longer
read, and can be deleted.

## Keeping focus in the terminal

The first `o` starts mpv, and a new window usually takes focus. Later ones load into the same
window and leave focus alone. The ways below rest on two mpv options, `--focus-on` and
`--wayland-app-id`, that an older mpv may lack: one that exits on them is started again without
`--focus-on`, then without both, and without them its window takes focus.

- macOS: nothing to do; subcheck starts mpv with `--focus-on=never`.
- KDE Plasma: mpv's window has the app id `subcheck-mpv`, so a KWin rule can keep focus off it.
  Add this to `~/.config/kwinrulesrc`, then run `qdbus6 org.kde.KWin /KWin reconfigure`:

  ```ini
  [General]
  count=1
  rules=subcheck-mpv

  [subcheck-mpv]
  Description=subcheck mpv: never take focus
  wmclass=subcheck-mpv
  wmclassmatch=1
  acceptfocus=false
  acceptfocusrule=2
  above=true
  aboverule=3
  ```

  If the file already has rules, add `subcheck-mpv` to the end of its `rules=` list and 1 to
  `count=` instead of replacing them. To undo, remove the group and its entry in `rules=`, lower
  `count=`, and reconfigure.

  The rule stops this one window from ever taking keyboard focus. Review's playback keys control
  it; mpv's own keys (`f`, `q`, …) do not reach it. The window closes when the review that started
  it quits or its terminal closes; close one left by any other way from its title bar.
  (KWin's focus stealing prevention would be gentler, but KWin 6.8 on Wayland ignores it per
  window when a window opens.) It also turns Keep Above on when the window opens, because a
  window that is refused focus opens behind the active one, so a maximized terminal would hide
  it. Alt+F3 → More Actions → Keep Above turns that off.

## Audio

`subcheck audio <files or directories>` decodes every audio track with ffmpeg in ten-second
windows spread over each file (0:30, 20, 40, 60 and 80 % in, and a minute before the end) and
cross-correlates every pair of tracks in each. It is for one audio in two codecs (a TrueHD and
its AC-3, a FLAC and an AAC of it), not a dub against its original. Two tracks hold the same
sound when they match in every window, or all but one in four; tracks in different languages
only when every window with sound in both scores 0.5 or more (see below). Each track is
reported against the lower-numbered track it matches best:

```
FMAB - E16.mkv
  track 1  Japanese FLAC 2.0   no other track holds this audio
  track 2  English TrueHD 5.1  reference
  track 3  English AC-3 5.1    34.2 ms late against track 2
           fix: mkvmerge --sync 3:-34, or --sync 2:34 if track 2 is the one off
```

- `in sync`: within 1 ms. Matroska keeps times to the millisecond, so a window can read that much
  off for tracks that are in step.
- `N ms late` (or early): the same in every window, within 3 ms (a lossy track's lag wanders
  that much with its codec).
- `drifts`: the windows disagree, so no one `--sync` fixes it; each window's lag is printed. A sync
  step partway through a track reads like this.
- `no other track holds this audio`: no track matches it; `track N holds this audio too, but goes
  with another track`: track N matches it, but another better (a copy of a copy, say).

A `fix` moves a track onto the one it is reported against. The sound alone cannot say which of
the two is off, so when that one is the top of its group (reported against none), no other
track holds its sound and none is reported against the track to move, moving it instead is given
too. A track in sync with one that has a fix gets the same move. One late or early against a
track that is itself off gets no fix: it names the track to fix first, and `audio` says the rest
once that is done. No `--sync` moves a drift, so a track steady against one is fixed against it,
as against the top of a group.

Measured as ffmpeg plays a file, which is how mpv and Plex do: an AC-3 or AAC whose encoder
priming the file does not record plays late by it, and is reported late (an AC-3 ffmpeg made, by
5.3 ms; an AAC, by about 21). Samples are placed by their timestamps, so a track with frames
missing reads in sync around the gap, not shifted.

A dub shares its original's music and effects, and its dialogue windows tell them apart; but an
episode of action can carry it through every window. So tracks in different languages hold the
same sound only when every window with sound in both scores 0.5 or more, as a clean copy does
(FMAB's dubs reached 0.37 at worst, its AC-3 copies 0.88 or more). A clean copy tagged with the
wrong language still matches, with the two languages said; a poor one does not (K-On's AAC copies
scored 0.26 at worst). Two tracks' languages compare in BCP 47 when both have it (`zh-yue` and
`zh-Hans` differ, `en-US` and `en-GB` do not), else in ISO 639-2 when both have it (`fre` is
`fra`, case aside), and the line names them in that kind, as tagged. A track with no Language
element is `eng`, Matroska's default, unless it has a BCP 47 tag. One tagged only in BCP 47 and
one only in ISO 639-2 count as different languages, as does `und` against any other: nothing says
they are the same, so they need the same 0.5.

`-v` prints every pair's lag and score in every window. The exit status is 1 only when a file or
a window cannot be read; `--ffmpeg` names the program.

## Apply

`subcheck apply` takes the accepted fixes of the newest run's folders (or those of the run named,
or of the files under some paths), with your wording where you reworded one, and makes them in each
file:

1. It finds the events each fix was made from and changes just the words that differ, keeping the
   tags around them (`{\i1}Ihave{\i0}` becomes `{\i1}I have{\i0}`).
2. mkvmerge writes `.<name>.mkv.subcheck` beside the file: the same file with the fixed tracks
   replaced, each in its place, with its name, language, flags and compression.
3. That remux is checked against the original: titles, chapters, attachments, every track's
   header, frame counts, every subtitle event, and the times of the audio frames in up to six
   5-second spans (the head, 20 to 80 % in, a minute before the end): a frame moved more than 2 ms
   fails it. With `--full` (or `--no-backup`), every frame of every track too, which reads both
   files end to end.
4. The original becomes `<name>.mkv.orig` and the remux takes its name. The original is linked to
   that name first, so the file is never missing on the way; a filesystem without hard links
   needs `--no-backup`.

A file apply swaps keeps its checks: they move to the swapped file, so the next scan leaves it
alone rather than sending its lines to the LLM again. A remux swapped in by hand (after
`--tracks-only` or `--check`) is checked again.

An existing `<name>.mkv.orig` stops a later apply of that file, since an earlier backup is never
overwritten, and the screen always keeps one. Move or delete the old `.orig`, or use `subcheck
apply --no-backup`.

A fixed track's own tags (a `Source` note, say) are carried over to its replacement. The check does
not compare tags. Any that may not come over exactly are noted in the output, the dry run's too:
a value that is not plain UTF-8 text, a tag that also targets something else (another track, a
chapter, an edition), or an element mkvmerge's tags XML cannot hold.

mkvmerge leaves a text subtitle track's default duration out of any remux and gives each event a
duration of its own instead, rounded to the millisecond. The check expects that rounding, so a file
whose events take their duration from the track's header passes, without the header field.

mkvmerge writes a track's language its own way too: Language lowercase and bibliographic (`fra`,
`Fre` and `fr` as `fre`), and a BCP 47 tag in canonical form (`iw` as `he`, `zh-yue` as `yue`);
one it does not know (`xyz`) as `und`. So the check takes each track's language as mkvmerge reads
the original (`mkvmerge -J`) and expects that in the remux: the tag when there is one (Language
beside it is written from the tag), else Language (the tag mkvmerge adds is no difference).
A fixed track goes into the remux in that language too, so mkvmerge takes it where it would refuse
the file's own (`xyz`). mkvmerge also trims spaces and tabs from a track name it is given, and
leaves an empty one out, which the check expects too.

The frame counts are the statistics tags mkvmerge writes, taken only from the program that wrote
the file. ffmpeg keeps the tags of a file it rewrites, so a track it re-encoded can still carry
mkvmerge's counts of what it was before. A track that lacks them in either file has its frames
compared by hash instead, which reads both files end to end.

mkvmerge writes some of what ffmpeg writes its own way, which the check expects too: an attachment's
UID in fewer bytes, an EditionUID for an edition that had none, and a chapter's language in its own
form (`fre` and a BCP 47 `fr` for `fra`). So it compares each chapter's languages as players read
them: its BCP 47 tags, else its language codes, else English. mkvmerge also gives new UIDs to the
editions and chapters of a file another program wrote (any whose WritingApp does not start
`mkvmerge` or `no_variable_data`, in any case), so the check compares those chapters without their
UIDs. In the track headers it expects an audio default duration where the original had none, a
FlagInterlaced of 2 (progressive) left out, and on either side, a value left out or written where
Matroska takes it as said (one channel, 8000 Hz, a display size of the picture's own).

A fixed track goes through its text format, which cuts each time down to the millisecond (SRT) or
the centisecond (ASS). Where that would change a time, a fix on that track is left for a hand fix
before any remux, and the output names the first such time: an ASS track timed to the
millisecond, say, or events that last a 23.976 fps default duration (41.708 ms, which a remux
rounds to 42, SRT writes as 41 and ASS as 40). A timing fix that moves every such event onto the
format's precision is made. The file's other fixes are made either way. An ASS or SSA track whose
every time is the same whole milliseconds past the centisecond (a mux 7 ms late, say) is made: its
`.ass` (or `.ssa`) holds each time that much early, and the remux puts them back with `--sync`, which the
output says. Where most of its lines start the same whole milliseconds past, the output names the
first time that is not that far past (a start or an end), or whatever else first keeps the track
from being written so.

A timing fix moves each line by whole frames on the frame grid the video's keyframes (its Cues)
give, each time to the same place in its new frame. Some muxes store a frame exactly half a
millisecond past one at either millisecond, both ways in one file. Where a line's move turns on
such a frame, apply takes its time from the Cues if it is a keyframe, and otherwise reads it from
the video around it; video there that does not read fails the file. A line still left a
millisecond unsure leaves its fix for a hand fix, and so does a typo on it once the fix is in.
A line the shift would leave on no frame leaves the fix for a hand fix too, naming it, and so does a
line with no duration of its own, from its block or its track (such as an SRT line shown until the
next), whose start it moves onto a frame before 0:00: apply leaves no line out. Any other line whose
start it moves onto a frame before 0:00 starts at 0:00, and the output names it
(`cut short at 0:00`). A line of duration 0 is on no frame wherever it is, and neither rule applies
to it.

`--no-backup` keeps no original. `--dry-run` shows each changed event before and after, and writes
nothing. It also checks what stops apply before it writes, an earlier `.orig` in the way or too
little free space, and fails such a file as apply would (in review, `D` marks its fixes `would
fail`). Running apply again is safe: a fix already in its file is reported and left alone.

Some fixes are left for a hand fix, with the reason: per-letter karaoke (one event per letter)
without `--karaoke`, signs set letter by letter (three characters or more besides spaces, half or
more of them right after a tag), a change across a tag or a line break, two fixes on one line, a
track whose times its text format cannot keep or whose header is not all UTF-8, every fix in a file
with any text track (ASS, SSA, SRT, WebVTT, USF, ASCII) whose lines are not all UTF-8 (mkvmerge
replaces those bytes in any remux), and a line that is not there any more.

`--karaoke` makes per-letter karaoke fixes too. Each changed letter keeps its event (tags,
colours, animation), and a new one is a copy of the letter beside it. The row is then re-spaced
by each letter's width, measured from where the style's other rows put their letters, and stays
anchored as the style aligns it: a centred row grows both ways. A letter the style never shows
gets a typical width, and the output says so. Look at each one afterwards (`subcheck review`,
then `o`). It leaves for a hand fix a row whose letters are timed one by one (`\k` or `\t`
times that differ from letter to letter; the same `\k` on every letter is fine), drawn at
different sizes, or not centred on their `\pos`.

A file in a directory you cannot write to (a library share, say) can be remuxed elsewhere:
`--tracks-only DIR` writes its fixed tracks to DIR and prints the mkvmerge command that remuxes
it to `.<name>.mkv.subcheck` beside the original; `--check` then checks that remux against the
original (with `--karaoke` too when the tracks were written with it; `--tracks-only` prints the
command), which takes mkvmerge here too, to read the original's languages: a remux made by an
mkvmerge that writes them another way fails the check. The swap is yours. Afterwards `subcheck
apply` finds the fixes in place and records them.
