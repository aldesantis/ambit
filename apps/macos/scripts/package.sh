#!/usr/bin/env bash
# Package a built Ambit.app as Ambit-<version>.dmg and Ambit-<version>.zip, optionally signing it
# with a Developer ID, notarizing it and writing a Sparkle appcast.xml.
#
# Usage: apps/macos/scripts/package.sh --app PATH [options]
#   --app PATH             the built Ambit.app (required)
#   --out DIR              where the outputs go. Defaults to apps/macos/build/dist.
#   --version V            defaults to the workspace version (set-version.sh --version)
#   --identity ID          re-sign every nested executable, Sparkle's helpers and the app with this
#                          codesign identity and the hardened runtime. "-" signs ad-hoc, which
#                          exercises the same steps without a certificate.
#   --entitlements FILE    defaults to apps/macos/Ambit/Resources/Ambit.entitlements when present
#   --notarize             submit to Apple, wait, and staple the app and the DMG. Needs a real
#                          --identity and APPLE_NOTARY_KEY_ID, APPLE_NOTARY_ISSUER_ID and
#                          APPLE_NOTARY_KEY_P8 in the environment.
#   --appcast URL_PREFIX   write appcast.xml whose download URL is URL_PREFIX/Ambit-<version>.zip,
#                          signed with SPARKLE_ED_PRIVATE_KEY from the environment. Sparkle's
#                          generate_appcast comes from $SPARKLE_BIN_DIR, or from the Sparkle
#                          package Xcode resolved under apps/macos/build.
#
# Without --identity the app is packaged exactly as built (Xcode signs it ad-hoc), so local and
# pull request builds need no credentials.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MACOS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

log() { printf '==> %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

app=""
out="$MACOS_DIR/build/dist"
version=""
identity=""
entitlements="$MACOS_DIR/Ambit/Resources/Ambit.entitlements"
notarize=0
appcast_prefix=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --app) app="${2:?--app needs a path}"; shift 2 ;;
    --out) out="${2:?--out needs a path}"; shift 2 ;;
    --version) version="${2:?--version needs a value}"; shift 2 ;;
    --identity) identity="${2:?--identity needs a value}"; shift 2 ;;
    --entitlements) entitlements="${2:?--entitlements needs a path}"; shift 2 ;;
    --notarize) notarize=1; shift ;;
    --appcast) appcast_prefix="${2:?--appcast needs a URL prefix}"; shift 2 ;;
    *) die "unknown argument: $1" ;;
  esac
done

[[ -n "$app" ]] || die "pass --app PATH"
[[ -d "$app/Contents/MacOS" ]] || die "$app is not an app bundle"
app="$(cd "$app" && pwd)"
[[ -n "$version" ]] || version="$("$SCRIPT_DIR/set-version.sh" --version)"
if [[ "$notarize" == 1 ]]; then
  [[ -n "$identity" && "$identity" != "-" ]] || die "--notarize needs a Developer ID --identity"
  for name in APPLE_NOTARY_KEY_ID APPLE_NOTARY_ISSUER_ID APPLE_NOTARY_KEY_P8; do
    [[ -n "${!name:-}" ]] || die "--notarize needs $name"
  done
fi
if [[ -n "$appcast_prefix" ]]; then
  [[ -n "${SPARKLE_ED_PRIVATE_KEY:-}" ]] || die "--appcast needs SPARKLE_ED_PRIVATE_KEY"
fi

name="$(basename "$app" .app)"
dmg="$out/$name-$version.dmg"
zip="$out/$name-$version.zip"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$out"
rm -f "$out/appcast.xml"

codesign_args() {
  local args=(--force --sign "$identity" --options runtime)
  [[ "$identity" == "-" ]] || args+=(--timestamp)
  printf '%s\0' "${args[@]}"
}

sign() {
  local args=()
  while IFS= read -r -d '' arg; do args+=("$arg"); done < <(codesign_args)
  codesign "${args[@]}" "$@"
}

