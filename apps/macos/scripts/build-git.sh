#!/usr/bin/env bash
# Build a relocatable, universal (arm64 + x86_64) git for bundling inside
# Ambit.app/Contents/Resources/git/.
#
# Usage: apps/macos/scripts/build-git.sh [OUTPUT_DIR]
#   OUTPUT_DIR defaults to $AMBIT_GIT_OUT, then apps/macos/build/git.
#
# The result links only against libraries and frameworks that ship with
# macOS (libcurl, libz, libiconv, CoreFoundation, CoreServices, Security),
# uses RUNTIME_PREFIX so it runs from any location, and needs neither
# Homebrew, the Xcode Command Line Tools, nor a git on PATH at runtime.
# Downloads and per-arch builds are cached under apps/macos/build/, so
# re-running is cheap and always yields the same output.
set -euo pipefail

GIT_VERSION="2.56.0"
GIT_SHA256="26c56c296b38c0695b26fa95f475f1d01704d2d38e73465ca30b0b2f5dc789d3"
GIT_URL="https://www.kernel.org/pub/software/scm/git/git-${GIT_VERSION}.tar.xz"
MACOS_MIN="26.0"
ARCHS=(arm64 x86_64)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MACOS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
BUILD_DIR="$MACOS_DIR/build"
CACHE_DIR="$BUILD_DIR/cache"
WORK_DIR="$BUILD_DIR/git-work"
OUT_DIR="${1:-${AMBIT_GIT_OUT:-$BUILD_DIR/git}}"

# Keep Homebrew and friends out of the build entirely: no curl-config,
# pkg-config or libraries from /opt/homebrew or /usr/local can leak in.
export PATH="/usr/bin:/bin:/usr/sbin:/sbin"
unset CPATH LIBRARY_PATH C_INCLUDE_PATH PKG_CONFIG_PATH CFLAGS LDFLAGS CPPFLAGS
SDKROOT="$(xcrun --sdk macosx --show-sdk-path)"
export SDKROOT
JOBS="$(sysctl -n hw.ncpu)"

