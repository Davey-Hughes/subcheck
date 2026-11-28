#!/bin/bash
# Makes the bundle that cmake --install put at <prefix>/subcheck-qt.app, after Qt's deploy, into
# <prefix>/Subcheck.app: an app that loads nothing from outside itself but the system's libraries,
# with the CLI beside the window, the lowest macOS its files run on, the licences of what it
# carries, its name and a signature.
# bundle.cmake runs it as the install's last step.
#
#   gui/qt/macos/finish-bundle.sh <prefix>/subcheck-qt.app <cli> <mpvqt-prefix> <brew-prefix> \
#     <repo-root>
#
# The deploy (macdeployqt, and Qt's copy of the QML modules) brings in Qt's frameworks and plugins
# and the libraries it finds them and the window loading, and points their load commands inside the
# bundle. What it leaves pointing outside is put right here: the rpaths its files had where they
# were built, such as a Qt plugin's to Homebrew's lib directory; a QML module's files that it
# copied as links into Homebrew's Cellar; a library still loaded from outside, which is found by
# name in MpvQt's prefix, as MpvQt is built outside Homebrew, and in Homebrew's; and the ids it left
# as they were in Homebrew's prefix, such as each Qt framework's. Homebrew need not link a formula
# into its prefix (mpv is not, on some Macs), so its opt/<formula>/lib directories are searched too.
#
# It stops at the first failure, saying what failed; a library found nowhere is named.
#
# bash 3.2, as macOS ships it, and the system's own tools.
set -euo pipefail

die() {
  echo "error: finish-bundle.sh: $*" >&2
  exit 1
}
[ "$#" -eq 5 ] || {
  echo "usage: $0 <prefix>/subcheck-qt.app <cli> <mpvqt-prefix> <brew-prefix> <repo-root>" >&2
  exit 2
}
[ -f "$1/Contents/MacOS/subcheck-qt" ] || die "$1 is no bundle with Contents/MacOS/subcheck-qt"
[ "${1##*/}" = subcheck-qt.app ] || die "$1 is not named subcheck-qt.app"
[ -f "$2" ] && [ -x "$2" ] || die "the CLI, $2, is no executable file"
[ -d "$3/lib" ] || die "MpvQt's prefix, $3, has no lib directory"
[ -n "$4" ] || die "no Homebrew prefix was given: configure found no brew"
[ -d "$4" ] || die "Homebrew's prefix, $4, is not a directory"
[ -d "$5" ] || die "the repository, $5, is not a directory"
scripts=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)
built=$(cd "$1" && pwd -P)
cli=$2
mpvqt=$3
brew=$4
repo=$5
prefix=${built%/*}
contents=$built/Contents
macos=$contents/MacOS
frameworks=$contents/Frameworks

work=$(mktemp -d)
cleanup() { rm -rf "${work:?}"; }
trap cleanup EXIT

# install_name_tool warns, for each file it changes, that the change breaks the file's signature,
# which sign-bundle.sh makes again over the whole bundle at the end: those lines are left out, and
# any other is kept.
name_tool() {
  local out
  if ! out=$(install_name_tool "$@" 2>&1); then
    printf '%s\n' "$out" >&2
    die "install_name_tool $* failed"
  fi
  [ -z "$out" ] || printf '%s\n' "$out" | grep -v 'will invalidate the code signature' >&2 || true
}

# $1 as the bundle names it, from Contents/.
rel() { printf '%s' "${1#"$built"/}"; }

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

# True when $1 is a file whose real path, its links followed, is inside the bundle.
in_bundle() {
  local real
  [ -f "$1" ] || return 1
  real=$(realpath "$1" 2>/dev/null) || return 1
  case $real in "$built"/*) return 0 ;; esac
  return 1
}

# True when $1 is there, a file or a directory, and its real path is inside the bundle.
inside() {
  local real
  [ -e "$1" ] || return 1
  real=$(realpath "$1" 2>/dev/null) || return 1
  case $real in "$built"/*) return 0 ;; esac
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

# The bundle's Mach-O files, one a line in $machos, found as check-bundle.sh finds them, so that
# the two see the same files: otool -h says "Mach header" under each one's name.
machos=$work/machos
list_machos() {
  find "$contents" -type f -exec otool -h {} + 2>/dev/null >"$work/headers" || true
  awk '
    $0 == "Mach header" {
      f = prev
      sub(/ \(architecture [^)]*\):$/, "", f)
      sub(/:$/, "", f)
      if (!(f in seen)) { seen[f] = 1; print f }
    }
    { prev = $0 }
  ' "$work/headers" >"$machos"
}

# What the file $1 says of itself, a line each: `load<TAB><name>` for each library it loads,
# `rpath<TAB><path>` for each rpath, `id<TAB><name>` for a library's id and `minos<TAB><version>`
# for the macOS it was built for.
load_commands() {
  otool -l "$1" | awk '
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
  '
}

# The name a library has in Contents/Frameworks, from a name or path that ends in it: a
# framework's binary keeps its place inside the framework, Foo.framework/Versions/A/Foo, and a
# dylib is its file's name.
bundled_name() {
  local fw
  case $1 in
    *.framework/*)
      fw=${1%%.framework/*}
      printf '%s\n' "${fw##*/}.framework/${1#*.framework/}"
      ;;
    *) printf '%s\n' "${1##*/}" ;;
  esac
}

