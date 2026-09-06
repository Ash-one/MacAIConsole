#!/usr/bin/env bash
# 停止旧的 MacAIConsole / aiworkd，重建两端并启动新 app。
# 用法：scripts/build-app.sh [release]（默认 release）
set -euo pipefail

cd "$(dirname "$0")/.."

CONFIG="${1:-release}"
if [ "${CONFIG}" != release ]; then
    echo "用法：$0 [release]" >&2
    exit 2
fi

REPO_ROOT="$(cd ../.. && pwd)"
APP="build/MacAIConsole.app"

# Launch Services 会复用旧 GUI；先退出它和旧 daemon，才能保证两端都使用新产物。
stop_process() {
    local name="$1"
    if pgrep -x "${name}" >/dev/null 2>&1; then
        echo "==> 停止旧 ${name}"
        pkill -x "${name}"
        for _ in {1..50}; do
            pgrep -x "${name}" >/dev/null 2>&1 || return
            sleep 0.1
        done
        echo "${name} 未在 5 秒内退出" >&2
        exit 1
    fi
}

stop_process MacAIConsole
stop_process aiworkd

if [ -d /Applications/Xcode.app/Contents/Developer ] && [ -z "${DEVELOPER_DIR:-}" ]; then
    export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
fi

echo "==> cargo build --release -p ai-daemon"
cargo build --release -p ai-daemon

echo "==> swift build -c ${CONFIG}"
swift build -c "${CONFIG}"

BIN_DIR="$(swift build -c "${CONFIG}" --show-bin-path)"
BIN="${BIN_DIR}/MacAIConsole"

echo "==> 组装 ${APP}"
rm -rf "${APP}"
mkdir -p "${APP}/Contents/MacOS" "${APP}/Contents/Resources"
cp "${BIN}" "${APP}/Contents/MacOS/MacAIConsole"
if [ -f resources/AppIcon.icns ]; then
    cp resources/AppIcon.icns "${APP}/Contents/Resources/AppIcon.icns"
else
    echo "!! 缺少 resources/AppIcon.icns，应用将使用系统默认图标" >&2
fi

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
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
</dict>
</plist>
PLIST

codesign --force --deep --sign - "${APP}" >/dev/null

echo "==> 启动新 ${APP}（AIWORKD_PATH=${REPO_ROOT}/target/${CONFIG}/aiworkd）"
AIWORKD_PATH="${REPO_ROOT}/target/${CONFIG}/aiworkd" open -n "${APP}"
