#!/usr/bin/env bash
# Copy the git built by build-git.sh into Ambit.app/Contents/Resources/git and sign every Mach-O
# file in it.
#
# Usage: apps/macos/scripts/embed-git.sh [--app PATH] [--git-dir DIR] [--identity ID]
#   --app       the .app bundle. Defaults to $TARGET_BUILD_DIR/$WRAPPER_NAME, which Xcode sets
#               when this runs as a build phase.
#   --git-dir   the staged git tree. Defaults to $AMBIT_GIT_OUT, then apps/macos/build/git.
#   --identity  the codesign identity. Defaults to $EXPANDED_CODE_SIGN_IDENTITY (set by Xcode),
#               then "-" (ad-hoc). Any other identity also gets the hardened runtime and a secure
#               timestamp, both of which notarization requires.
#
# Symlinks are copied as symlinks: git's exec path is mostly links to bin/git, and copying them as
# files would multiply the bundle size and the number of executables to sign.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MACOS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

die() { printf 'error: %s\n' "$*" >&2; exit 1; }

app=""
if [[ -n "${TARGET_BUILD_DIR:-}" && -n "${WRAPPER_NAME:-}" ]]; then
  app="$TARGET_BUILD_DIR/$WRAPPER_NAME"
fi
git_dir="${AMBIT_GIT_OUT:-$MACOS_DIR/build/git}"
identity="${EXPANDED_CODE_SIGN_IDENTITY:-}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --app) app="${2:?--app needs a path}"; shift 2 ;;
    --git-dir) git_dir="${2:?--git-dir needs a path}"; shift 2 ;;
    --identity) identity="${2:?--identity needs a value}"; shift 2 ;;
    *) die "unknown argument: $1" ;;
  esac
done
identity="${identity:--}"

[[ -n "$app" ]] || die "no app bundle: pass --app or run as an Xcode build phase"
[[ -d "$app/Contents" ]] || die "$app is not an app bundle"
[[ -x "$git_dir/bin/git" ]] || die "no git at $git_dir/bin/git. Run apps/macos/scripts/build-git.sh first."

dest="$app/Contents/Resources/git"
rm -rf "$dest"
mkdir -p "$dest"
cp -RP "$git_dir/." "$dest/"

sign_args=(--force --sign "$identity")
if [[ "$identity" != "-" ]]; then
  sign_args+=(--options runtime --timestamp)
fi

count=0
while IFS= read -r -d '' file; do
  file -b "$file" | grep -q 'Mach-O' || continue
  codesign "${sign_args[@]}" "$file"
  count=$((count + 1))
done < <(find "$dest" -type f -perm -u+x -print0)

printf 'Embedded git in %s (%d executables signed with %s)\n' "$dest" "$count" "$identity"
