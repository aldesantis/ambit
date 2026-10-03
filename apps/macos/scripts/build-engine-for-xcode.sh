#!/usr/bin/env bash
# Called from the Xcode build (scheme pre-action and the Ambit target's first phase). Brings the
# engine up to date for the configuration being built: Release builds it universal, every other
# configuration host-only with the dev profile. A no-op when nothing changed.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Cargo must not see the target's toolchain settings, which Xcode exports to build scripts.
unset SDKROOT ARCHS CC CXX LD LDFLAGS CFLAGS CPPFLAGS IPHONEOS_DEPLOYMENT_TARGET

if [[ "${CONFIGURATION:-Debug}" == Release ]]; then
  exec "$script_dir/build-engine.sh"
else
  exec "$script_dir/build-engine.sh" --debug
fi