# Where on this Mac the library a file loads by the name $1 is: at the name itself, when it is a
# path that is there; else in the first of MpvQt's and Homebrew's library directories to hold it.
locate() {
  local name dir
  case $1 in
    /*)
      if [ -f "$1" ]; then
        printf '%s\n' "$1"
        return 0
      fi
      ;;
  esac
  name=$(bundled_name "$1")
  for dir in "$mpvqt/lib" "$brew/lib" "$brew"/opt/*/lib; do
    if [ -f "$dir/$name" ]; then
      printf '%s\n' "$dir/$name"
      return 0
    fi
  done
  return 1
}

# Copies the library at $1 into Contents/Frameworks as $2, its name there, unless one of that name
# is there already, and gives it the id that names it there. A framework is copied whole, its
# links kept, as its binary finds its resources through them. Homebrew's files are read-only, and
# install_name_tool must write them.
bring_in() {
  local from=$1 name=$2 top=${2%%/*}
  [ ! -e "$frameworks/$top" ] || return 0
  mkdir -p "$frameworks"
  case $name in
    *.framework/*) ditto "$(cd "${from%%.framework/*}.framework" && pwd -P)" "$frameworks/$top" ;;
    *) cp "$from" "$frameworks/$name" ;;
  esac
  chmod -R u+w "$frameworks/$top"
  name_tool -id "@rpath/$name" "$frameworks/$name"
  echo "finish-bundle.sh: brought in $from as Contents/Frameworks/$name"
  changed=1
}

# 1. The CLI, beside the window. Its name differs from the window's executable, subcheck-qt, in more
# than case, which macOS's disk ignores.
cp "$cli" "$macos/subcheck"
chmod 755 "$macos/subcheck"

# 2. What the deploy left outside. First the links it copied as links: where Homebrew's prefix
# merges a directory that several formulae fill, such as share/qt/qml/QtQuick, each file there is
# a link into the formula's own directory in the Cellar, by a relative path that inside the bundle
# leads nowhere. Each link in the bundle to nothing inside it is made a copy of the file it named,
# found in Homebrew's Cellar by the part of the link's path from Cellar/ on; one that names no file
# there fails the script. Links that stay inside, such as a framework's to its current version,
# are kept.
find "$contents" -type l >"$work/links"
while IFS= read -r link <&3; do
  ! inside "$link" || continue
  text=$(readlink "$link")
  case $text in
    Cellar/* | */Cellar/*) from=$brew/Cellar/${text#*Cellar/} ;;
    *) die "$(rel "$link") is a link to $text, which is nothing inside the bundle" ;;
  esac
  [ -f "$from" ] ||
    die "$(rel "$link") is a link to $text, which names no file in Homebrew's Cellar"
  rm -f "$link"
  cp "$from" "$link"
  chmod u+w "$link"
  echo "finish-bundle.sh: $(rel "$link") is a copy of $from, not a link to it"
done 3<"$work/links"

# Then the libraries. Each executable finds Contents/Frameworks by its rpath, and so does each
# library it loads, as dyld looks for an @rpath/ name in the rpaths of everything that led to the
# load, back to the executable: that rpath goes in first, so that a name it resolves is seen to be
# inside. Then, until a pass changes nothing, each name a file loads from outside the system that
# is not inside the bundle, by path or by an @rpath/ that none of its rpaths or the executables'
# makes a file inside, is brought in and loaded as @rpath/<its name there>.
for exe in "$macos/subcheck-qt" "$macos/subcheck"; do
  has=$(load_commands "$exe")
  case $'\n'$has$'\n' in
    *$'\n'rpath$'\t'@executable_path/../Frameworks$'\n'*) ;;
    *) name_tool -add_rpath @executable_path/../Frameworks "$exe" ;;
  esac
done
exe_rpaths=$frameworks
passes=0
changed=1
while [ "$changed" -eq 1 ]; do
  changed=0
  passes=$((passes + 1))
  # Each pass brings in a library that is not yet inside or names it as one that is: a pass
  # beyond the depth of any load chain is a loop.
  [ "$passes" -le 50 ] || die "the bundle still loads from outside after 50 passes"
  list_machos
  while IFS= read -r f <&3; do
    here=${f%/*}
    load_commands "$f" >"$work/lc"
    rpaths=$exe_rpaths
    while IFS=$'\t' read -r kind value; do
      [ "$kind" = rpath ] || continue
      case $value in
        @executable_path | @executable_path/* | @loader_path | @loader_path/*)
          rpaths=$rpaths$'\n'$(expand "$value" "$here")
          ;;
      esac
    done <"$work/lc"
    while IFS=$'\t' read -r kind value; do
      [ "$kind" = load ] || continue
      case $value in
        /System/* | /usr/lib/*) continue ;;
        @executable_path/* | @loader_path/*)
          ! in_bundle "$(expand "$value" "$here")" || continue
          ;;
        @rpath/*)
          found=
          while IFS= read -r rp; do
            if [ -n "$rp" ] && in_bundle "$(norm "$rp/${value#@rpath/}")"; then
              found=1
              break
            fi
          done <<EOF
$rpaths
EOF
          [ -z "$found" ] || continue
          ;;
      esac
      from=$(locate "$value") ||
        die "$(rel "$f") loads $value, which is in none of $mpvqt/lib, $brew/lib and" \
          "$brew/opt/*/lib"
      name=$(bundled_name "$value")
      bring_in "$from" "$name"
      if [ "$value" != "@rpath/$name" ]; then
        name_tool -change "$value" "@rpath/$name" "$f"
        echo "finish-bundle.sh: $(rel "$f") loads $value as @rpath/$name"
        changed=1
      fi
    done <"$work/lc"
  done 3<"$machos"
done

# Then no file keeps an rpath outside the bundle, where dyld would look for a library the bundle
# lacks and find it on the Mac that made the app alone. The system's own are kept.
while IFS= read -r f <&3; do
  load_commands "$f" >"$work/lc"
  while IFS=$'\t' read -r kind value; do
    [ "$kind" = rpath ] || continue
    case $value in
      /System/* | /usr/lib/*) continue ;;
      @executable_path | @executable_path/* | @loader_path | @loader_path/*)
        case $(expand "$value" "${f%/*}") in "$built" | "$built"/*) continue ;; esac
        ;;
    esac
    name_tool -delete_rpath "$value" "$f"
    echo "finish-bundle.sh: $(rel "$f") no longer has the rpath $value"
  done <"$work/lc"
done 3<"$machos"

# Then each library's id is a name of the bundle's own: the id is the name a program linked
# against the library loads it by, so one left as the deploy copied it, in Homebrew's prefix, sends
# such a program to Homebrew's copy. A library in Contents/Frameworks whose id is no @ name takes
# @rpath/<its name there>, as the bundle's loads name it; one elsewhere fails the script, as no
# rpath leads there.
while IFS= read -r f <&3; do
  load_commands "$f" >"$work/lc"
  while IFS=$'\t' read -r kind value; do
    [ "$kind" = id ] || continue
    case $value in @*) continue ;; esac
    case $f in
      "$frameworks"/*) name=${f#"$frameworks"/} ;;
      *)
        die "$(rel "$f") has the id $value, and is not in Contents/Frameworks, where @rpath/" \
          "leads"
        ;;
    esac
    name_tool -id "@rpath/$name" "$f"
    echo "finish-bundle.sh: $(rel "$f") has the id @rpath/$name, not $value"
  done <"$work/lc"
done 3<"$machos"

# 3. The lowest macOS the app runs on is the highest any of its files was built for, which the
# libraries Homebrew built for its own macOS set: a Mac older than that refuses the app in the
# Finder rather than failing in dyld.
top=
while IFS= read -r f <&3; do
  load_commands "$f" >"$work/lc"
  while IFS=$'\t' read -r kind value; do
    [ "$kind" = minos ] || continue
    if [ -z "$top" ] || version_lt "$top" "$value"; then top=$value; fi
  done <"$work/lc"
done 3<"$machos"
[ -n "$top" ] || die "no file in the bundle says the macOS it was built for"
/usr/libexec/PlistBuddy -c "Set :LSMinimumSystemVersion $top" "$contents/Info.plist"
echo "finish-bundle.sh: the app needs macOS $top or later"

# 4. The licences of what it carries, as licenses.sh says, before the signature, which then
# covers them.
"$scripts/licenses.sh" "$built" "$brew" "$repo"

# 5. Its name, in place of one an earlier install left.
rm -rf "${prefix:?}/Subcheck.app"
mv "$built" "$prefix/Subcheck.app"

# 6. The signature, last, over everything above.
"$scripts/sign-bundle.sh" "$prefix/Subcheck.app"
