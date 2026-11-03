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

# The line-coverage floor and the cargo-llvm-cov version, read from CI's job (which says why the
# floor is what it is), so this gate cannot check another floor or run another version than CI's.
# Each must come out as exactly one value: two matches, or none, stop the run.
ci=.forgejo/workflows/ci.yml
floor=$(sed -n 's/^ *floor=\([0-9][0-9]*\)$/\1/p' "$ci")
llvm_cov=$(sed -n 's#.*/cargo-llvm-cov/releases/download/v\([0-9.]*\)/.*#\1#p' "$ci")
[[ $floor =~ ^[0-9]+$ ]] || {
  echo "error: no single \`floor=N\` line in $ci to read the floor from" >&2
  exit 1
}
[[ $llvm_cov =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
  echo "error: no single cargo-llvm-cov release URL in $ci to read the version from" >&2
  exit 1
}

# Checked first, so a missing tool stops the run before the slow steps rather than after them. The
# version must be CI's: another could count lines differently, and pass a floor CI fails.
have=$(cargo llvm-cov --version 2>/dev/null) || have="none"
if [ "$have" != "cargo-llvm-cov $llvm_cov" ]; then
  echo "error: needs cargo-llvm-cov $llvm_cov, found: $have" >&2
  echo "       cargo install cargo-llvm-cov --locked --version $llvm_cov" >&2
  exit 1
fi
# Captured, not piped into `grep -q`: grep stops at the first match, and under pipefail a write by
# rustup after that can fail the check with the component installed.
components=$(rustup component list --installed)
grep -q '^llvm-tools' <<<"$components" || {
  echo "error: needs llvm-tools: rustup component add llvm-tools" >&2
  exit 1
}

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
# The tests, under coverage, against the floor. nextest when present, cargo test (CI's runner)
# otherwise: either runs the same instrumented test binaries, so the floor means the same under
# both. Doctests are run apart, uninstrumented: neither cargo llvm-cov nor nextest runs them.
if cargo nextest --version >/dev/null 2>&1; then
  tests=(cargo llvm-cov nextest)
else
  echo; echo "note: cargo-nextest not installed, using cargo test (slower)" >&2
  tests=(cargo llvm-cov)
fi
# cargo llvm-cov fails under the floor without saying so, hence the message.
run "${tests[@]}" --fail-under-lines "$floor" || {
  echo "error: cargo llvm-cov failed. If it built and every test passed, compare TOTAL's" >&2
  echo "       Lines Cover with the $floor% floor" >&2
  exit 1
}
run cargo test --doc

echo
echo "all green"
