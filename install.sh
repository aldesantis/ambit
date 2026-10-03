#!/bin/sh
# Installs ambit by running the shell installer that dist attaches to every GitHub Release
# (`ambit-installer.sh`). This file exists so the long-standing
# `curl -fsSL https://raw.githubusercontent.com/aldesantis/ambit/main/install.sh | sh` keeps working
# and so a version can be picked by tag; the installer itself does the platform detection, the
# download and the checksum check.
#
# Environment:
#   AMBIT_VERSION          a tag like `v0.5.0`; default is the latest release
#   AMBIT_INSTALL_DIR      where to put the binary; default is `$HOME/.local/bin`
#   AMBIT_NO_MODIFY_PATH   set to 1 to leave shell profiles alone
#
# The last two are read by the dist installer directly.
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
