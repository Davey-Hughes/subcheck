#!/bin/bash
# Signs a Subcheck.app ad hoc, as finish-bundle.sh's last step: macOS runs no arm64 code that is
# not signed, and the install changed the load commands of the files it put in the bundle, which
# breaks the signatures they came with. An ad hoc signature says nothing of who made the app, so
# macOS opens a downloaded copy only once its user lets it through in Privacy & Security. This is
# the one step that a Developer ID signature and notarization replace, which need a paid Apple
# developer account.
#
#   gui/qt/macos/sign-bundle.sh <path/to/Subcheck.app>
#
# bash 3.2, as macOS ships it, and the system's own tools.
set -euo pipefail

[ "$#" -eq 1 ] && [ -d "$1/Contents" ] || {
  echo "usage: $0 <path/to/Subcheck.app>" >&2
  exit 2
}
codesign --force --deep --sign - "$1"
