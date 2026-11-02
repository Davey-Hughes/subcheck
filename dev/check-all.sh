#!/usr/bin/env bash
# The local mirror of CI (.forgejo/workflows/ci.yml, and its macOS counterpart in
# .github/workflows/ci.yml). Run it before you push a branch and open a PR: CI runs these same steps
# there, and this is them several minutes earlier.
#
#   dev/check-all.sh
#
# The point of this file is that the local gate and the CI gate cannot drift: CI's gating steps are
# reproduced here in the same order with the same commands. When a step changes there, change it
# here. The Rust is the same too: rust-toolchain.toml pins it for every cargo run in the repository,
# and CI's image is that version.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

[ "$#" -eq 0 ] || { echo "error: no arguments taken" >&2; exit 2; }

run() { echo; echo "==> $*"; "$@"; }

# A dev script no test runs can carry a stray quote or a missing `fi` to main with CI green.
echo; echo "==> bash -n over dev/"
for f in dev/*.sh; do bash -n "$f"; done

# As in CI: the `apply` tests fail, rather than skip, without mkvmerge, and the `audio` tests
# without ffmpeg.
export SUBCHECK_NEED_MKVMERGE=1 SUBCHECK_NEED_FFMPEG=1
# As in CI: which Rust this was, in the log (rust-toolchain.toml names it).
run rustc --version
run cargo fmt --all --check
run cargo clippy --all-targets -- -D warnings
# nextest when present, cargo test otherwise. Doctests are run explicitly either way: nextest
# cannot run them, so relying on it alone would drop them.
if cargo nextest --version >/dev/null 2>&1; then
  run cargo nextest run
else
  echo; echo "note: cargo-nextest not installed, using cargo test (slower)" >&2
  run cargo test
fi
run cargo test --doc

echo
echo "all green"
