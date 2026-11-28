#!/bin/bash
# Puts the licences of what a Subcheck.app carries in its Contents/Resources/licenses/, with a list
# of each library's exact version and a link to its source: the app carries GPL and LGPL libraries,
# which may be passed on only with their licences and word of where their source is, and its
# executables link Rust crates under MIT and Apache licences, which ask that their notices go with
# every copy. finish-bundle.sh runs it before the signature, which then covers what it writes.
#
#   gui/qt/macos/licenses.sh <path/to/the.app> <brew-prefix> <repo-root>
#
# It writes:
#   manifest.tsv      a line that names the columns, then one for each Mach-O file in Contents/ but
#                     the two executables in Contents/MacOS, which are subcheck's own:
#                     file<TAB>project<TAB>version<TAB>licence<TAB>source<TAB>formula, being its
#                     path from Contents/, the Homebrew formula it is from or MpvQt, that project's
#                     version as bundled, its licence, where the source of that version is, and the
#                     formula that built it, which MpvQt, built by CI, has none of.
#   <project>/        the licence files of each project the manifest names.
#   formulae/         each formula as its keg holds it, which says how Homebrew built it from the
#                     source, with its patches: the source a GPL library is passed on with is the
#                     source it was built from, and the way it was built.
#   subcheck/LICENSE  the repository's own.
#   rust-crates.txt   each Rust crate the executables are built from, but the workspace's own, with
#                     its version, licence and repository and the licence files it ships.
#
# A library is matched to its formula by the path it had in the formula's keg, which its place in
# the bundle gives: Contents/Frameworks/<name>, a dylib or a framework's binary, was the keg's
# lib/<name>; Contents/PlugIns/<type>/<name>, a Qt plugin, was share/qt/plugins/<type>/<name>;
# Contents/PlugIns/<name>, the plugin of a QML module, which Qt's deploy puts there, was in that
# module's directory under share/qt/qml/; and Contents/Resources/qml/<path> was share/qt/qml/<path>.
# The keg, Cellar/<formula>/<version>, gives the project and its version. Where two kegs hold that
# path, as when an older version of a formula is still in the Cellar, the one whose file has the
# bundled file's UUID is the one it was copied from: install_name_tool and codesign change a file's
# load commands and signature, not its UUID. A file that no keg holds fails the script, naming it.
#
# A formula's source, licence and formula are taken from its keg, which records the version
# installed: the source archive and the licence its sbom.spdx.json names, its own copy of the
# formula, and the homebrew-core commit it was installed from, where INSTALL_RECEIPT.json records
# one, which an install from Homebrew's API does not. brew info stands in for what a keg does not
# record, such as the SBOM an older Homebrew did not write, only when its formula is the version
# installed, as it speaks of the formula as Homebrew has it now. A formula that neither gives fails
# the script, naming it and what it lacks.
#
# A formula's licence files are those at its keg's root, which Homebrew copies there from the
# source. A formula whose keg has none takes gui/qt/macos/licenses/<formula>/, which holds them
# from its source: qtbase's, qtdeclarative's and qtsvg's from Qt 6.12.0, as Qt keeps its licences
# in a LICENSES/ directory, which Homebrew does not copy, and glib's from 2.90.2, whose keg has its
# licence only under the licence's own name. A bump of one of them that changes its licences copies
# them there again, as nothing here can tell that the copies are out of date. A formula with neither
# fails the script, naming it.
#
# MpvQt, which Homebrew does not package, is built from its v1.2.1 tag, as CI builds it, and
# gui/qt/macos/licenses/MpvQt/ holds that tag's LICENSES/: a bundled MpvQt whose library says it is
# another version fails the script.
#
# bash 3.2, as macOS ships it, and the system's own tools, with cargo and Homebrew's brew.
set -euo pipefail

die() {
  echo "error: licenses.sh: $*" >&2
  exit 1
}
[ "$#" -eq 3 ] || {
  echo "usage: $0 <path/to/the.app> <brew-prefix> <repo-root>" >&2
  exit 2
}
[ -d "$1/Contents" ] || die "$1 is not an app: it has no Contents/ directory"
[ -d "$2/Cellar" ] && [ -x "$2/bin/brew" ] ||
  die "$2 is not Homebrew's prefix: it has no Cellar/ and bin/brew"
