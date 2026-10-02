#!/usr/bin/env bash
# Builds the Rust core (crates/dbcore-ffi) for macOS and regenerates the Swift bindings:
#   apps/macos/Frameworks/DBCoreFFI.xcframework   (static lib + C header + modulemap)
#   apps/macos/Sources/DBCoreFFI/dbcore_ffi.swift (generated Swift API)
# Usage: scripts/build-core.sh [release|debug]   (UNIVERSAL=1 to also build x86_64)
set -euo pipefail
cd "$(dirname "$0")/.."

PROFILE="${1:-release}"
OUT_DIR=$([[ "$PROFILE" == "release" ]] && echo release || echo debug)
CARGO_PROFILE=$([[ "$PROFILE" == "release" ]] && echo release || echo dev)
MAC=apps/macos
LIB=libdbcore_ffi
export MACOSX_DEPLOYMENT_TARGET=15.0

TARGETS=(aarch64-apple-darwin)
[[ "${UNIVERSAL:-0}" == 1 ]] && TARGETS+=(x86_64-apple-darwin)

for t in "${TARGETS[@]}"; do
  cargo build -p dbcore-ffi --profile "$CARGO_PROFILE" --target "$t"
done

if [[ ${#TARGETS[@]} -gt 1 ]]; then
  STATIC=target/universal/$OUT_DIR/$LIB.a
  mkdir -p "$(dirname "$STATIC")"
  lipo -create "${TARGETS[@]/#/target/}" -output "$STATIC" 2>/dev/null || \
    lipo -create $(for t in "${TARGETS[@]}"; do echo "target/$t/$OUT_DIR/$LIB.a"; done) -output "$STATIC"
else
  STATIC=target/${TARGETS[0]}/$OUT_DIR/$LIB.a
fi
DYLIB=target/${TARGETS[0]}/$OUT_DIR/$LIB.dylib

GEN=target/uniffi-swift
rm -rf "$GEN" && mkdir -p "$GEN/include"
cargo run -q -p uniffi-bindgen -- generate --library "$DYLIB" --language swift --out-dir "$GEN"
cp "$GEN/dbcore_ffiFFI.h" "$GEN/include/"
cp "$GEN/dbcore_ffiFFI.modulemap" "$GEN/include/module.modulemap"

rm -rf "$MAC/Frameworks/DBCoreFFI.xcframework"
mkdir -p "$MAC/Frameworks" "$MAC/Sources/DBCoreFFI"
xcodebuild -create-xcframework -library "$STATIC" -headers "$GEN/include" \
  -output "$MAC/Frameworks/DBCoreFFI.xcframework" >/dev/null
cp "$GEN/dbcore_ffi.swift" "$MAC/Sources/DBCoreFFI/"
echo "Rust core → $MAC/Frameworks/DBCoreFFI.xcframework"
