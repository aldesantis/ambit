#!/usr/bin/env bash
# Run by `xcodegen generate` (preGenCommand). Builds the engine once, host-only, when the local
# AmbitEngine package has never been assembled, because Xcode resolves that package before any
# build phase could build it. Later builds keep it current through build-engine-for-xcode.sh.

set -euo pipefail

package="$(cd "$(dirname "${BASH_SOURCE[0]}")/../AmbitEngine" && pwd)"

if [[ -d "$package/AmbitEngineFFI.xcframework" && -f "$package/Sources/AmbitEngine/AmbitEngine.swift" ]]; then
  exit 0
fi

echo "bootstrap-engine.sh: the engine was never built; building it (debug, host architecture)"
exec "$(dirname "${BASH_SOURCE[0]}")/build-engine.sh" --debug