[ -f "$3/LICENSE" ] && [ -f "$3/Cargo.lock" ] ||
  die "$3 is not the repository: it has no LICENSE and Cargo.lock"
command -v otool >/dev/null || die "needs otool, from Xcode's command line tools"
command -v jq >/dev/null || die "needs jq, which macOS has from 15"
command -v cargo >/dev/null || die "needs cargo"
app=$(cd "$1" && pwd -P)
cellar=$(cd "$2/Cellar" && pwd -P)
brew=$2/bin/brew
repo=$(cd "$3" && pwd -P)
contents=$app/Contents
out=$contents/Resources/licenses
ours=$repo/gui/qt/macos/licenses
tab=$(printf '\t')
# What the crate list's fields are split on, not a tab: a crate need give no licence or repository,
# and read takes two tabs in a row as one, as it does spaces.
unit=$(printf '\037')

mpvqt_version=1.2.1
mpvqt_licence='LGPL-2.1-only OR LGPL-3.0-only OR LicenseRef-KDE-Accepted-LGPL'
mpvqt_commit=8a4d1ae9d1a1c5c94cd96bccb5330b83c1704b99
mpvqt_source="https://invent.kde.org/libraries/mpvqt/-/tree/v1.2.1"
mpvqt_source="$mpvqt_source (tag v1.2.1, commit $mpvqt_commit)"

work=$(mktemp -d)
cleanup() { rm -rf "${work:?}"; }
trap cleanup EXIT

# The files at the top of the directory $1 whose names say they are licences, one a line, in an
# order that does not change from one Mac to the next.
licence_files() {
  find "$1" -mindepth 1 -maxdepth 1 -type f \( -iname 'LICENSE*' -o -iname 'LICENCE*' \
    -o -iname 'COPYING*' -o -iname 'NOTICE*' -o -iname 'COPYRIGHT*' \) | LC_ALL=C sort
}

# The UUID of the Mach-O file $1, or nothing when it has none.
uuid() {
  otool -l "$1" 2>/dev/null | awk '$1 == "uuid" && id == "" { id = $2 } END { print id }' || true
}

# What an earlier run left goes first, so that nothing in it outlives the library it was for.
rm -rf "${out:?}"
mkdir -p "$out"

# The bundle's Mach-O files, found as check-bundle.sh finds them, so that the manifest has a line
# for each file the check looks for: otool -h says "Mach header" under each one's name.
find "$contents" -type f -exec otool -h {} + >"$work/headers" 2>"$work/unread" ||
  die "otool -h ended with status $?"
[ ! -s "$work/unread" ] || die "otool -h cannot read every file in Contents/: $(cat "$work/unread")"
awk '
  $0 == "Mach header" {
    f = prev
    sub(/ \(architecture [^)]*\):$/, "", f)
    sub(/:$/, "", f)
    if (!(f in seen)) { seen[f] = 1; print f }
  }
  { prev = $0 }
' "$work/headers" >"$work/machos"

