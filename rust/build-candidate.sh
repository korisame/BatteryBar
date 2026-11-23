#!/bin/zsh
set -euo pipefail
cd "${0:A:h}"
cargo build --offline --locked --release
candidate="$PWD/dist/BatteryBar-candidate.noindex/BatteryBar.app"
mkdir -p "$candidate/Contents/MacOS" "$candidate/Contents/Resources"
cp target/release/batterybar-rust "$candidate/Contents/MacOS/BatteryBar"
cp ../AppIcon.icns "$candidate/Contents/Resources/AppIcon.icns"
cat > "$candidate/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>BatteryBar</string>
<key>CFBundleDisplayName</key><string>BatteryBar</string>
<key>CFBundleIdentifier</key><string>com.shaungioelegori.batterybar.rust-candidate</string>
<key>CFBundleExecutable</key><string>BatteryBar</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>3.0.0</string>
<key>CFBundleVersion</key><string>10</string>
<key>CFBundleIconFile</key><string>AppIcon</string>
<key>LSMinimumSystemVersion</key><string>26.0</string>
<key>LSUIElement</key><true/>
<key>NSHighResolutionCapable</key><true/>
<key>NSPrincipalClass</key><string>NSApplication</string>
<key>NSUserNotificationsUsageDescription</key><string>BatteryBar ti avvisa sulla carica della batteria.</string>
</dict></plist>
PLIST
codesign --force --sign - "$candidate"
codesign --verify --deep --strict "$candidate"
print -r -- "$candidate"
