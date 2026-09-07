#!/usr/bin/env bash
# 构建、组装与打包 MacAIConsole 及自包含 aiworkd / runners。
# 用法：
#   scripts/build-app.sh [release|debug]   # 本地开发：编译、组装并直接启动（默认 release）
#   scripts/build-app.sh package           # 打包模式：编译并组装自包含 MacAIConsole.app
#   scripts/build-app.sh dmg               # 打包模式：编译、组装并创建 MacAIConsole.dmg
set -euo pipefail

cd "$(dirname "$0")/.."

ACTION="run"
CONFIG="release"

# 参数解析
for arg in "$@"; do
    case "${arg}" in
        release|debug)
            CONFIG="${arg}"
            ;;
        package)
            ACTION="package"
            ;;
        dmg)
            ACTION="dmg"
            ;;
        run)
            ACTION="run"
            ;;
        *)
            echo "用法：$0 [release|debug] [package|dmg|run]" >&2
            exit 2
            ;;
    esac
done

REPO_ROOT="$(cd ../.. && pwd)"
BUILD_DIR="build"
APP="${BUILD_DIR}/MacAIConsole.app"
DMG="${BUILD_DIR}/MacAIConsole.dmg"
CODESIGN_ID="${CODESIGN_IDENTITY:--}"

stop_process() {
    local name="$1"
    if pgrep -x "${name}" >/dev/null 2>&1; then
        echo "==> 停止旧 ${name}"
        pkill -x "${name}" || true
        for _ in {1..50}; do
            pgrep -x "${name}" >/dev/null 2>&1 || return 0
            sleep 0.1
        done
        echo "${name} 未在 5 秒内退出" >&2
        exit 1
    fi
}

if [ "${ACTION}" = "run" ]; then
    stop_process MacAIConsole
    stop_process aiworkd
fi

if [ -d /Applications/Xcode.app/Contents/Developer ] && [ -z "${DEVELOPER_DIR:-}" ]; then
    export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
fi

echo "==> 编译 Rust 二进制 (ai-daemon, ai-cli) [profile: ${CONFIG}]"
CARGO_FLAGS=()
if [ "${CONFIG}" = "release" ]; then
    CARGO_FLAGS+=(--release)
fi
cargo build "${CARGO_FLAGS[@]}" -p ai-daemon -p ai-cli

echo "==> 编译 SwiftUI 应用 [config: ${CONFIG}]"
swift build -c "${CONFIG}"

BIN_DIR="$(swift build -c "${CONFIG}" --show-bin-path)"
GUI_BIN="${BIN_DIR}/MacAIConsole"
DAEMON_BIN="${REPO_ROOT}/target/${CONFIG}/aiworkd"
CLI_BIN="${REPO_ROOT}/target/${CONFIG}/macai"

echo "==> 组装自包含 ${APP}"
rm -rf "${APP}"
mkdir -p "${APP}/Contents/MacOS" "${APP}/Contents/Resources"

# 获取并准备内置 uv 二进制（固定版本 0.9.21，目标 aarch64-apple-darwin）
UV_VERSION="0.9.21"
UV_CACHE_DIR="${REPO_ROOT}/.build/tools/uv-${UV_VERSION}"
UV_BIN="${UV_CACHE_DIR}/uv"

prepare_uv() {
    if [ -x "${UV_BIN}" ]; then
        return 0
    fi
    echo "==> 准备内置 uv 二进制 (${UV_VERSION}, aarch64-apple-darwin)"
    mkdir -p "${UV_CACHE_DIR}"

    if [ -n "${MACAI_UV_BIN:-}" ] && [ -x "${MACAI_UV_BIN}" ]; then
        echo "==> 使用 MACAI_UV_BIN 指定的 uv: ${MACAI_UV_BIN}"
        cp "${MACAI_UV_BIN}" "${UV_BIN}"
    elif command -v uv >/dev/null 2>&1; then
        local sys_uv
        sys_uv="$(command -v uv)"
        if file "${sys_uv}" | grep -q "arm64"; then
            echo "==> 使用系统已有 arm64 uv: ${sys_uv}"
            cp "${sys_uv}" "${UV_BIN}"
        fi
    fi

    if [ ! -x "${UV_BIN}" ]; then
        echo "==> 从官方 Release 下载 uv ${UV_VERSION}..."
        local download_url="https://github.com/astral-sh/uv/releases/download/${UV_VERSION}/uv-aarch64-apple-darwin.tar.gz"
        local tmp_tar="${UV_CACHE_DIR}/uv.tar.gz"
        curl -fsSL --retry 3 "${download_url}" -o "${tmp_tar}"
        tar -xzf "${tmp_tar}" -C "${UV_CACHE_DIR}" --strip-components=1
        rm -f "${tmp_tar}"
    fi

    chmod +x "${UV_BIN}"
}

