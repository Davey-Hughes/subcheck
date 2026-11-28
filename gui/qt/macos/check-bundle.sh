#!/bin/bash
# Checks a Subcheck.app that cmake --install made on macOS, before anyone runs it: that it loads
# nothing from outside itself but the system, that it is signed and says what it is, that the
# window starts from it, and that the CLI inside runs. CI's gui-macos Release leg runs it on the app
# it installs, and it runs on a Mac by hand too; starts opens a window, so it needs the Mac's own
# session, not an ssh one.
#
#   gui/qt/macos/check-bundle.sh <path/to/Subcheck.app> [check...]
#
# With no check named it runs them all, otherwise only those named. It goes on past a failure, so
# that one run lists them all: a line `error: <check>: <what failed>` for each, on stderr, then
# exit 1. Exit 0 when every check it ran passed, and 2 when it is called wrong.
#
# self-contained: each Mach-O file in Contents/ loads only the system's libraries (under /System/
#   or /usr/lib/) and files inside the bundle, has no rpath outside it, and, a library, has an id
#   that names it inside the bundle. A name or rpath anywhere else, such as Homebrew's prefix, the
#   build directory or the runner's temporary directory, loads on the machine that made the app and
#   fails on a Mac without that file; an rpath outside is where dyld would look for a library the
#   bundle lacks; and an id is the name a program linked against the library loads it by.
# signed: codesign passes the signature, deep and strict, as macOS runs no arm64 code that is not
#   signed; and Info.plist has the identifier macOS knows the app by, the executable it starts and
#   the name it shows, and a minimum macOS no lower than any file inside was built for, so that a
#   Mac too old for one of its libraries refuses the app in the Finder rather than failing in dyld.
# starts: the window, on cocoa as from the Finder, in a near-empty environment and with a home,
#   store and settings of the check's own, says `subcheck-qt: no runs yet` on stderr and is still
#   running, in its dialog, 5 s later. The QML loads before the session says that, so the line
#   means every library, the platform plugin and every QML import loaded; a dyld, plugin or QML
#   error ends the app first. Started from here, its parent is not launchd, so it reads no login
#   shell. Its home is the check's too, so that nothing it writes reaches the real one.
# cli: Contents/MacOS/subcheck --version runs in a near-empty environment and says its version.
# licenses: Contents/Resources/licenses/, which licenses.sh writes, has a line in manifest.tsv for
#   each Mach-O file in Contents/ but the two executables in Contents/MacOS, which are subcheck's
#   own; licence files for each project the manifest names; subcheck's own LICENSE; and the Rust
#   crates' notices, in rust-crates.txt. And no bundled libav* file holds `--enable-nonfree`: each
#   of ffmpeg's libraries keeps the line it was configured with, and a build with nonfree parts may
#   not be passed on at all. libmpv links ffmpeg's libraries, so an app with no libav* file has
#   them under names this check does not look for, and fails it.
#
# self-contained is there beside starts because the runner has Homebrew: a bundle that still loads
# a library from /opt/homebrew starts there, and fails on any Mac without Homebrew's copy.
#
# bash 3.2, as macOS ships it, and the system's own tools.
set -euo pipefail

all='self-contained signed starts cli licenses'

[ "$#" -ge 1 ] || {
  echo "usage: $0 <path/to/Subcheck.app> [check...], the checks being: $all" >&2
  exit 2
}
given=$1
shift
run=$all
if [ "$#" -gt 0 ]; then
  for name in "$@"; do
    case " $all " in
      *" $name "*) ;;
      *)
        echo "error: no check is named $name; the checks are: $all" >&2
        exit 2
        ;;
    esac
  done
  run=$*
fi
wants() { case " $run " in *" $1 "*) return 0 ;; esac; return 1; }

need() {
  command -v "$1" >/dev/null || {
    echo "error: needs $1, $2" >&2
    exit 2
  }
}
if wants self-contained || wants signed || wants licenses; then
  need otool "from Xcode's command line tools"
  need realpath "which macOS has from 13"
fi
if wants signed; then
  need codesign "from Xcode's command line tools"
  need /usr/libexec/PlistBuddy "which macOS ships"
fi

