#!/usr/bin/env bash
# Build Zwisper.app from a release build.
#   ./scripts/bundle-macos.sh                 unsigned (local testing; ad-hoc signed)
#   SIGN_ID="Developer ID Application: …" ./scripts/bundle-macos.sh   signed + hardened runtime
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release -p zwisper
APP=target/release/bundle/Zwisper.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Frameworks" "$APP/Contents/Resources"
cp target/release/zwisper "$APP/Contents/MacOS/zwisper"
cp packaging/macos/Info.plist "$APP/Contents/Info.plist"

# Bundle any dylibs the engines link dynamically (e.g. onnxruntime / sherpa-onnx).
for lib in $(otool -L "$APP/Contents/MacOS/zwisper" | awk '/@rpath|target\/release/ {print $1}'); do
  name=$(basename "$lib")
  src=$(find target/release -name "$name" -print -quit || true)
  if [ -n "$src" ]; then
    cp "$src" "$APP/Contents/Frameworks/"
    install_name_tool -change "$lib" "@executable_path/../Frameworks/$name" "$APP/Contents/MacOS/zwisper"
  fi
done

if [ -n "${SIGN_ID:-}" ]; then
  codesign --force --options runtime --timestamp --entitlements packaging/macos/entitlements.plist \
    --sign "$SIGN_ID" "$APP/Contents/Frameworks/"* "$APP" 2>/dev/null || \
  codesign --force --options runtime --timestamp --entitlements packaging/macos/entitlements.plist --sign "$SIGN_ID" "$APP"
else
  # Ad-hoc signature so macOS remembers permissions between launches of this build.
  codesign --force --deep --sign - "$APP"
fi
echo "Built $APP"
