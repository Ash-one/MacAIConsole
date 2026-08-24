#!/usr/bin/env bash
# 构建 MacAIConsole.app
# 用法：scripts/build-app.sh [debug|release]  （默认 release）
set -euo pipefail

cd "$(dirname "$0")/.."

CONFIG="${1:-release}"

echo "==> swift build -c ${CONFIG}"
swift build -c "${CONFIG}"

BIN_DIR="$(swift build -c "${CONFIG}" --show-bin-path)"
BIN="${BIN_DIR}/MacAIConsole"
APP="build/MacAIConsole.app"

echo "==> 组装 ${APP}"
rm -rf "${APP}"
mkdir -p "${APP}/Contents/MacOS"
cp "${BIN}" "${APP}/Contents/MacOS/MacAIConsole"

cat > "${APP}/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>MacAIConsole</string>
    <key>CFBundleIdentifier</key>
    <string>com.guanxuzeng.MacAIConsole</string>
    <key>CFBundleName</key>
    <string>MacAIConsole</string>
    <key>CFBundleDisplayName</key>
    <string>MacAIConsole</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSMinimumSystemVersion</key>
    <string>14.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
</dict>
</plist>
PLIST

codesign --force --deep --sign - "${APP}" >/dev/null

echo "==> 完成：${APP}"