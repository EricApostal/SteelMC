#!/usr/bin/env bash
# Builds the Dart kernel loaded by the embedded Steel Dart runtime.
#
# STEEL_DART_SDK must point at a Dart SDK build tree containing
# bootstrap_gen_kernel.exe and vm_platform.dill (for example
# <dart-sdk-src>/out/ReleaseX64).
#
# STEEL_DART_ENTRY selects the entry point (default bin/essentials.dart) and
# STEEL_DART_OUTPUT the kernel path (default essentials.dill).
set -euo pipefail

SDK="${STEEL_DART_SDK:?set STEEL_DART_SDK to a Dart SDK build directory}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENTRY="${STEEL_DART_ENTRY:-$HERE/bin/essentials.dart}"
OUTPUT="${STEEL_DART_OUTPUT:-$HERE/essentials.dill}"

if [[ ! -x "$SDK/bootstrap_gen_kernel.exe" ]]; then
  echo "missing $SDK/bootstrap_gen_kernel.exe" >&2
  exit 1
fi
if [[ ! -f "$SDK/vm_platform.dill" ]]; then
  echo "missing $SDK/vm_platform.dill" >&2
  exit 1
fi

cd "$HERE"
dart pub get
"$SDK/bootstrap_gen_kernel.exe" \
  --platform="$SDK/vm_platform.dill" \
  --no-aot \
  --no-embed-sources \
  --no-link-platform \
  --packages="$HERE/.dart_tool/package_config.json" \
  --output="$OUTPUT" \
  --link-platform "$ENTRY"
echo "wrote $OUTPUT"