# Signing goes inside out: a bundle's signature seals the signatures of the code it contains, so
# every nested executable is signed before the bundle around it.
sign_app() {
  log "Signing $app with $identity"
  "$SCRIPT_DIR/embed-git.sh" --app "$app" --identity "$identity"

  local frameworks="$app/Contents/Frameworks"
  local sparkle="$frameworks/Sparkle.framework/Versions/B"
  if [[ -d "$sparkle" ]]; then
    # The order and the preserved Downloader entitlements follow Sparkle's documentation for
    # re-signing outside Xcode.
    sign "$sparkle/XPCServices/Installer.xpc"
    sign --preserve-metadata=entitlements "$sparkle/XPCServices/Downloader.xpc"
    sign "$sparkle/Autoupdate"
    sign "$sparkle/Updater.app"
    sign "$frameworks/Sparkle.framework"
  fi

  if [[ -d "$frameworks" ]]; then
    while IFS= read -r -d '' item; do
      [[ "$(basename "$item")" == "Sparkle.framework" ]] && continue
      sign "$item"
    done < <(find "$frameworks" -mindepth 1 -maxdepth 1 \( -name '*.framework' -o -name '*.dylib' \) -print0)
  fi

  if [[ -f "$entitlements" ]]; then
    sign --entitlements "$entitlements" "$app"
  else
    sign --preserve-metadata=entitlements "$app"
  fi
  codesign --verify --deep --strict --verbose=2 "$app"
}

notarize() {
  local file="$1"
  local key="$work/AuthKey.p8"
  printf '%s' "$APPLE_NOTARY_KEY_P8" >"$key"
  log "Notarizing $(basename "$file")"
  xcrun notarytool submit "$file" --key "$key" --key-id "$APPLE_NOTARY_KEY_ID" \
    --issuer "$APPLE_NOTARY_ISSUER_ID" --wait --output-format json >"$work/notary.json" || true
  local status id
  status="$(plutil -extract status raw -o - "$work/notary.json" 2>/dev/null || echo unknown)"
  if [[ "$status" != "Accepted" ]]; then
    id="$(plutil -extract id raw -o - "$work/notary.json" 2>/dev/null || true)"
    cat "$work/notary.json" >&2 || true
    if [[ -n "$id" ]]; then
      xcrun notarytool log "$id" --key "$key" --key-id "$APPLE_NOTARY_KEY_ID" \
        --issuer "$APPLE_NOTARY_ISSUER_ID" >&2 || true
    fi
    die "notarization of $(basename "$file") ended with status: $status"
  fi
}

find_sparkle_bin() {
  if [[ -n "${SPARKLE_BIN_DIR:-}" ]]; then
    printf '%s\n' "$SPARKLE_BIN_DIR"
    return
  fi
  local tool
  tool="$(find "$MACOS_DIR/build" -path '*/artifacts/sparkle/Sparkle/bin/generate_appcast' -print -quit 2>/dev/null || true)"
  [[ -n "$tool" ]] || die "generate_appcast not found: set SPARKLE_BIN_DIR to Sparkle's bin directory"
  dirname "$tool"
}

if [[ -n "$identity" ]]; then
  sign_app
fi

if [[ "$notarize" == 1 ]]; then
  ditto -c -k --keepParent "$app" "$work/notarize.zip"
  notarize "$work/notarize.zip"
  xcrun stapler staple "$app"
fi

# Sparkle's update archive. ditto keeps symlinks, permissions and extended attributes.
log "Creating $(basename "$zip")"
rm -f "$zip"
ditto -c -k --sequesterRsrc --keepParent "$app" "$zip"

log "Creating $(basename "$dmg")"
staging="$work/dmg"
mkdir -p "$staging"
ditto "$app" "$staging/$(basename "$app")"
ln -s /Applications "$staging/Applications"
rm -f "$dmg"
hdiutil create -quiet -volname "$name" -srcfolder "$staging" -fs HFS+ -format UDZO -ov "$dmg"

if [[ -n "$identity" && "$identity" != "-" ]]; then
  codesign --force --sign "$identity" --timestamp "$dmg"
fi
if [[ "$notarize" == 1 ]]; then
  notarize "$dmg"
  xcrun stapler staple "$dmg"
  spctl --assess --type execute --verbose=2 "$app"
fi

if [[ -n "$appcast_prefix" ]]; then
  key="$(/usr/libexec/PlistBuddy -c 'Print :SUPublicEDKey' "$app/Contents/Info.plist" 2>/dev/null || true)"
  [[ -n "$key" ]] || die "$app has no SUPublicEDKey: build it with SPARKLE_PUBLIC_ED_KEY set"

  log "Writing appcast.xml"
  bin="$(find_sparkle_bin)"
  feed="$work/feed"
  mkdir -p "$feed"
  cp "$zip" "$feed/"
  printf '%s' "$SPARKLE_ED_PRIVATE_KEY" | "$bin/generate_appcast" --ed-key-file - \
    --download-url-prefix "${appcast_prefix%/}/" -o "$out/appcast.xml" "$feed"
fi

log "Packaged:"
for file in "$dmg" "$zip" "$out/appcast.xml"; do
  [[ -f "$file" ]] && printf '    %s\n' "$file"
done
exit 0
