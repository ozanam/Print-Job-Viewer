#!/bin/bash
# Build "Print File Viewer.app" (universal: Apple Silicon + Intel) into dist/.
# Needs rustup with aarch64-apple-darwin and x86_64-apple-darwin targets.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
export MACOSX_DEPLOYMENT_TARGET=11.0
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)

for t in aarch64-apple-darwin x86_64-apple-darwin; do
  rustup run stable cargo build --release --target "$t"
done

APP="dist/Print File Viewer.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create -output "$APP/Contents/MacOS/PrintFileViewer" \
  target/aarch64-apple-darwin/release/printfileviewer \
  target/x86_64-apple-darwin/release/printfileviewer
cp assets/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Print File Viewer</string>
  <key>CFBundleDisplayName</key><string>Print File Viewer</string>
  <key>CFBundleIdentifier</key><string>local.printfileviewer</string>
  <key>CFBundleExecutable</key><string>PrintFileViewer</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>Print job file</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>CFBundleTypeExtensions</key>
      <array>
        <string>prn</string><string>pcl</string><string>pxl</string><string>pcl6</string><string>xl</string>
        <string>spl</string><string>shd</string><string>ps</string><string>eps</string><string>epsf</string><string>plt</string>
        <string>hpgl</string><string>hpg</string><string>emf</string><string>xps</string><string>oxps</string>
      </array>
    </dict>
    <dict>
      <key>CFBundleTypeName</key><string>Any file</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <key>LSHandlerRank</key><string>None</string>
      <key>LSItemContentTypes</key><array><string>public.data</string></array>
    </dict>
  </array>
</dict>
</plist>
PLIST

# Ad-hoc signature (required on Apple Silicon; not notarized)
codesign --force --deep --sign - "$APP"

ditto -c -k --keepParent "$APP" "dist/PrintFileViewer-macOS-${VERSION}.zip"
echo "Built $APP"
