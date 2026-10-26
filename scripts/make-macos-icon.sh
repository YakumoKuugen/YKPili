#!/usr/bin/env bash
#
# 重新生成 packaging/macos/AppIcon.icns。
#
# 直接 `swift make-macos-icon.swift` 在某些机器上会撞上
# “CommandLineTools SDK 与编译器版本不匹配”，所以这里显式挑 Xcode 的
# toolchain / SDK；没有 Xcode 时退回系统 swiftc。
#
# 用法：./scripts/make-macos-icon.sh [输出.icns]
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${1:-$ROOT/packaging/macos/AppIcon.icns}"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/ykpili-icon.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT

export CLANG_MODULE_CACHE_PATH="$ROOT/app/target/tmp/clang-module-cache"
mkdir -p "$CLANG_MODULE_CACHE_PATH"

XCODE_DEV="/Applications/Xcode.app/Contents/Developer"
if [[ -x "$XCODE_DEV/Toolchains/XcodeDefault.xctoolchain/usr/bin/swiftc" ]]; then
    SWIFTC="$XCODE_DEV/Toolchains/XcodeDefault.xctoolchain/usr/bin/swiftc"
    SDK="$XCODE_DEV/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk"
    SWIFTC_ARGS=(-sdk "$SDK")
else
    SWIFTC="$(command -v swiftc)"
    SWIFTC_ARGS=()
fi

"$SWIFTC" "${SWIFTC_ARGS[@]}" "$ROOT/scripts/make-macos-icon.swift" -o "$TMP/makeicon"
"$TMP/makeicon" "$TMP/icon-1024.png"

ICONSET="$TMP/AppIcon.iconset"
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$TMP/icon-1024.png" \
        --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    doubled=$((size * 2))
    sips -z "$doubled" "$doubled" "$TMP/icon-1024.png" \
        --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
cp "$TMP/icon-1024.png" "$ICONSET/icon_512x512@2x.png"

mkdir -p "$(dirname "$OUT")"
iconutil -c icns "$ICONSET" -o "$OUT"
echo "已生成 $OUT"
