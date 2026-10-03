#!/usr/bin/env bash
# Fail before a production release does any work if a signing, notarization, update or OAuth input
# is missing. Every missing name is reported, not just the first.
#
# Usage: apps/macos/scripts/check-release-secrets.sh
#
# Reads the inputs from the environment. In GitHub Actions they are repository secrets and
# variables of the same name (see the README's development section). Local and pull request builds
# never run this script and need none of them.
set -euo pipefail

SECRETS=(
  APPLE_DEVELOPER_ID_CERT_P12      # base64 of the Developer ID Application certificate + key (.p12)
  APPLE_DEVELOPER_ID_CERT_PASSWORD # password of that .p12
  APPLE_TEAM_ID                    # 10-character Apple team identifier
  APPLE_NOTARY_KEY_ID              # App Store Connect API key ID used by notarytool
  APPLE_NOTARY_ISSUER_ID           # App Store Connect API issuer ID
  APPLE_NOTARY_KEY_P8              # contents of the AuthKey_<id>.p8 file
  SPARKLE_ED_PRIVATE_KEY           # Sparkle EdDSA private key that signs appcast.xml entries
)
VARIABLES=(
  AMBIT_GITHUB_CLIENT_ID # client ID of the GitHub OAuth app with device flow enabled
  SPARKLE_PUBLIC_ED_KEY  # public half of SPARKLE_ED_PRIVATE_KEY, embedded in Info.plist
)

# GitHub Actions turns `::error::` lines into annotations on the run summary.
prefix="error: "
[[ "${GITHUB_ACTIONS:-}" == "true" ]] && prefix="::error::"

missing=0
for name in "${SECRETS[@]}"; do
  if [[ -z "${!name:-}" ]]; then
    printf '%srepository secret %s is not set\n' "$prefix" "$name" >&2
    missing=1
  fi
done
for name in "${VARIABLES[@]}"; do
  if [[ -z "${!name:-}" ]]; then
    printf '%srepository variable %s is not set\n' "$prefix" "$name" >&2
    missing=1
  fi
done

if [[ "$missing" != 0 ]]; then
  printf 'The macOS app cannot be signed, notarized and published without them.\n' >&2
  printf 'See "Releasing the macOS app" in README.md for how to create each one.\n' >&2
  exit 1
fi

printf 'All macOS release secrets and variables are set.\n'