# Each file but the executables, and the path in a keg it was copied from: a path, or qml:<name>
# for a QML module's plugin, whose module is not known here.
: >"$work/keys"
: >"$work/files"
while IFS= read -r f <&3; do
  case $f in "$contents/MacOS/subcheck-qt" | "$contents/MacOS/subcheck") continue ;; esac
  rel=${f#"$contents"/}
  case $rel in
    Frameworks/libMpvQt.*)
      version=$(otool -l "$f" | awk '
        $1 == "cmd" { cmd = $2 }
        cmd == "LC_ID_DYLIB" && $1 == "current" && $2 == "version" && v == "" { v = $3 }
        END { print v }
      ') || die "otool -l cannot read $rel"
      [ "$version" = "$mpvqt_version" ] ||
        die "$rel is MpvQt ${version:-of no version}, and gui/qt/macos/licenses/MpvQt/ holds" \
          "the licences of $mpvqt_version"
      printf '%s\t%s\t%s\n' "$rel" MpvQt "$mpvqt_version" >>"$work/files"
      continue
      ;;
    Frameworks/*) key=lib/${rel#Frameworks/} ;;
    PlugIns/*/*) key=share/qt/plugins/${rel#PlugIns/} ;;
    PlugIns/*) key=qml:${rel#PlugIns/} ;;
    Resources/qml/*) key=share/qt/qml/${rel#Resources/qml/} ;;
    *) key= ;;
  esac
  printf '%s\t%s\n' "$rel" "$key" >>"$work/keys"
done 3<"$work/machos"

# One index of the Cellar: what is under each keg's lib/ and share/qt/, where the bundle's libraries
# and plugins were. Links count, as a library's name is often a link to its file. Then each match,
# `<file><TAB><formula><TAB><version><TAB><the keg's file>`.
find "$cellar" -mindepth 4 \( -path "$cellar/*/*/lib/*" -o -path "$cellar/*/*/share/qt/*" \) \
  \( -type f -o -type l \) >"$work/cellar"
awk -F '\t' -v cellar="$cellar/" '
  FILENAME == ARGV[1] { if ($2 != "") want[$2] = $1; next }
  {
    p = substr($0, length(cellar) + 1)
    i = index(p, "/"); formula = substr(p, 1, i - 1); p = substr(p, i + 1)
    i = index(p, "/"); version = substr(p, 1, i - 1); p = substr(p, i + 1)
    if (p in want) print want[p] "\t" formula "\t" version "\t" $0
    if (substr(p, 1, 13) == "share/qt/qml/") {
      n = p
      sub(/.*\//, "", n)
      if (("qml:" n) in want) print want["qml:" n] "\t" formula "\t" version "\t" $0
    }
  }
' "$work/keys" "$work/cellar" >"$work/matches"

missing=
while IFS="$tab" read -r rel _ <&3; do
  awk -F '\t' -v rel="$rel" '$1 == rel { print $2 "\t" $3 "\t" $4 }' "$work/matches" \
    >"$work/these"
  cut -f1,2 "$work/these" | sort -u >"$work/kegs"
  if [ ! -s "$work/kegs" ]; then
    missing="$missing
    $rel"
    continue
  fi
  if [ "$(wc -l <"$work/kegs")" -gt 1 ]; then
    id=$(uuid "$contents/$rel")
    [ -n "$id" ] ||
      die "$rel is in more than one keg, $(tr '\t\n' '/ ' <"$work/kegs"), and has no UUID to" \
        "say which it is from"
    : >"$work/kegs"
    while IFS="$tab" read -r formula version path <&4; do
      [ "$(uuid "$path")" != "$id" ] || printf '%s\t%s\n' "$formula" "$version" >>"$work/kegs"
    done 4<"$work/these"
    sort -u -o "$work/kegs" "$work/kegs"
    [ "$(wc -l <"$work/kegs")" -eq 1 ] ||
      die "$rel is in more than one keg, and its UUID, $id, is in $(wc -l <"$work/kegs" |
        tr -d ' ') of them"
  fi
  IFS="$tab" read -r formula version <"$work/kegs"
  printf '%s\t%s\t%s\n' "$rel" "$formula" "$version" >>"$work/files"
done 3<"$work/keys"
[ -z "$missing" ] ||
  die "no keg in $cellar holds these, so the project each is from is not known:$missing"

# Each formula the bundle has files from, and its version, which is one version a formula.
awk -F '\t' '$2 != "MpvQt" { print $2 "\t" $3 }' "$work/files" | sort -u >"$work/formulae"
twice=$(cut -f1 "$work/formulae" | uniq -d | tr '\n' ' ')
[ -z "$twice" ] || die "the bundle has files from more than one version of each of: $twice"

# What each formula's keg records of itself, a line each, its fields split on $unit:
# <formula> <version> <source> <licence> <commit> <path> <copy>. The source and licence are those
# its sbom.spdx.json gives the source archive of the version installed; a source taken from git,
# which the SBOM gives without the commit, has beside it the tag or commit that the keg's copy of
# the formula names. The commit is the homebrew-core commit INSTALL_RECEIPT.json says the formula
# was installed from, where it records one, and the path is the formula's file in homebrew-core.
# The copy is the keg's own .brew/<formula>.rb, which says how the bottle was built, patches and
# all, put in formulae/. What a keg does not record is left empty.
mkdir "$out/formulae"
: >"$work/kegs-say"
while IFS="$tab" read -r formula version <&3; do
  keg=$cellar/$formula/$version
  stable=$(printf '%s\n' "$version" | sed 's/_[0-9][0-9]*$//')
  source=
  licence=
  head=
  path=
  copy=
  if [ -f "$keg/sbom.spdx.json" ]; then
    said=$(jq -r --arg name "$formula" --arg version "$stable" '
      [.packages[] | select(.name == $name and .versionInfo == $version
        and (.SPDXID | startswith("SPDXRef-Archive-")))][0] // {}
      | [.downloadLocation, .licenseConcluded]
      | map(if . == null or . == "NOASSERTION" then "" else . end) | join("\u001f")
    ' "$keg/sbom.spdx.json") || die "jq cannot read $keg/sbom.spdx.json"
    IFS="$unit" read -r source licence <<EOF || true
$said
EOF
  fi
  if [ -f "$keg/INSTALL_RECEIPT.json" ]; then
    said=$(jq -r '.source // {} | select(.tap == "homebrew/core")
      | [.tap_git_head // "", .path // ""] | join("\u001f")
    ' "$keg/INSTALL_RECEIPT.json") || die "jq cannot read $keg/INSTALL_RECEIPT.json"
    IFS="$unit" read -r head path <<EOF || true
$said
EOF
    case $path in
      */homebrew-core/*) path=${path##*/homebrew-core/} ;;
      *) path= ;;
    esac
  fi
  case $source in
    *.git | git+* | git://*) git=1 ;;
    *) git= ;;
  esac
  if [ -f "$keg/.brew/$formula.rb" ]; then
    cp "$keg/.brew/$formula.rb" "$out/formulae/$formula.rb"
    copy=formulae/$formula.rb
    if [ -n "$git" ]; then
      # The tag and commit written with the url, on its line or the lines that go on from it.
      ref=$(awk -v url="$source" '
        !on && index($0, "url \"" url "\"") { on = 1 }
        on {
          s = $0
          while (match(s, /(tag|revision):[[:space:]]*"[^"]*"/)) {
            m = substr(s, RSTART, RLENGTH)
            s = substr(s, RSTART + RLENGTH)
            v = m
            sub(/^[^"]*"/, "", v)
            sub(/"$/, "", v)
            ref = ref (ref == "" ? "" : ", ") (m ~ /^tag/ ? "tag " : "commit ") v
          }
          if ($0 !~ /,[[:space:]]*$/) exit
        }
        END { print ref }
      ' "$keg/.brew/$formula.rb")
      [ -z "$ref" ] || git=
      [ -z "$ref" ] || source="$source ($ref)"
    fi
  fi
  [ -z "$git" ] || source=
  printf '%s\n' "$formula$unit$version$unit$source$unit$licence$unit$head$unit$path$unit$copy" \
    >>"$work/kegs-say"
done 3<"$work/formulae"

# brew info stands in for what a keg does not record: a source, a licence, a formula when there is
# no copy of it, or the formula's path in homebrew-core beside a commit. It speaks of the formula as
# Homebrew has it now, so it stands in for a source, a licence or a formula only when its version
# is the one installed; HOMEBREW_NO_AUTO_UPDATE keeps it to the formulae Homebrew last fetched. Its
# fields: <name> <version> <licence> <source> <commit> <path>, the version being the keg's name,
# the version and then the revision, if any. A licence is an SPDX expression, which older brews
# give as a tree of any_of and all_of.
awk -F "$unit" '$3 == "" || $4 == "" || ($5 == "" && $7 == "") || ($5 != "" && $6 == "") {
  print $1
}' "$work/kegs-say" >"$work/ask"
: >"$work/brew-says"
if [ -s "$work/ask" ]; then
  set --
  while IFS= read -r formula; do set -- "$@" "$formula"; done <"$work/ask"
  HOMEBREW_NO_AUTO_UPDATE=1 "$brew" info --json=v2 --formula "$@" >"$work/info.json" ||
    die "brew info --json=v2 --formula $* failed"
  jq -r '
    def expression:
      if type == "string" then .
      elif type == "object" and has("any_of") then
        "(" + (.any_of | map(expression) | join(" OR ")) + ")"
      elif type == "object" and has("all_of") then
        "(" + (.all_of | map(expression) | join(" AND ")) + ")"
      elif type == "object" then
        to_entries | map(.key + (if (.value | type) == "object" and .value.with
          then " WITH " + .value.with else "" end)) | join(" AND ")
      else "" end;
    .formulae[] | [
      .name,
      .versions.stable + (if .revision > 0 then "_" + (.revision | tostring) else "" end),
      (.license | expression),
      (.urls.stable | .url + ([if .tag then "tag " + .tag else empty end,
        if .revision then "commit " + .revision else empty end]
        | if length > 0 then " (" + join(", ") + ")" else "" end)),
      .tap_git_head // "",
      .ruby_source_path // ""
    ] | join("\u001f")
  ' "$work/info.json" >"$work/brew-says"
fi

# Each formula's licence, source and formula, `<formula> <version> <licence> <source> <formula>`:
# the formula is its copy in the bundle, with its file at the homebrew-core commit beside it when
# the keg names the commit, or, with no copy, its file at brew info's commit. A formula that the keg
# and brew info leave without one of them fails the script, which names all such formulae and what
# each lacks.
awk -F "$unit" -v OFS="$unit" -v problems="$work/problems" '
  function link(commit, path) {
    return "https://github.com/Homebrew/homebrew-core/blob/" commit "/" path
  }
  FILENAME == ARGV[1] {
    now[$1] = $2; licence[$1] = $3; source[$1] = $4; commit[$1] = $5; path[$1] = $6
    next
  }
  {
    f = $1
    same = (f in now) && now[f] == $2
    lacks = ""
    s = $3
    if (s == "" && same) s = source[f]
    if (s == "") lacks = lacks ", a source"
    l = $4
    if (l == "" && same) l = licence[f]
    if (l == "") lacks = lacks ", a licence"
    p = $6 != "" ? $6 : path[f]
    formula = $7
    if ($5 != "" && p != "") formula = (formula == "" ? "" : formula " (") link($5, p) \
      (formula == "" ? "" : ")")
    if (formula == "" && same && commit[f] != "" && path[f] != "")
      formula = link(commit[f], path[f])
    if (formula == "") lacks = lacks ", its formula"
    if (lacks != "") {
      print "    " f " " $2 ": " substr(lacks, 3) (f in now ? "; brew info speaks of " now[f] \
        : "; brew info does not know it") >problems
      next
    }
    print f, $2, l, s, formula
  }
' "$work/brew-says" "$work/kegs-say" >"$work/said"
[ ! -s "$work/problems" ] ||
  die "neither the keg nor brew info, of the version installed, gives these:
$(cat "$work/problems")"

# The licence files, from each keg or from the repository.
bare=
while IFS="$tab" read -r formula version <&3; do
  mkdir "$out/$formula"
  licence_files "$cellar/$formula/$version" >"$work/texts"
  if [ -s "$work/texts" ]; then
    while IFS= read -r t <&4; do cp "$t" "$out/$formula/"; done 4<"$work/texts"
  elif [ -n "$(ls -A "$ours/$formula" 2>/dev/null)" ]; then
    cp "$ours/$formula"/* "$out/$formula/"
  else
    bare="$bare $formula"
  fi
done 3<"$work/formulae"
[ -z "$bare" ] || die "neither the keg nor gui/qt/macos/licenses/ has the licence files of:$bare;" \
  "add them there, from each one's source at the version bundled"
if awk -F '\t' '$2 == "MpvQt" { found = 1 } END { exit !found }' "$work/files"; then
  [ -n "$(ls -A "$ours/MpvQt" 2>/dev/null)" ] || die "gui/qt/macos/licenses/MpvQt/ is empty"
  mkdir "$out/MpvQt"
  cp "$ours/MpvQt"/* "$out/MpvQt/"
fi
mkdir "$out/subcheck"
cp "$repo/LICENSE" "$out/subcheck/LICENSE"

{
  printf 'file\tproject\tversion\tlicence\tsource\tformula\n'
  awk -F '\t' -v OFS='\t' -v unit="$unit" -v licence="$mpvqt_licence" -v source="$mpvqt_source" '
    FILENAME == ARGV[1] { split($0, f, unit); said[f[1]] = f[3] "\t" f[4] "\t" f[5]; next }
    $2 == "MpvQt" { print $1, $2, $3, licence, source, ""; next }
    { print $1, $2, $3, said[$2] }
  ' "$work/said" "$work/files" | LC_ALL=C sort
} >"$out/manifest.tsv"

# The Rust crates the two executables link, as cargo resolves them for the Mac: normal
# dependencies, not those that only build or test them. A crate cargo tree has shown already ends
# with (*). Then each one's directory, where its licence files are, from cargo metadata, which is
# asked of the Mac's crates alone, as it fetches every crate it lists.
(cd "$repo" && cargo tree --locked -e normal --target aarch64-apple-darwin -p subcheck-ffi \
  -p subcheck --prefix none --format '{p}|{l}|{r}') >"$work/tree" || die "cargo tree failed"
(cd "$repo" && cargo metadata --locked --format-version 1 --filter-platform aarch64-apple-darwin) \
  >"$work/metadata.json" ||
  die "cargo metadata failed"
jq -r '
  .workspace_members as $members | .packages[] |
    [.name, .version, (.manifest_path | rtrimstr("/Cargo.toml")),
      (if (.id as $id | $members | index([$id])) then "member" else "" end)] | @tsv
' "$work/metadata.json" >"$work/packages"
sed 's/ (\*)$//' "$work/tree" | awk -F '|' -v OFS='\t' '
  {
    split($1, word, " ")
    version = word[2]
    sub(/^v/, "", version)
    print word[1], version, $2, $3
  }
' | LC_ALL=C sort -u >"$work/crates"
# cargo tree puts an empty line between the trees of the two packages.
awk -F '\t' -v OFS="$unit" -v unknown="$work/unknown" '
  FILENAME == ARGV[1] { dir[$1 "\t" $2] = $3; member[$1 "\t" $2] = $4; next }
  $1 == "" { next }
  !(($1 "\t" $2) in dir) { print $1 " " $2 >unknown; next }
  member[$1 "\t" $2] == "" { print $1, $2, $3, $4, dir[$1 "\t" $2] }
' "$work/packages" "$work/crates" >"$work/rust"
[ ! -s "$work/unknown" ] ||
  die "cargo metadata does not say where these crates that cargo tree names are: $(tr '\n' ' ' \
    <"$work/unknown")"
[ -s "$work/rust" ] || die "cargo tree names no crate but the workspace's own"
rule='=============================================================================='
{
  echo "The Rust crates that Contents/MacOS/subcheck and Contents/MacOS/subcheck-qt are built from,"
  echo "but subcheck's own, each with its version, licence and repository, and the text of each"
  echo "licence file it ships."
  while IFS="$unit" read -r name version licence repository dir <&3; do
    printf '\n%s\n%s %s\nLicence: %s\n' "$rule" "$name" "$version" "$licence"
    [ -z "$repository" ] || printf 'Repository: %s\n' "$repository"
    licence_files "$dir" >"$work/texts"
    [ -s "$work/texts" ] || printf '\nThe crate ships no licence file.\n'
    while IFS= read -r t <&4; do
      printf '\n---- %s ----\n\n' "${t##*/}"
      cat "$t"
    done 4<"$work/texts"
  done 3<"$work/rust"
} >"$out/rust-crates.txt"

echo "licenses.sh: the kegs say each formula's source, licence and formula, but for these, which" \
  "brew info gave:$(awk '{ printf " %s", $1 }' "$work/ask")$([ -s "$work/ask" ] || echo ' none');" \
  "$(awk -F "$unit" '$5 != ""' "$work/kegs-say" | wc -l | tr -d ' ') receipts name the" \
  "homebrew-core commit"
echo "licenses.sh: Contents/Resources/licenses/ has the licences of $(wc -l <"$work/files" |
  tr -d ' ') libraries from $(cut -f2 "$work/files" | sort -u | wc -l | tr -d ' ') projects, and" \
  "of $(wc -l <"$work/rust" | tr -d ' ') Rust crates"
