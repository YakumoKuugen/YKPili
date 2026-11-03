#!/usr/bin/env bash
#
# 打一个可双击运行的 macOS .app（可选打成 DMG）。
#
# 默认行为：
#   - release 编译 app/crates/app 的 ykpili 二进制（当前架构）；
#   - 生成 dist/YKPili.app；
#   - 按 packaging/libmpv/features.toml 裁剪编译 libmpv（已有最新产物则跳过），
#     把 libmpv 及整棵依赖树打进 Contents/Frameworks，重写 install_name，
#     做到不依赖用户装 mpv；
#   - ad-hoc 签名（本机可运行；分发给别人仍会被 Gatekeeper 拦）。
#
# 常用参数：
#   ./scripts/package-macos.sh                  # release + 裁剪并内置 libmpv + DMG
#   ./scripts/package-macos.sh --debug          # 快速验证（用 debug 构建）
#   ./scripts/package-macos.sh --no-libmpv      # 不内置 libmpv（用系统安装的）
#   ./scripts/package-macos.sh --no-dmg         # 只出 .app
#   ./scripts/package-macos.sh --identity "Apple Development: ..."  # 用真实证书签名
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORKSPACE="$ROOT/app"
cd "$ROOT"

APP_NAME="${APP_NAME:-YKPili}"
EXECUTABLE="ykpili"
BUNDLE_ID="${BUNDLE_ID:-com.ykpili.app}"
MIN_MACOS="11.0"
COPYRIGHT="${COPYRIGHT:-Copyright © 2026 YKPili}"

PROFILE="release"
BUNDLE_LIBMPV="auto"
MAKE_DMG=1
SIGN_IDENTITY="${CODESIGN_IDENTITY:--}"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --debug) PROFILE="debug"; shift ;;
        --release) PROFILE="release"; shift ;;
        --no-libmpv) BUNDLE_LIBMPV="0"; shift ;;
        --libmpv) BUNDLE_LIBMPV="1"; shift ;;
        --no-dmg) MAKE_DMG=0; shift ;;
        --identity)
            [[ $# -ge 2 ]] || { echo "--identity 需要一个参数" >&2; exit 2; }
            SIGN_IDENTITY="$2"; shift 2 ;;
        -h|--help)
            sed -n '2,/^set -euo pipefail$/p' "${BASH_SOURCE[0]}" \
                | sed '$d' | sed 's/^# \{0,1\}//'
            exit 0 ;;
        *) echo "未知参数：$1（用 --help 查看用法）" >&2; exit 2 ;;
    esac
done

