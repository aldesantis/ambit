#!/bin/sh
# AMBIT_INSTALL_DIR and AMBIT_NO_MODIFY_PATH are read by the dist installer itself.
set -eu

REPO="aldesantis/ambit"
VERSION="${AMBIT_VERSION:-latest}"

if [ "$VERSION" = "latest" ]; then
  url="https://github.com/$REPO/releases/latest/download/ambit-installer.sh"
else
  url="https://github.com/$REPO/releases/download/$VERSION/ambit-installer.sh"
fi

command -v curl >/dev/null 2>&1 || {
  echo "ambit: curl is required." >&2
  exit 1
}

# Downloaded in full before running, so a dropped connection cannot execute half a script.
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl --proto '=https' --tlsv1.2 -fsSL "$url" -o "$tmp/ambit-installer.sh" || {
  echo "ambit: could not download $url" >&2
  exit 1
}

sh "$tmp/ambit-installer.sh"
