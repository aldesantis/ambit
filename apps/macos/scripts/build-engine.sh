#!/usr/bin/env bash
# Builds the ambit engine (crates/ambit-ffi) for the macOS app and assembles the local Swift
# package apps/macos/AmbitEngine from it:
#
#   AmbitEngine/AmbitEngineFFI.xcframework     static library + C header + module map
#   AmbitEngine/Sources/AmbitEngine/*.swift    the generated Swift bindings
#
# Usage: build-engine.sh [--debug] [--force]
#
#   --debug   Build the host architecture only, with the dev profile. For local iteration; the
#             release build is universal (arm64 + x86_64) with the release-ffi profile.
#   --force   Rebuild even when no input changed since the last run.
#
# Every output is generated and gitignored. A run whose inputs (the Rust sources, manifests,
# lockfile, this script and the mode) match the previous run's exits without touching the outputs,
# so Xcode does not relink the app.

set -euo pipefail

mode=release
force=0

for arg in "$@"; do
  case "$arg" in
    --debug) mode=debug ;;
    --force) force=1 ;;
    -h | --help)
      sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "build-engine.sh: unknown argument: $arg" >&2
      exit 2
      ;;
  esac
done

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$script_dir/../../.." && pwd)"
package="$repo/apps/macos/AmbitEngine"
work="$repo/target/ambit-engine/$mode"
stamp="$package/.build-stamp"

# Xcode build phases run with a minimal PATH.
export PATH="$HOME/.cargo/bin:$PATH"
export MACOSX_DEPLOYMENT_TARGET=26.0

inputs_digest() {
  {
    echo "mode=$mode"
    (
      cd "$repo"
      find Cargo.toml Cargo.lock rust-toolchain.toml crates/ambit-core crates/ambit-ffi \
        -type f \( -name '*.rs' -o -name '*.toml' -o -name 'Cargo.lock' \) \
        -not -path '*/tests/*' -print0 | sort -z | xargs -0 shasum -a 256
    )
    shasum -a 256 "$script_dir/build-engine.sh"
  } | shasum -a 256 | cut -d' ' -f1
}

digest="$(inputs_digest)"

if [[ $force -eq 0 && -f "$stamp" && "$(cat "$stamp")" == "$digest" &&
  -d "$package/AmbitEngineFFI.xcframework" && -f "$package/Sources/AmbitEngine/AmbitEngine.swift" ]]; then
  echo "build-engine.sh: engine is up to date ($mode)"
  exit 0
fi

if [[ $mode == debug ]]; then
  profile=dev
  profile_dir=debug
  case "$(uname -m)" in
    arm64) targets=(aarch64-apple-darwin) ;;
    x86_64) targets=(x86_64-apple-darwin) ;;
    *)
      echo "build-engine.sh: unsupported host architecture $(uname -m)" >&2
      exit 1
      ;;
  esac
else
  profile=release-ffi
  profile_dir=release-ffi
  targets=(aarch64-apple-darwin x86_64-apple-darwin)
fi

cd "$repo"

libraries=()

for target in "${targets[@]}"; do
  echo "build-engine.sh: building ambit-ffi for $target ($profile)"
  cargo build --locked -p ambit-ffi --lib --profile "$profile" --target "$target"
  libraries+=("$repo/target/$target/$profile_dir/libambit_ffi.a")
done

rm -rf "$work"
mkdir -p "$work/headers" "$work/bindings"

lipo -create "${libraries[@]}" -output "$work/libambit_ffi.a"

# The bindings come from the metadata embedded in the library. A thin archive is read: every
# architecture embeds the same metadata.
echo "build-engine.sh: generating Swift bindings"
cargo run --locked -q -p ambit-ffi --features bindgen --bin uniffi-bindgen -- \
  generate "${libraries[0]}" --language swift --out-dir "$work/bindings"

cp "$work/bindings/AmbitEngineFFI.h" "$work/headers/"
cp "$work/bindings/AmbitEngineFFI.modulemap" "$work/headers/module.modulemap"

echo "build-engine.sh: assembling AmbitEngineFFI.xcframework"
rm -rf "$package/AmbitEngineFFI.xcframework"
xcodebuild -create-xcframework \
  -library "$work/libambit_ffi.a" \
  -headers "$work/headers" \
  -output "$package/AmbitEngineFFI.xcframework" >/dev/null

mkdir -p "$package/Sources/AmbitEngine"
cp "$work/bindings/AmbitEngine.swift" "$package/Sources/AmbitEngine/AmbitEngine.swift"

echo "$digest" >"$stamp"
echo "build-engine.sh: done ($mode, ${targets[*]})"