# 拷贝二进制
cp "${GUI_BIN}" "${APP}/Contents/MacOS/MacAIConsole"
cp "${DAEMON_BIN}" "${APP}/Contents/MacOS/aiworkd"
cp "${CLI_BIN}" "${APP}/Contents/MacOS/macai"

prepare_uv
cp "${UV_BIN}" "${APP}/Contents/MacOS/uv"
chmod +x "${APP}/Contents/MacOS/"*

# 拷贝图标
if [ -f resources/AppIcon.icns ]; then
    cp resources/AppIcon.icns "${APP}/Contents/Resources/AppIcon.icns"
else
    echo "!! 缺少 resources/AppIcon.icns，应用将使用系统默认图标" >&2
fi

# 拷贝内置 runners（排除本地虚拟环境与开发缓存）
echo "==> 拷贝内置 Runners 到 App Bundle Resources"
mkdir -p "${APP}/Contents/Resources/runners"
rsync -a \
    --exclude '.venv' \
    --exclude '__pycache__' \
    --exclude '*.pyc' \
    --exclude '.pytest_cache' \
    --exclude '.coverage' \
    "${REPO_ROOT}/runners/" "${APP}/Contents/Resources/runners/"

# 生成 Info.plist
cat > "${APP}/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>MacAIConsole</string>
    <key>CFBundleIdentifier</key>
    <string>org.macai.MacAIConsole</string>
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

echo "==> 代码签名 (${CODESIGN_ID})"
codesign --force --deep --sign "${CODESIGN_ID}" "${APP}" >/dev/null

create_dmg() {
    local app_path="$1"
    local dmg_path="$2"
    local vol_name="MacAI"
    local staging_dir="${BUILD_DIR}/dmg_staging"

    echo "==> 正在打包 DMG: ${dmg_path}"
    rm -rf "${dmg_path}" "${staging_dir}"
    mkdir -p "${staging_dir}"

    cp -R "${app_path}" "${staging_dir}/"
    ln -s /Applications "${staging_dir}/Applications"

    if command -v create-dmg >/dev/null 2>&1; then
        echo "==> 使用 create-dmg 制作带布局的 DMG"
        create-dmg \
            --volname "${vol_name}" \
            --window-pos 200 120 \
            --window-size 600 400 \
            --icon-size 120 \
            --icon "MacAIConsole.app" 160 190 \
            --app-drop-link 440 190 \
            --hide-extension "MacAIConsole.app" \
            "${dmg_path}" \
            "${staging_dir}" || {
                echo "!! create-dmg 执行未完成，回退至原生 hdiutil 打包"
                hdiutil create -volname "${vol_name}" -srcfolder "${staging_dir}" -ov -format UDZO "${dmg_path}"
            }
    else
        echo "==> 使用原生 hdiutil 制作 DMG"
        hdiutil create -volname "${vol_name}" -srcfolder "${staging_dir}" -ov -format UDZO "${dmg_path}"
    fi

    rm -rf "${staging_dir}"
    echo "==> DMG 生成成功: ${dmg_path} ($(du -h "${dmg_path}" | cut -f1))"
}

if [ "${ACTION}" = "dmg" ]; then
    create_dmg "${APP}" "${DMG}"
elif [ "${ACTION}" = "run" ]; then
    echo "==> 启动新 ${APP}"
    open -n "${APP}"
fi

echo "==> 完成！"