[ -d "$given/Contents" ] || {
  echo "error: $given is not an app: it has no Contents/ directory" >&2
  exit 1
}
# Its real path, as realpath gives a file's, so that a file inside is seen to be inside.
app=$(cd "$given" && pwd -P)
contents=$app/Contents
macos=$contents/MacOS

pid=
work=$(mktemp -d)
cleanup() {
  if [ -n "$pid" ]; then
    kill -KILL "$pid" 2>/dev/null || true
  fi
  rm -rf "${work:?}"
}
trap cleanup EXIT

failures=0
# An error line for the check $1, saying the rest.
fail() {
  local check=$1
  shift
  echo "error: $check: $*" >&2
  failures=$((failures + 1))
}
# A program's output, under the error line that quotes it.
indent() { sed 's/^/    /' >&2; }
# $1 as the bundle names it, from Contents/.
rel() { printf '%s' "${1#"$app"/}"; }

# The absolute path $1 with its empty, . and .. parts taken out, as dyld reads it, whether or not
# it is there.
norm() {
  local rest=$1/ part out=''
  while [ -n "$rest" ]; do
    part=${rest%%/*}
    rest=${rest#*/}
    case $part in
      '' | .) ;;
      ..) out=${out%/*} ;;
      *) out=$out/$part ;;
    esac
  done
  printf '%s\n' "${out:-/}"
}

# The path dyld makes of the name or rpath $1 in a file in the directory $2: @executable_path is
# Contents/MacOS, where both executables are, and @loader_path the file's own directory.
expand() {
  case $1 in
    @executable_path | @executable_path/*) norm "$macos/${1#@executable_path}" ;;
    @loader_path | @loader_path/*) norm "$2/${1#@loader_path}" ;;
    *) printf '%s\n' "$1" ;;
  esac
}

# True when $1 is a file whose real path, its links followed, is inside the bundle: a link inside
# that points out loads from outside.
in_bundle() {
  local real
  [ -f "$1" ] || return 1
  real=$(realpath "$1" 2>/dev/null) || return 1
  case $real in "$app"/*) return 0 ;; esac
  return 1
}

# True when the dotted version $1 is lower than $2, a missing part being 0.
version_lt() {
  local a=$1. b=$2. x y
  while [ -n "$a$b" ]; do
    x=${a%%.*}
    y=${b%%.*}
    a=${a#*.}
    b=${b#*.}
    if [ "${x:-0}" -lt "${y:-0}" ]; then return 0; fi
    if [ "${x:-0}" -gt "${y:-0}" ]; then return 1; fi
  done
  return 1
}
is_version() {
  case $1 in '' | .* | *. | *..* | *[!0-9.]*) return 1 ;; esac
  return 0
}

# The bundle's Mach-O files, one a line in $machos, and what the checks read of each one's load
# commands, in $lc/<its line's number>: `load<TAB><name>` for each library it loads, weakly or
# not, `rpath<TAB><path>` for each rpath, `id<TAB><name>` for a library's id and
# `minos<TAB><version>` for the macOS it was built for.
# A file otool -l cannot read has no $lc entry, and what otool -h could not read is in $unread:
# each check that reads them reports either, as a file it skipped is one it did not check.
machos=$work/machos
lc=$work/lc
unread=$work/unread
read_load_commands() {
  local f n=0
  mkdir "$lc"
  # otool -h says "<file>:", or "<file> (architecture <arch>):" for each part of a universal
  # file, then "Mach header" for a Mach-O file, and "<file>: is not an object file" for another.
  find "$contents" -type f -exec otool -h {} + >"$work/headers" 2>"$unread" ||
    echo "otool -h ended with status $?" >>"$unread"
  awk '
    $0 == "Mach header" {
      f = prev
      sub(/ \(architecture [^)]*\):$/, "", f)
      sub(/:$/, "", f)
      if (!(f in seen)) { seen[f] = 1; print f }
    }
    { prev = $0 }
  ' "$work/headers" >"$machos"
  while IFS= read -r f <&3; do
    n=$((n + 1))
    otool -l "$f" >"$work/l" 2>/dev/null || continue
    awk '
      $1 == "cmd" { cmd = $2; next }
      $1 == "name" && cmd ~ /^LC_(LOAD|LOAD_WEAK|REEXPORT|LOAD_UPWARD|LAZY_LOAD)_DYLIB$/ {
        v = $0; sub(/^ *name /, "", v); sub(/ \(offset [0-9]+\)$/, "", v); print "load\t" v; next
      }
      $1 == "name" && cmd == "LC_ID_DYLIB" {
        v = $0; sub(/^ *name /, "", v); sub(/ \(offset [0-9]+\)$/, "", v); print "id\t" v; next
      }
      $1 == "path" && cmd == "LC_RPATH" {
        v = $0; sub(/^ *path /, "", v); sub(/ \(offset [0-9]+\)$/, "", v); print "rpath\t" v; next
      }
      $1 == "minos" && cmd == "LC_BUILD_VERSION" { print "minos\t" $2; next }
    ' "$work/l" >"$lc/$n"
  done 3<"$machos"
}

check_self_contained() {
  local f n at here kind value p rp found rpaths exe_rpaths=''
  if [ -s "$unread" ]; then
    fail self-contained "otool -h cannot read every file in Contents/:"
    indent <"$unread"
  fi
  [ -s "$machos" ] || {
    fail self-contained "otool reads no Mach-O file in Contents/"
    return
  }
  # dyld looks for a library's @rpath/ name in the library's own rpaths, then in those of what
  # loaded it, which leads back to an executable.
  n=0
  while IFS= read -r f <&3; do
    n=$((n + 1))
    [ "${f%/*}" = "$macos" ] && [ -f "$lc/$n" ] || continue
    while IFS=$'\t' read -r kind value <&4; do
      [ "$kind" = rpath ] || continue
      case $value in
        @*)
          p=$(expand "$value" "$macos")
          case $p in "$app" | "$app"/*) exe_rpaths=$exe_rpaths$p$'\n' ;; esac
          ;;
      esac
    done 4<"$lc/$n"
  done 3<"$machos"
  n=0
  while IFS= read -r f <&3; do
    n=$((n + 1))
    at=$(rel "$f")
    here=${f%/*}
    [ -f "$lc/$n" ] || {
      fail self-contained "otool -l cannot read $at"
      continue
    }
    rpaths=
    while IFS=$'\t' read -r kind value <&4; do
      [ "$kind" = rpath ] || continue
      case $value in
        /System/* | /usr/lib/*) ;;
        /*) fail self-contained "$at has the rpath $value, outside the bundle" ;;
        @executable_path | @executable_path/* | @loader_path | @loader_path/*)
          p=$(expand "$value" "$here")
          case $p in
            "$app" | "$app"/*) rpaths=$rpaths$p$'\n' ;;
            *) fail self-contained "$at has the rpath $value, which is $p, outside the bundle" ;;
          esac
          ;;
        *)
          fail self-contained "$at has the rpath $value, which dyld reads from the" \
            "working directory"
          ;;
      esac
    done 4<"$lc/$n"
    while IFS=$'\t' read -r kind value <&4; do
      [ "$kind" = load ] || continue
      case $value in
        /System/* | /usr/lib/*) ;;
        /*) fail self-contained "$at loads $value, outside the bundle" ;;
        @executable_path/* | @loader_path/*)
          in_bundle "$(expand "$value" "$here")" ||
            fail self-contained "$at loads $value, which is no file inside the bundle"
          ;;
        @rpath/*)
          found=
          while IFS= read -r rp; do
            if [ -n "$rp" ] && in_bundle "$(norm "$rp/${value#@rpath/}")"; then
              found=1
              break
            fi
          done <<EOF
$rpaths$exe_rpaths
EOF
          [ -n "$found" ] ||
            fail self-contained "$at loads $value, which none of its rpaths or the executables'" \
              "makes a file inside the bundle"
          ;;
        *) fail self-contained "$at loads $value, which dyld looks for outside the bundle" ;;
      esac
    done 4<"$lc/$n"
    # The id leads, as a load of it would from here, to this file: the first rpath under which an
    # @rpath/ name is a file is the one dyld takes.
    while IFS=$'\t' read -r kind value <&4; do
      [ "$kind" = id ] || continue
      p=
      case $value in
        /*)
          fail self-contained "$at has the id $value, outside the bundle"
          continue
          ;;
        @executable_path/* | @loader_path/*) p=$(expand "$value" "$here") ;;
        @rpath/*)
          while IFS= read -r rp; do
            if [ -n "$rp" ] && [ -e "$(norm "$rp/${value#@rpath/}")" ]; then
              p=$(norm "$rp/${value#@rpath/}")
              break
            fi
          done <<EOF
$rpaths$exe_rpaths
EOF
          ;;
      esac
      [ -n "$p" ] && in_bundle "$p" && [ "$(realpath "$p")" = "$f" ] ||
        fail self-contained "$at has the id $value, which does not name it inside the bundle"
    done 4<"$lc/$n"
  done 3<"$machos"
}

check_signed() {
  local out pair key want have f n kind value top='' topfile='' plist=$contents/Info.plist
  if ! out=$(codesign --verify --deep --strict "$app" 2>&1); then
    fail signed "codesign --verify --deep --strict does not pass it:"
    printf '%s\n' "$out" | indent
  fi
  if [ -s "$unread" ]; then
    fail signed "otool -h cannot read every file in Contents/, so the macOS each was built for is" \
      "not known:"
    indent <"$unread"
  fi
  [ -f "$plist" ] || {
    fail signed "there is no Contents/Info.plist"
    return
  }
  for pair in CFBundleIdentifier=com.daveyhughes.subcheck CFBundleExecutable=subcheck-qt \
    CFBundleName=Subcheck; do
    key=${pair%%=*}
    want=${pair#*=}
    if ! have=$(/usr/libexec/PlistBuddy -c "Print :$key" "$plist" 2>/dev/null); then
      fail signed "Info.plist has no $key, which should be $want"
    elif [ "$have" != "$want" ]; then
      fail signed "Info.plist's $key is $have, not $want"
    fi
  done
  n=0
  while IFS= read -r f <&3; do
    n=$((n + 1))
    [ -f "$lc/$n" ] || {
      fail signed "otool -l cannot read $(rel "$f"), so the macOS it was built for is not known"
      continue
    }
    while IFS=$'\t' read -r kind value <&4; do
      [ "$kind" = minos ] || continue
      if ! is_version "$value"; then
        fail signed "$(rel "$f") has the minos $value, which is not a version"
      elif [ -z "$top" ] || version_lt "$top" "$value"; then
        top=$value
        topfile=$f
      fi
    done 4<"$lc/$n"
  done 3<"$machos"
  if ! have=$(/usr/libexec/PlistBuddy -c "Print :LSMinimumSystemVersion" "$plist" 2>/dev/null); then
    fail signed "Info.plist has no LSMinimumSystemVersion${top:+, which should be $top or higher}"
  elif ! is_version "$have"; then
    fail signed "Info.plist's LSMinimumSystemVersion is $have, which is not a version"
  elif [ -n "$top" ] && version_lt "$have" "$top"; then
    fail signed "Info.plist's LSMinimumSystemVersion is $have, lower than the minos $top of" \
      "$(rel "$topfile")"
  fi
}

check_starts() {
  local exe=$macos/subcheck-qt home status=0 _
  [ -f "$exe" ] && [ -x "$exe" ] || {
    fail starts "there is no executable Contents/MacOS/subcheck-qt"
    return
  }
  home=$(mktemp -d "$work/home.XXXXXX")
  # No QT_QPA_PLATFORM, so cocoa, as from the Finder. CFFIXED_USER_HOME is the home macOS's own
  # settings store takes, which the window's settings use when XDG_CONFIG_HOME is not set.
  # QT_FORCE_STDERR_LOGGING, as with no terminal macOS's Qt sends its warnings and fatal errors to
  # the system log: a plugin that does not load or a QML import that fails would end the app with
  # nothing here to say why. It moves only where Qt's messages go.
  env -i HOME="$home" CFFIXED_USER_HOME="$home" PATH=/usr/bin:/bin SUBCHECK_DB="$home/subcheck.db" \
    XDG_STATE_HOME="$home/state" SUBCHECK_CONFIG="$home/config.toml" QT_FORCE_STDERR_LOGGING=1 \
    "$exe" >/dev/null 2>"$work/starts.err" &
  pid=$!
  sleep 5
  if kill -0 "$pid" 2>/dev/null; then
    # The shell says that a job a signal ended was `Terminated` when it next looks at its jobs,
    # which can be after any of these commands, so the block's stderr goes nowhere: the app's own
    # is in starts.err, and none of these commands has a failure to tell.
    {
      kill "$pid" || true
      for _ in 1 2 3 4 5 6 7 8 9 10; do
        kill -0 "$pid" || break
        sleep 0.2
      done
      kill -KILL "$pid" || true
      wait "$pid" || true
    } 2>/dev/null
    pid=
    if ! grep -qxF 'subcheck-qt: no runs yet' "$work/starts.err"; then
      fail starts "subcheck-qt ran 5 s and did not say \`subcheck-qt: no runs yet\`; it said:"
      indent <"$work/starts.err"
    fi
  else
    wait "$pid" 2>/dev/null || status=$?
    pid=
    fail starts "subcheck-qt ended, with status $status, within 5 s; it said:"
    indent <"$work/starts.err"
  fi
}

check_cli() {
  local exe=$macos/subcheck out status=0
  [ -f "$exe" ] && [ -x "$exe" ] || {
    fail cli "there is no executable Contents/MacOS/subcheck"
    return
  }
  out=$(env -i PATH=/usr/bin:/bin "$exe" --version 2>"$work/cli.err") || status=$?
  if [ "$status" -ne 0 ]; then
    fail cli "subcheck --version ended with status $status; it said:"
  elif ! printf '%s\n' "$out" | grep -q '^subcheck '; then
    fail cli "subcheck --version said no line that starts \`subcheck \`; it said:"
  else
    return 0
  fi
  {
    [ -z "$out" ] || printf '%s\n' "$out"
    cat "$work/cli.err"
  } | indent
}

check_licenses() {
  local dir=$contents/Resources/licenses f project
  local manifest=$dir/manifest.tsv
  if [ -s "$unread" ]; then
    fail licenses "otool -h cannot read every file in Contents/, so not every library is known:"
    indent <"$unread"
  fi
  if [ ! -f "$manifest" ]; then
    fail licenses "there is no Contents/Resources/licenses/manifest.tsv"
  else
    # Its first line names the columns.
    awk -F '\t' 'NR == 1 && $1 == "file" { next } { print $1 }' "$manifest" >"$work/listed"
    while IFS= read -r f <&3; do
      case $f in "$macos/subcheck-qt" | "$macos/subcheck") continue ;; esac
      grep -qxF "${f#"$contents"/}" "$work/listed" ||
        fail licenses "$(rel "$f") has no line in Contents/Resources/licenses/manifest.tsv"
    done 3<"$machos"
    awk -F '\t' 'NR == 1 && $1 == "file" { next } { print $2 }' "$manifest" | sort -u \
      >"$work/projects"
    while IFS= read -r project <&3; do
      if [ -z "$project" ]; then
        fail licenses "a line in Contents/Resources/licenses/manifest.tsv names no project"
      elif [ -z "$(ls -A "$dir/$project" 2>/dev/null)" ]; then
        fail licenses "the manifest names $project, and Contents/Resources/licenses/$project/" \
          "holds no licence file"
      fi
    done 3<"$work/projects"
  fi
  [ -s "$dir/subcheck/LICENSE" ] ||
    fail licenses "there is no Contents/Resources/licenses/subcheck/LICENSE, or it is empty"
  [ -s "$dir/rust-crates.txt" ] ||
    fail licenses "there is no Contents/Resources/licenses/rust-crates.txt, or it is empty"
  find "$contents" -type f -name 'libav*' >"$work/libav"
  [ -s "$work/libav" ] ||
    fail licenses "there is no libav* file in Contents/, so no ffmpeg library was checked for" \
      "--enable-nonfree"
  while IFS= read -r f <&3; do
    if LC_ALL=C grep -qaF -e --enable-nonfree "$f"; then
      fail licenses "$(rel "$f") is from an ffmpeg configured --enable-nonfree, which may not be" \
        "passed on"
    fi
  done 3<"$work/libav"
}

if wants self-contained || wants signed || wants licenses; then
  read_load_commands
fi
for check in $all; do
  wants "$check" || continue
  before=$failures
  case $check in
    self-contained) check_self_contained ;;
    signed) check_signed ;;
    starts) check_starts ;;
    cli) check_cli ;;
    licenses) check_licenses ;;
  esac
  if [ "$failures" -eq "$before" ]; then
    echo "ok: $check"
  fi
done
[ "$failures" -eq 0 ] || exit 1
