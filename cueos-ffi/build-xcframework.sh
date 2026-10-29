#!/bin/sh
# Build cueos_ffi.xcframework (macOS arm64+x86_64, iOS arm64, iOS simulator
# arm64+x86_64) and, with --install <dir>, replace <dir>/cueos_ffi.xcframework
# (CUE OS: cue-os-v10-native/Vendor).
set -eu
cd "$(dirname "$0")/.."
ROOT=$(pwd)
OUT="$ROOT/build"
TARGETS="aarch64-apple-darwin x86_64-apple-darwin aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios"

for t in $TARGETS; do
  cargo build -p cueos-ffi --release --target "$t"
done

lib() { echo "$ROOT/target/$1/release/libcueos_ffi.a"; }
mkdir -p "$OUT/headers"
lipo -create "$(lib aarch64-apple-darwin)" "$(lib x86_64-apple-darwin)" -output "$OUT/libcueos_ffi_mac_universal.a"
lipo -create "$(lib aarch64-apple-ios-sim)" "$(lib x86_64-apple-ios)" -output "$OUT/libcueos_ffi_sim_universal.a"
cp cueos-ffi/cueos_ffi.h "$OUT/headers/cueos_ffi.h"
printf 'module cueos_ffi {\n  header "cueos_ffi.h"\n  export *\n}\n' > "$OUT/headers/module.modulemap"

rm -rf "$OUT/cueos_ffi.xcframework"
xcodebuild -create-xcframework \
  -library "$OUT/libcueos_ffi_mac_universal.a" -headers "$OUT/headers" \
  -library "$(lib aarch64-apple-ios)" -headers "$OUT/headers" \
  -library "$OUT/libcueos_ffi_sim_universal.a" -headers "$OUT/headers" \
  -output "$OUT/cueos_ffi.xcframework"

if [ "${1:-}" = "--install" ]; then
  dest="${2:?--install needs a directory}"
  rm -rf "$dest/cueos_ffi.xcframework"
  cp -R "$OUT/cueos_ffi.xcframework" "$dest/"
  echo "installed → $dest/cueos_ffi.xcframework"
fi
