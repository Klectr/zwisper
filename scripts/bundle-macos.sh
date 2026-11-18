#!/usr/bin/env bash
# Build Zwisper.app and Zwisper.dmg from a release build, then open the DMG's
# drag-to-Applications window.
#   ./scripts/bundle-macos.sh                 local build, signed with a stable self-signed
#                                             identity so macOS permissions survive rebuilds
#                                             (see dev-signing-identity.sh; ad-hoc on CI)
#   SIGN_ID="Developer ID Application: …" ./scripts/bundle-macos.sh   signed + hardened runtime
#   NO_OPEN=1 ./scripts/bundle-macos.sh       build only, don't mount the DMG (also skipped when $CI is set)
set -euo pipefail
cd "$(dirname "$0")/.."

# Precompile GPUI's Metal shaders when full Xcode is present; otherwise fall
# back to compiling them at launch (Command Line Tools have no `metal`).
if xcrun -f metal >/dev/null 2>&1; then
  cargo build --release -p zwisper --no-default-features
else
  echo "warning: Xcode's metal compiler not found; shaders will compile at launch" >&2
  cargo build --release -p zwisper
fi
APP=target/release/bundle/Zwisper.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Frameworks" "$APP/Contents/Resources"
cp target/release/zwisper "$APP/Contents/MacOS/zwisper"
cp packaging/macos/Info.plist "$APP/Contents/Info.plist"
# GPL-3.0: the license and the attribution terms travel with the app.
cp LICENSE NOTICE "$APP/Contents/Resources/"
cp packaging/macos/AppIcon.icns "$APP/Contents/Resources/"   # see packaging/macos/icon/

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
elif [ -z "${CI:-}" ] && KC=$(./scripts/dev-signing-identity.sh) \
    && HASH=$(security find-certificate -c "Zwisper Local Dev" -Z "$KC" | awk '/SHA-1/ {print $3}') \
    && [ -n "$HASH" ]; then
  # Stable self-signed identity: permission grants survive rebuilds.
  # codesign only finds the identity when its keychain is on the search list,
  # so add it for this one call and put the user's list back exactly.
  ORIG_KCS=()
  while IFS= read -r k; do
    k="${k#"${k%%[![:space:]]*}"}"; k="${k%\"}"; k="${k#\"}"
    [ -n "$k" ] && ORIG_KCS+=("$k")
  done < <(security list-keychains -d user)
  security list-keychains -d user -s "${ORIG_KCS[@]}" "$KC"
  set +e
  codesign --force --deep --keychain "$KC" --sign "$HASH" "$APP"
  rc=$?
  set -e
  security list-keychains -d user -s "${ORIG_KCS[@]}"
  [ $rc -eq 0 ] || { echo "error: signing with the local identity failed" >&2; exit $rc; }
else
  # CI, or no local identity: ad-hoc. Permissions reset on every rebuild.
  codesign --force --deep --sign - "$APP"
fi
echo "Built $APP"

# Disk image: the app next to an /Applications shortcut, so opening it shows the
# usual "drag to install" window.
DMG=target/release/bundle/Zwisper.dmg
VOL=Zwisper
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
# Eject a previous build's image so the new one doesn't mount as "Zwisper 1".
if [ -d "/Volumes/$VOL" ]; then hdiutil detach "/Volumes/$VOL" -quiet || hdiutil detach "/Volumes/$VOL" -force -quiet || true; fi
rm -f "$DMG"
hdiutil create -volname "$VOL" -srcfolder "$STAGE" -fs HFS+ -format UDZO -ov -quiet "$DMG"
if [ -n "${SIGN_ID:-}" ]; then
  codesign --force --timestamp --sign "$SIGN_ID" "$DMG"
fi
echo "Built $DMG"

if [ -z "${NO_OPEN:-}" ] && [ -z "${CI:-}" ]; then
  open "$DMG"
fi
