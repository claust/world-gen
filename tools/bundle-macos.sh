#!/bin/bash
# Bundle world-gen as a macOS .app with icon.
# Usage: ./tools/bundle-macos.sh [--release]

set -euo pipefail
cd "$(dirname "$0")/.."

PROFILE="debug"
if [[ "${1:-}" == "--release" ]]; then
    PROFILE="release"
    cargo build --release
else
    cargo build
fi

APP_NAME="World Gen"
BUNDLE_DIR="target/${PROFILE}/${APP_NAME}.app"
CONTENTS="${BUNDLE_DIR}/Contents"
MACOS="${CONTENTS}/MacOS"
RESOURCES="${CONTENTS}/Resources"

rm -rf "${BUNDLE_DIR}"
mkdir -p "${MACOS}" "${RESOURCES}"

# Copy binary
cp "target/${PROFILE}/world-gen" "${MACOS}/world-gen"

# Copy icon
cp "assets/icon/world-gen.icns" "${RESOURCES}/world-gen.icns"

# Copy asset files the binary needs at runtime
cp -R assets "${MACOS}/assets"

# Create Info.plist
cat > "${CONTENTS}/Info.plist" << 'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>World Gen</string>
    <key>CFBundleDisplayName</key>
    <string>World Gen</string>
    <key>CFBundleIdentifier</key>
    <string>com.worldgen.app</string>
    <key>CFBundleVersion</key>
    <string>0.1.0</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.0</string>
    <key>CFBundleExecutable</key>
    <string>world-gen</string>
    <key>CFBundleIconFile</key>
    <string>world-gen</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST

echo "Bundled: ${BUNDLE_DIR}"
echo "Run with: open \"${BUNDLE_DIR}\""
