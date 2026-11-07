#!/usr/bin/env bash
# The local mirror of the gui job in .forgejo/workflows/ci.yml: the Qt app's build and tests
# (gui/qt/), through the C++ bindings generated from ffi/. Run it, as well as dev/check-all.sh,
# before you push a branch that changes gui/ or any of the Rust: the bindings build on core, and the
# tests' seed runs cli's `subcheck scan`.
#
#   dev/check-gui.sh
#
# The job's steps, in its order, with its commands; when a step changes there, change it here.
# check-all.sh stays Rust-only, so that it still works on a machine without the C++ tools.
#
# Needs cmake, ninja, clang-format, Corrosion, and the bindgen at CI's tag, installed with
# `cargo install` (the error says how). The build directory is SUBCHECK_GUI_BUILD, or one per
# checkout under the XDG cache: a CMake build directory belongs to one source directory, and it is
# large, with a build of the Rust inside.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

[ "$#" -eq 0 ] || { echo "error: no arguments taken" >&2; exit 2; }

run() { echo; echo "==> $*"; "$@"; }

# The bindgen's tag, read from CI's job, so that this generates with the bindgen CI installs. The
# bindgen has no --version, so the installed one's tag comes from cargo's list of its install root.
# gui/qt/CMakeLists.txt finds the copy there, but for the cases its comment names, and regenerates
# the bindings when a newer copy is installed, so a reused build directory keeps none from an older
# copy. The tag must come out as exactly one value: two matches, or none, stop the run.
ci=.forgejo/workflows/ci.yml
tag=$(sed -n 's#.*cargo install uniffi-bindgen-cpp .* --tag \([^ ]*\) .*#\1#p' "$ci")
[[ $tag =~ ^v[0-9.]+\+v[0-9.]+$ ]] || {
  echo "error: no single \`cargo install uniffi-bindgen-cpp\` line in $ci to read the tag from" >&2
  exit 1
}
installed=$(cargo install --list)
have=$(sed -n 's|^uniffi-bindgen-cpp .*[?&]tag=\([^#&]*\)#.*|\1|p' <<<"$installed")
if [ "$have" != "$tag" ]; then
  echo "error: needs uniffi-bindgen-cpp $tag, found: ${have:-none}" >&2
  echo "       cargo install uniffi-bindgen-cpp --git https://github.com/NordSecurity/uniffi-bindgen-cpp --tag $tag --locked" >&2
  exit 1
fi

key=$(printf '%s' "$PWD" | cksum | cut -d' ' -f1)
build=${SUBCHECK_GUI_BUILD:-${XDG_CACHE_HOME:-$HOME/.cache}/subcheck-check-gui/$key}

# As in CI: which Rust this was, in the log (rust-toolchain.toml names it).
run rustc --version
# Untracked files too, unlike the job's clean checkout, so that a new file is checked before `git
# add`; and only files that exist, as a tracked file removed with `rm` is still listed, and
# clang-format stops on a path that is not there. The list goes through a file first, so that a
# git that fails stops the run, where a partial list could pass. An empty list stops it, as
# clang-format given no file reads an empty stdin and passes.
echo; echo "==> clang-format over gui/qt/"
listed=$(mktemp)
trap 'rm -f "$listed"' EXIT
git ls-files -z --cached --others --exclude-standard 'gui/qt/*.cpp' 'gui/qt/*.h' 'gui/qt/*.hpp' >"$listed"
cpp=()
while IFS= read -r -d '' f; do
  if [ -e "$f" ]; then cpp+=("$f"); fi
done <"$listed"
[ "${#cpp[@]}" -gt 0 ] || {
  echo "error: no C++ under gui/qt/ for clang-format to check" >&2
  exit 1
}
clang-format --dry-run -Werror "${cpp[@]}"
run cmake -S gui/qt -B "$build" -G Ninja -DCMAKE_BUILD_TYPE=Debug
run cmake --build "$build"
# As in CI: finding no tests fails, as CTest alone passes with none run.
run ctest --test-dir "$build" --output-on-failure --no-tests=error

echo
echo "all green"
