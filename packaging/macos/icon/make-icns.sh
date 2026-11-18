#!/usr/bin/env bash
# Rebuild packaging/macos/AppIcon.icns (and the 1024 px preview) from render.swift.
# Every size is rendered natively, so small sizes get the simplified mark.
set -euo pipefail
cd "$(dirname "$0")"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
swiftc -O -o "$work/render" render.swift

set_dir="$work/AppIcon.iconset"
mkdir -p "$set_dir"
# name:pixels — the sizes an .icns needs (each point size at 1x and 2x).
for spec in 16x16:16 16x16@2x:32 32x32:32 32x32@2x:64 128x128:128 128x128@2x:256 \
            256x256:256 256x256@2x:512 512x512:512 512x512@2x:1024; do
  "$work/render" "$set_dir/icon_${spec%%:*}.png" "${spec##*:}" >/dev/null
done

iconutil -c icns "$set_dir" -o ../AppIcon.icns
"$work/render" AppIcon-1024.png 1024 >/dev/null
echo "wrote packaging/macos/AppIcon.icns and icon/AppIcon-1024.png"