if [[ -f "$WORKSPACE/Cargo.toml" ]]; then
    VERSION="$(awk '
        /^\[workspace\.package\]/ { in_pkg = 1; next }
        /^\[/ { in_pkg = 0 }
        in_pkg && /^version[[:space:]]*=/ {
            gsub(/["[:space:]]/, "", $3); print $3; exit
        }' "$WORKSPACE/Cargo.toml")"
fi
VERSION="${VERSION:-0.1.0}"
BUILD_NUMBER="$(git rev-list --count HEAD 2>/dev/null || echo 1)"
ARCH="$(uname -m)"
DIST="$ROOT/dist"
APP="$DIST/$APP_NAME.app"
DMG="$DIST/$APP_NAME-$VERSION-macos-$ARCH.dmg"

echo "==> 编译 ${PROFILE}（${ARCH}）"
if [[ "$PROFILE" == "release" ]]; then
    cargo build --manifest-path "$WORKSPACE/Cargo.toml" --release -p yk-app --bin "$EXECUTABLE"
    BIN="$WORKSPACE/target/release/$EXECUTABLE"
else
    cargo build --manifest-path "$WORKSPACE/Cargo.toml" -p yk-app --bin "$EXECUTABLE"
    BIN="$WORKSPACE/target/debug/$EXECUTABLE"
fi

echo "==> 组装 $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Frameworks"
install -m 755 "$BIN" "$APP/Contents/MacOS/$EXECUTABLE"
cp packaging/macos/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
printf 'APPL????' > "$APP/Contents/PkgInfo"

sed \
    -e "s|__APP_NAME__|$APP_NAME|g" \
    -e "s|__EXECUTABLE__|$EXECUTABLE|g" \
    -e "s|__BUNDLE_ID__|$BUNDLE_ID|g" \
    -e "s|__VERSION__|$VERSION|g" \
    -e "s|__BUILD__|$BUILD_NUMBER|g" \
    -e "s|__MIN_MACOS__|$MIN_MACOS|g" \
    -e "s|__COPYRIGHT__|$COPYRIGHT|g" \
    packaging/macos/Info.plist > "$APP/Contents/Info.plist"

plutil -lint "$APP/Contents/Info.plist" >/dev/null

# ---------- 内置 libmpv ----------
if [[ "$BUNDLE_LIBMPV" != "0" ]]; then
    echo "==> 裁剪 libmpv（只编译清单启用的功能；已有最新产物则跳过）"
    ./scripts/build-libmpv.sh --platform macos
fi

VENDOR_LIBMPV="$ROOT/packaging/macos/vendor/libmpv.2.dylib"
LIBMPV=""
if [[ -f "$VENDOR_LIBMPV" ]]; then
    LIBMPV="$VENDOR_LIBMPV"
fi

if [[ "$BUNDLE_LIBMPV" == "1" && -z "$LIBMPV" ]]; then
    echo "找不到 libmpv：先跑 ./scripts/build-libmpv.sh --platform macos" >&2
    exit 1
fi

if [[ -n "$LIBMPV" && "$BUNDLE_LIBMPV" != "0" ]]; then
    echo "==> 校验 libmpv 裁剪产物：$LIBMPV"
    if ! python3 scripts/libmpv_features.py verify --platform macos --file "$LIBMPV"; then
        echo "libmpv 校验未通过。排错见 packaging/libmpv/README.md。" >&2
        exit 1
    fi
fi

if [[ -n "$LIBMPV" && "$BUNDLE_LIBMPV" != "0" ]]; then
    echo "==> 内置 libmpv：$LIBMPV"
    python3 scripts/bundle-libmpv.py --app "$APP" --libmpv "$LIBMPV" --strip

    # mpv 是 GPL、FFmpeg 本次为 LGPL（未启用 GPL 编码器），分发时把许可证一并带上。
    LICENSE_DIR="$APP/Contents/Resources/Licenses"
    mkdir -p "$LICENSE_DIR"
    for license in \
        "$ROOT/packaging/windows/licenses/GPL-2.0.txt" \
        "$ROOT/packaging/windows/licenses/MPV-Copyright.txt"; do
        [[ -f "$license" ]] && cp "$license" "$LICENSE_DIR/"
    done
    cat > "$LICENSE_DIR/LIBMPV-BUILD.txt" <<EOF
libmpv 为 YKPili 按 packaging/libmpv/features.toml 裁剪编译的版本。
mpv：https://github.com/mpv-player/mpv（GPL-2.0-or-later）
FFmpeg：https://ffmpeg.org（LGPL-2.1-or-later，未启用 GPL 编码器）
其余动态库来自 Homebrew，许可证见同目录与各库发行包。
源码获取：见项目仓库的对应 tag 与上述上游。
EOF
else
    echo "==> 跳过内置 libmpv（运行时需要本机已安装 mpv）"
    rmdir "$APP/Contents/Frameworks"
fi

# ---------- 签名 ----------
if [[ "$SIGN_IDENTITY" == "-" ]]; then
    SIGN_ARGS=(--force --sign -)
    echo "==> ad-hoc 签名"
else
    SIGN_ARGS=(--force --sign "$SIGN_IDENTITY" --options runtime --timestamp)
    echo "==> 使用证书签名：$SIGN_IDENTITY"
fi

if [[ -d "$APP/Contents/Frameworks" ]]; then
    while IFS= read -r -d '' lib; do
        codesign "${SIGN_ARGS[@]}" "$lib"
    done < <(find "$APP/Contents/Frameworks" -type f -name "*.dylib" -print0)
fi
codesign "${SIGN_ARGS[@]}" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

# ---------- DMG ----------
ZIP=""
DMG_CREATED=0
if [[ "$MAKE_DMG" == "1" ]]; then
    echo "==> 制作 DMG"
    STAGE="$(mktemp -d "${TMPDIR:-/tmp}/ykpili-dmg.XXXXXX")"
    trap 'rm -rf "$STAGE"' EXIT
    cp -R "$APP" "$STAGE/"
    ln -s /Applications "$STAGE/Applications"
    rm -f "$DMG"
    if hdiutil create -volname "$APP_NAME $VERSION" -srcfolder "$STAGE" \
        -ov -format UDZO "$DMG" >/dev/null 2>&1; then
        DMG_CREATED=1
    else
        # hdiutil 需要访问原始磁盘设备；在受限沙箱里会失败。
        # 退化成 zip，双击解压后同样是可运行的 .app。
        rm -f "$DMG"
        ZIP="$DIST/$APP_NAME-$VERSION-macos-$ARCH.zip"
        echo "    DMG 创建失败（通常是没有磁盘设备权限），改打 zip：$ZIP"
        rm -f "$ZIP"
        ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"
    fi
    rm -rf "$STAGE"
    trap - EXIT
fi

echo
echo "完成："
echo "  APP: $APP"
if [[ "$DMG_CREATED" == "1" ]]; then
    echo "  DMG: $DMG"
fi
if [[ -n "$ZIP" ]]; then
    echo "  ZIP: $ZIP"
fi
echo "  版本: ${VERSION}（build ${BUILD_NUMBER}，${ARCH}）"
