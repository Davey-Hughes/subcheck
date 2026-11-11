#!/usr/bin/env bash
# The local mirror of the gui job in .forgejo/workflows/ci.yml: for the Qt app under gui/qt/,
# clang-format, qmllint, the build through the C++ bindings generated from ffi/, the tests, and the
# C++ floor. Run it, as well as dev/check-all.sh, before you push a branch that changes gui/ or any
# of the Rust: the bindings build on core, and the tests' seed runs cli's `subcheck scan`.
#
#   dev/check-gui.sh
#
# The job's steps, in its order, with its commands; when a step changes there, change it here.
# check-all.sh stays Rust-only, so that it still works on a machine without the C++ tools.
#
# Needs cmake, ninja, clang-format, Corrosion, Qt 6.12 (Quick, Test and qmllint), gcovr with the
# gcov of the compiler CMake picks, and the bindgen at CI's tag, installed with `cargo install`
# (the error says how). The build directory is SUBCHECK_GUI_BUILD, or one per checkout under the
# XDG cache: a CMake build directory belongs to one source directory, and it is large, with a
# build of the Rust inside.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

[ "$#" -eq 0 ] || { echo "error: no arguments taken" >&2; exit 2; }

run() { echo; echo "==> $*"; "$@"; }

# The C++ line floor, read from CI's job (which says why it is what it is), so that this gate
# cannot check another; and the bindgen's tag, read from both CI files, which must agree, so that
# this generates with the bindgen both jobs install. The bindgen has no --version, so the installed
# one's tag comes from cargo's list of its install root. gui/qt/CMakeLists.txt finds the copy
# there, but for the cases its comment names, and regenerates the bindings when a newer copy is
# installed, so a reused build directory keeps none from an older copy. Each must come out as
# exactly one value: two matches, or none, stop the run.
ci=.forgejo/workflows/ci.yml
mac=.github/workflows/ci.yml
floor=$(sed -n 's/^ *cpp_floor=\([0-9][0-9]*\)$/\1/p' "$ci")
[[ $floor =~ ^[0-9]+$ ]] || {
  echo "error: no single \`cpp_floor=N\` line in $ci to read the floor from" >&2
  exit 1
}
tag_in() { sed -n 's#.*cargo install uniffi-bindgen-cpp .* --tag \([^ ]*\) .*#\1#p' "$1"; }
tag=$(tag_in "$ci")
[[ $tag =~ ^v[0-9.]+\+v[0-9.]+$ ]] || {
  echo "error: no single \`cargo install uniffi-bindgen-cpp\` line in $ci to read the tag from" >&2
  exit 1
}
[ "$(tag_in "$mac")" = "$tag" ] || {
  echo "error: $mac does not install uniffi-bindgen-cpp $tag, as $ci does, once" >&2
  exit 1
}
# Checked before the slow steps, as it is needed only after them.
command -v gcovr >/dev/null || {
  echo "error: needs gcovr, for the C++ floor: https://gcovr.com" >&2
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
run cmake -S gui/qt -B "$build" -G Ninja -DCMAKE_BUILD_TYPE=Debug -DSUBCHECK_COVERAGE=ON
run cmake --build "$build" --target all_qmllint
run cmake --build "$build"
# The counts an earlier run left in this build directory would add to this run's, and a line only
# that run reached would pass the floor here and fail it in CI's fresh build.
find "${build:?}" -name '*.gcda' -delete
# As in CI: finding no tests fails, as CTest alone passes with none run.
run ctest --test-dir "$build" --output-on-failure --no-tests=error
run gcovr --root . --filter gui/qt/src/ --exclude gui/qt/src/main.cpp --merge-lines \
  --fail-under-line "$floor" "$build"

echo
echo "all green"