log() { printf '==> %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# The install prefix is a placeholder: with RUNTIME_PREFIX every path is
# resolved relative to the running executable, so it never matters at runtime.
PREFIX="/ambit-git"

make_flags() {
  local arch="$1"
  local flags="-arch $arch -mmacosx-version-min=$MACOS_MIN -isysroot $SDKROOT"
  printf '%s\n' \
    "prefix=$PREFIX" \
    "RUNTIME_PREFIX=YesPlease" \
    "gitexecdir=libexec/git-core" \
    "template_dir=share/git-core/templates" \
    "sysconfdir=etc" \
    "CFLAGS=-O2 $flags" \
    "LDFLAGS=$flags" \
    "uname_M=$arch" \
    "NO_GETTEXT=YesPlease" \
    "NO_PERL=YesPlease" \
    "NO_PYTHON=YesPlease" \
    "NO_TCLTK=YesPlease" \
    "NO_RUST=YesPlease" \
    "NO_EXPAT=YesPlease" \
    "NO_OPENSSL=YesPlease" \
    "NO_HOMEBREW=YesPlease" \
    "NO_FINK=YesPlease" \
    "NO_DARWIN_PORTS=YesPlease" \
    "NO_INSTALL_HARDLINKS=YesPlease" \
    "INSTALL_SYMLINKS=YesPlease" \
    "SKIP_DASHED_BUILT_INS=YesPlease" \
    "CURL_CFLAGS=" \
    "CURL_LDFLAGS=-lcurl" \
    "ICONVDIR=" \
    "V=0"
}

fetch_source() {
  local tarball="$CACHE_DIR/git-${GIT_VERSION}.tar.xz"
  mkdir -p "$CACHE_DIR"
  if [[ -f "$tarball" ]] && [[ "$(shasum -a 256 "$tarball" | cut -d' ' -f1)" == "$GIT_SHA256" ]]; then
    log "Using cached $(basename "$tarball")"
  else
    log "Downloading $GIT_URL"
    curl -fL --retry 3 -o "$tarball.part" "$GIT_URL"
    local got
    got="$(shasum -a 256 "$tarball.part" | cut -d' ' -f1)"
    [[ "$got" == "$GIT_SHA256" ]] || { rm -f "$tarball.part"; die "checksum mismatch: expected $GIT_SHA256, got $got"; }
    mv "$tarball.part" "$tarball"
  fi
  TARBALL="$tarball"
}

build_arch() {
  local arch="$1"
  local src="$WORK_DIR/src-$arch"
  local dest="$WORK_DIR/install-$arch"
  local stamp="$WORK_DIR/stamp-$arch"
  local flags
  flags="$(make_flags "$arch")"
  local fingerprint
  fingerprint="$(printf '%s\n%s\n%s\n' "$GIT_VERSION" "$GIT_SHA256" "$flags" | shasum -a 256 | cut -d' ' -f1)"

  if [[ -f "$stamp" && "$(cat "$stamp")" == "$fingerprint" && -x "$dest$PREFIX/bin/git" ]]; then
    log "Using cached $arch build"
    return
  fi

  log "Building git $GIT_VERSION for $arch"
  rm -rf "$src" "$dest" "$stamp"
  mkdir -p "$src"
  tar -xf "$TARBALL" -C "$src" --strip-components 1

  local args=()
  while IFS= read -r line; do args+=("$line"); done <<<"$flags"

  make -C "$src" -j"$JOBS" "${args[@]}" all
  make -C "$src" "${args[@]}" DESTDIR="$dest" install
  printf '%s' "$fingerprint" >"$stamp"
}

is_macho() { file -b "$1" | grep -q 'Mach-O'; }

assemble_universal() {
  local base="$WORK_DIR/install-${ARCHS[0]}$PREFIX"
  log "Assembling universal tree in $OUT_DIR"
  rm -rf "$OUT_DIR"
  mkdir -p "$OUT_DIR"

  # Only what the app needs at runtime: the git binary, the exec-path helpers
  # (including git-remote-https) and the default templates.
  for dir in bin libexec/git-core share/git-core/templates; do
    [[ -d "$base/$dir" ]] && mkdir -p "$OUT_DIR/$dir" && cp -RP "$base/$dir/." "$OUT_DIR/$dir/"
  done

  # Server-side and auxiliary tools the app never runs: dropping them keeps the
  # bundle (and the set of executables to sign) small.
  rm -f "$OUT_DIR/bin/scalar" "$OUT_DIR/bin/git-shell" "$OUT_DIR/bin/git-cvsserver" \
    "$OUT_DIR/libexec/git-core/scalar" "$OUT_DIR/libexec/git-core/git-shell" \
    "$OUT_DIR/libexec/git-core/git-daemon" "$OUT_DIR/libexec/git-core/git-http-backend" \
    "$OUT_DIR/libexec/git-core/git-imap-send" "$OUT_DIR/libexec/git-core/git-cvsserver"

  # Replace each thin Mach-O file with a fat one built from every arch.
  while IFS= read -r -d '' file; do
    local rel="${file#"$OUT_DIR"/}"
    is_macho "$file" || continue
    local inputs=()
    for arch in "${ARCHS[@]}"; do
      local candidate="$WORK_DIR/install-$arch$PREFIX/$rel"
      [[ -f "$candidate" ]] || die "missing $arch slice for $rel"
      inputs+=("$candidate")
    done
    lipo -create "${inputs[@]}" -output "$file"
    strip -x "$file"
  done < <(find "$OUT_DIR" -type f -print0)

  # libexec/git-core/git is a copy of bin/git; link it to keep the bundle small.
  if [[ -f "$OUT_DIR/libexec/git-core/git" && ! -L "$OUT_DIR/libexec/git-core/git" ]]; then
    rm "$OUT_DIR/libexec/git-core/git"
    ln -s ../../bin/git "$OUT_DIR/libexec/git-core/git"
  fi

  cp "$WORK_DIR/src-${ARCHS[0]}/COPYING" "$OUT_DIR/LICENSE-git.txt"
}

verify() {
  log "Verifying"
  local failed=0
  while IFS= read -r -d '' file; do
    is_macho "$file" || continue
    for arch in "${ARCHS[@]}"; do
      lipo "$file" -verify_arch "$arch" || { echo "  $file lacks $arch" >&2; failed=1; }
    done
    # Every dependency must ship with macOS: /usr/lib or /System only.
    local bad
    bad="$(otool -L "$file" | grep -E $'^\t' | awk '{print $1}' | grep -Ev '^(/usr/lib/|/System/)' || true)"
    if [[ -n "$bad" ]]; then
      echo "  $file links outside the system: $bad" >&2
      failed=1
    fi
  done < <(find "$OUT_DIR" -type f -print0)

  # Every symlink must resolve inside the tree.
  while IFS= read -r -d '' link; do
    [[ -e "$link" ]] || { echo "  dangling symlink: $link" >&2; failed=1; }
  done < <(find "$OUT_DIR" -type l -print0)

  [[ "$failed" == 0 ]] || die "verification failed"
  "$OUT_DIR/bin/git" --version >/dev/null || die "bin/git does not run"
}

fetch_source
for arch in "${ARCHS[@]}"; do build_arch "$arch"; done
assemble_universal
verify

log "git $GIT_VERSION ($(lipo -archs "$OUT_DIR/bin/git")) staged at:"
echo "$OUT_DIR"
du -sh "$OUT_DIR" | awk '{print "    size: " $1}'
