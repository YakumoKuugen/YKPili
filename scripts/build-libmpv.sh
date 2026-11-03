#!/usr/bin/env bash
#
# 按 packaging/libmpv/features.toml 编译「只含启用功能」的 libmpv。
#
# 为什么需要重新编译：libmpv 是单体库，预编译的 libmpv-2.dll（shinchiro 全量构建）
# 把 B 站播放用不到的编码器、滤镜、DVD/蓝光、脚本引擎全编进去了，121 MB 里
# 大部分代码永远不会执行。只有按清单 --disable-everything 重编，体积才会真正下来。
#
# 用法：
#   scripts/build-libmpv.sh                       # 当前平台，输出到 packaging/<平台>/vendor
#   scripts/build-libmpv.sh --platform macos
#   scripts/build-libmpv.sh --print-only          # 只打印将执行的命令
#   scripts/build-libmpv.sh --clean               # 删掉源码/构建目录重来
#   scripts/build-libmpv.sh --force               # 忽略已有产物，强制重编
#   scripts/build-libmpv.sh --jobs 8
#
# package-windows.ps1 / package-macos.sh 打包时会自动调用本脚本；
# 产物指纹仍然匹配时直接复用，不会每次重编。
#
# 依赖（Windows）：MSYS2 UCRT64 + mingw-w64-ucrt-x86_64-{gcc,ninja,meson,pkgconf,
#   libass,libplacebo,harfbuzz,freetype,fribidi,dav1d}。
#   没装 MSYS2 时本脚本会自动调用 scripts/install-msys2.ps1：下载固定版本
#   （SHA256 校验）到 %LOCALAPPDATA%\ykpili\msys64，再用 pacman 装依赖。
#   也可以自己在 UCRT64 终端里跑本脚本，或手动装到 C:\msys64。
# 依赖（macOS）：brew install meson ninja pkgconf nasm libass libplacebo harfbuzz
#   freetype fribidi dav1d
#
# 产物：libmpv-2.dll（Windows）/ libmpv.2.dylib（macOS）+ *.build.json 指纹文件；
# Windows 还会把 MSYS2 侧的运行时 DLL 一并拷到 vendor，包脚本按 deps.txt 收集。
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

PLATFORM="auto"
JOBS=""
PRINT_ONLY=0
CLEAN=0
FORCE=0
SRC="$ROOT/packaging/libmpv/build"
OUT=""
NO_DAV1D=0

# ffmpeg 官方发布包的校验和（固定版本，防止下载到半截或被替换）。
FFMPEG_SHA256="b6863adde98898f42602017462871b5f6333e65aec803fdd7a6308639c52edf3"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --platform) PLATFORM="$2"; shift 2 ;;
        --jobs) JOBS="$2"; shift 2 ;;
        --print-only) PRINT_ONLY=1; shift ;;
        --clean) CLEAN=1; shift ;;
        --force) FORCE=1; shift ;;
        --src) SRC="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        --no-dav1d) NO_DAV1D=1; shift ;;
        -h|--help) sed -n '2,/^set -euo pipefail$/p' "${BASH_SOURCE[0]}" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "未知参数：$1" >&2; exit 2 ;;
    esac
done

say()  { printf '\n==> %s\n' "$*"; }
warn() { printf '  ! %s\n' "$*" >&2; }

# ---------- 平台 / 工具 ----------
if [[ "$PLATFORM" == "auto" ]]; then
    case "$(uname -s)" in
        Darwin) PLATFORM="macos" ;;
        MINGW*|MSYS*|CYGWIN*) PLATFORM="windows" ;;
        *) echo "不支持的平台：$(uname -s)（只做 Windows / macOS）" >&2; exit 2 ;;
    esac
fi
case "$PLATFORM" in
    windows|macos) ;;
    *) echo "未知平台：$PLATFORM（windows / macos）" >&2; exit 2 ;;
esac

# Windows 上如果不在 MSYS2 UCRT64 里（比如从 Git Bash / PowerShell 直接跑），
# 先确保工具链可用，再在 UCRT64 里重跑自己。--print-only 不触发安装。
if [[ "$PLATFORM" == "windows" && "$PRINT_ONLY" != "1" && "${MSYSTEM:-}" != "UCRT64" ]]; then
    INSTALLER="$ROOT/scripts/install-msys2.ps1"
    if [[ -f "$INSTALLER" ]]; then
        echo "==> 当前不在 MSYS2 UCRT64，先准备工具链（已安装则跳过）"
        INSTALLER_WIN="$(cygpath -w "$INSTALLER" 2>/dev/null || printf '%s' "$INSTALLER")"
        MSYS_ROOT="$(powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$INSTALLER_WIN" \
            | sed -n 's/^MSYS2_ROOT=//p' | tail -1)"
        if [[ -z "$MSYS_ROOT" || ! -f "$MSYS_ROOT/usr/bin/bash.exe" ]]; then
            echo "MSYS2 准备失败：拿不到可用的 MSYS2 根目录" >&2
            exit 2
        fi
        ARGS_Q=""
        for _a in "$@"; do ARGS_Q="$ARGS_Q $(printf '%q' "$_a")"; done
        exec "$MSYS_ROOT/usr/bin/bash.exe" -lc "cd '$PWD' && MSYSTEM=UCRT64 exec ./scripts/build-libmpv.sh$ARGS_Q"
    fi
fi

PY=""
for cand in python3 python; do
    if command -v "$cand" >/dev/null 2>&1 && "$cand" -c 'import sys;raise SystemExit(0 if sys.version_info>=(3,11) else 1)' 2>/dev/null; then
        PY="$cand"; break
    fi
done
[[ -n "$PY" ]] || { echo "需要 Python 3.11+（读 TOML 清单）" >&2; exit 2; }

if [[ -z "$OUT" ]]; then
    if [[ "$PLATFORM" == "windows" ]]; then OUT="$ROOT/packaging/windows/vendor"; else OUT="$ROOT/packaging/macos/vendor"; fi
fi

# MSYS2 里的 python/meson/ninja 是原生 Windows 程序，不认 `/d/...` 这种 POSIX 路径；
# 统一换成 `D:/...`（bash/configure 同样接受），否则 meson 会把产物装到 D:\d\...。
if [[ "$PLATFORM" == "windows" ]]; then
    SRC="$(cygpath -m "$SRC")"
    OUT="$(cygpath -m "$OUT")"
fi

if [[ "$PRINT_ONLY" != "1" ]]; then mkdir -p "$SRC" "$OUT"; fi

if [[ -z "$JOBS" ]]; then
    if command -v nproc >/dev/null 2>&1; then JOBS="$(nproc)"
    elif command -v sysctl >/dev/null 2>&1; then JOBS="$(sysctl -n hw.ncpu)"
    else JOBS=4; fi
fi

FEATURES="$ROOT/scripts/libmpv_features.py"
if [[ "$PLATFORM" == "windows" ]]; then FEATURES="$(cygpath -m "$FEATURES")"; fi
MPV_VER="$("$PY" "$FEATURES" plan --platform "$PLATFORM" | sed -n 's/.*mpv \([^ ]*\) \/ FFmpeg.*/\1/p')"
FF_VER="$("$PY" "$FEATURES" plan --platform "$PLATFORM" | sed -n 's/.*\/ FFmpeg \([^ ]*\).*/\1/p')"
[[ -n "$MPV_VER" && -n "$FF_VER" ]] || { echo "无法从清单读出 mpv/ffmpeg 版本" >&2; exit 2; }

# bash 3.2（macOS 自带）没有 mapfile，用 while read 收集参数。
FF_ARGS=()
while IFS= read -r _line; do FF_ARGS+=("$_line"); done < <("$PY" "$FEATURES" ffmpeg --platform "$PLATFORM")
MPV_ARGS=()
while IFS= read -r _line; do MPV_ARGS+=("$_line"); done < <("$PY" "$FEATURES" mpv --platform "$PLATFORM")

# 产物已存在且指纹/体积/导出都过校验：直接复用，打包脚本每次跑也不会重编。
ARTIFACT=""
if [[ "$PLATFORM" == "windows" ]]; then ARTIFACT="$OUT/libmpv-2.dll"; else ARTIFACT="$OUT/libmpv.2.dylib"; fi
if [[ "$PRINT_ONLY" != "1" && "$FORCE" != "1" && -f "$ARTIFACT" ]]; then
    if "$PY" "$FEATURES" verify --platform "$PLATFORM" --file "$ARTIFACT" >/dev/null 2>&1; then
        say "已有最新裁剪产物，跳过编译：$ARTIFACT"
        echo "  要强制重编：./scripts/build-libmpv.sh --platform $PLATFORM --force"
        exit 0
    fi
fi

# ---------- 依赖检查 ----------
need() {
    command -v "$1" >/dev/null 2>&1 || { echo "缺少命令：$1（$2）" >&2; exit 2; }
}

# 已经在 MSYS2 UCRT64 里、但依赖不全：直接用 pacman 补（不用退出重来）。
MSYS_PKGS="make git curl mingw-w64-ucrt-x86_64-gcc mingw-w64-ucrt-x86_64-ninja mingw-w64-ucrt-x86_64-meson mingw-w64-ucrt-x86_64-pkgconf mingw-w64-ucrt-x86_64-nasm mingw-w64-ucrt-x86_64-libass mingw-w64-ucrt-x86_64-libplacebo mingw-w64-ucrt-x86_64-harfbuzz mingw-w64-ucrt-x86_64-freetype mingw-w64-ucrt-x86_64-fribidi mingw-w64-ucrt-x86_64-dav1d"
if [[ "$PLATFORM" == "windows" && "$PRINT_ONLY" != "1" && "${MSYSTEM:-}" == "UCRT64" ]]; then
    if ! pkg-config --exists libass libplacebo >/dev/null 2>&1 ||
       ! command -v meson >/dev/null 2>&1 || ! command -v gcc >/dev/null 2>&1; then
        say "MSYS2 依赖不全，用 pacman 安装"
        pacman -S --needed --noconfirm --disable-download-timeout $MSYS_PKGS || {
            echo "pacman 安装失败。请先执行 scripts/install-msys2.ps1，或在 UCRT64 终端手动装依赖。" >&2
            exit 2
        }
    fi
fi

if [[ "$PRINT_ONLY" != "1" ]]; then
    need tar "解压源码"
    need make "编译 FFmpeg"
    need meson "配置 mpv"
    need ninja "编译 mpv"
    need pkg-config "查找依赖"
    need cc "C 编译器"
fi

# --print-only 只打印命令，允许在没有工具链的机器上预览。
if [[ "$PRINT_ONLY" != "1" ]]; then
    if ! command -v nasm >/dev/null 2>&1; then
        warn "没找到 nasm：FFmpeg 的 x86 汇编优化会被关掉，解码性能会明显下降"
    fi

    if [[ "$PLATFORM" == "windows" ]]; then
        [[ "${MSYSTEM:-}" == UCRT64 || "${MSYSTEM:-}" == MINGW64 ]] || {
            echo "请在 MSYS2 的 UCRT64（或 MINGW64）终端里运行；当前 MSYSTEM=${MSYSTEM:-未设置}" >&2
            echo "安装依赖：pacman -S --needed mingw-w64-ucrt-x86_64-{gcc,ninja,meson,pkgconf,libass,libplacebo,harfbuzz,freetype,fribidi,dav1d}" >&2
            exit 2
        }
    else
        command -v brew >/dev/null 2>&1 || { echo "macOS 构建需要 Homebrew（装 meson/ninja/libass/libplacebo 等）" >&2; exit 2; }
    fi

    # mpv 需要的两个硬依赖：缺了 meson 会在后面才报，提前说清楚。
    pkg-config --exists libass      || { echo "缺少 libass（dev 包）" >&2; exit 2; }
    pkg-config --exists libplacebo  || { echo "缺少 libplacebo（dev 包）" >&2; exit 2; }
fi

PREFIX="$SRC/prefix-$PLATFORM"
FF_SRC="$SRC/ffmpeg-$FF_VER"
MPV_SRC="$SRC/mpv-$MPV_VER"
FF_BUILD="$PREFIX/ffmpeg"
MPV_BUILD="$SRC/build-mpv-$PLATFORM"

if [[ "$CLEAN" == "1" ]]; then
    say "清理旧的源码与构建目录"
    rm -rf "$FF_SRC" "$MPV_SRC" "$PREFIX" "$MPV_BUILD"
fi

run() {
    if [[ "$PRINT_ONLY" == "1" ]]; then
        printf '  $'; printf ' %q' "$@"; printf '\n'
    else
        "$@"
    fi
}

# GNU tar 会把 `D:/…` 当成远程主机（host:file 语法），解压时要用 POSIX 路径。
posix_path() { cygpath -u "$1" 2>/dev/null || printf '%s' "$1"; }

# ---------- 下载源码 ----------
fetch() { # fetch <url> <out> [sha256]
    local url="$1" out="$2" sha="${3:-}"
    if [[ -f "$out" ]]; then echo "  已缓存：$(basename "$out")"; return; fi
    run curl -fL --retry 3 --connect-timeout 20 -o "$out.part" "$url"
    [[ "$PRINT_ONLY" == "1" ]] && return
    if [[ -n "$sha" ]]; then
        local got; got="$(sha256sum "$out.part" | awk '{print $1}')"
        [[ "$got" == "$sha" ]] || { rm -f "$out.part"; echo "校验和不对：$got != $sha" >&2; exit 3; }
    fi
    mv "$out.part" "$out"
}

say "准备源码（FFmpeg $FF_VER / mpv $MPV_VER）"
fetch "https://ffmpeg.org/releases/ffmpeg-$FF_VER.tar.xz" "$SRC/ffmpeg-$FF_VER.tar.xz" "$FFMPEG_SHA256"
if [[ ! -d "$FF_SRC" ]]; then
    # tar 解压出来的目录名就是 ffmpeg-<版本>，正好是 $FF_SRC。
    run tar -xJf "$(posix_path "$SRC/ffmpeg-$FF_VER.tar.xz")" -C "$(posix_path "$SRC")"
fi
if [[ ! -d "$MPV_SRC" ]]; then
    run git clone --depth 1 --branch "$MPV_VER" https://github.com/mpv-player/mpv.git "$MPV_SRC"
fi

# ---------- 校验 mpv 选项名 ----------
# 清单按 v0.41.0 写；换版本时这一步会立刻暴露改名的选项，而不是编到一半才报。
if [[ "$PRINT_ONLY" != "1" ]]; then
    OPT_FILE=""
    for cand in "$MPV_SRC/meson.options" "$MPV_SRC/meson_options.txt"; do
        [[ -f "$cand" ]] && OPT_FILE="$cand" && break
    done
    if [[ -n "$OPT_FILE" ]]; then
        BAD_DISABLED=(); BAD_ENABLED=()
        for arg in "${MPV_ARGS[@]}"; do
            name="${arg#-D}"; name="${name%%=*}"
            value="${arg##*=}"
            if ! grep -q "option('$name'" "$OPT_FILE"; then
                if [[ "$value" == "disabled" || "$value" == "false" ]]; then BAD_DISABLED+=("$name"); else BAD_ENABLED+=("$name"); fi
            fi
        done
        if [[ ${#BAD_DISABLED[@]} -gt 0 ]]; then
            warn "mpv 不认识这些「关闭」选项，已忽略：${BAD_DISABLED[*]}（版本漂移，建议更新清单）"
        fi
        if [[ ${#BAD_ENABLED[@]} -gt 0 ]]; then
            echo "mpv 不认识这些「启用」选项：${BAD_ENABLED[*]}" >&2
            echo "清单与 mpv $MPV_VER 不匹配，改 packaging/libmpv/features.toml 或换版本。" >&2
            exit 4
        fi
    fi
fi

# ---------- FFmpeg ----------
DAV1D_FLAG=()
if [[ "$NO_DAV1D" != "1" ]] && command -v pkg-config >/dev/null 2>&1 && pkg-config --exists dav1d; then
    DAV1D_FLAG=(--enable-libdav1d)
elif [[ "$NO_DAV1D" != "1" ]]; then
    warn "没找到 dav1d：AV1 会用 FFmpeg 原生解码器（4K 可能掉帧）"
fi

if [[ ! -f "$FF_BUILD/lib/pkgconfig/libavcodec.pc" ]]; then
    say "编译 FFmpeg（只含清单里的解码器/解复用器/滤镜）"
    [[ "$PRINT_ONLY" == "1" ]] || pushd "$FF_SRC" >/dev/null
    run ./configure "${FF_ARGS[@]}" "${DAV1D_FLAG[@]}" --prefix="$FF_BUILD"
    run make -j"$JOBS"
    run make install
    [[ "$PRINT_ONLY" == "1" ]] || popd >/dev/null
else
    echo "  FFmpeg 已编译：$FF_BUILD"
fi

# ---------- mpv ----------
say "编译 mpv（libmpv，无 cplayer）"
# Windows 上 meson 是原生 Python，按 `;` 切 PKG_CONFIG_PATH；POSIX 平台用 `:`。
if [[ "$PLATFORM" == "windows" ]]; then
    PKG_SEP=";"
else
    PKG_SEP=":"
fi
if [[ -n "${PKG_CONFIG_PATH:-}" ]]; then
    export PKG_CONFIG_PATH="$FF_BUILD/lib/pkgconfig$PKG_SEP$PKG_CONFIG_PATH"
else
    export PKG_CONFIG_PATH="$FF_BUILD/lib/pkgconfig"
fi
if [[ ! -f "$MPV_BUILD/build.ninja" ]]; then
    # macOS 的 FFmpeg 是静态库，告诉 meson 优先静态链接；Windows 是共享 DLL。
    PREFER_STATIC=()
    if [[ "$PLATFORM" == "macos" ]]; then PREFER_STATIC=(-Dprefer_static=true); fi
    run meson setup "$MPV_BUILD" "$MPV_SRC" \
        --buildtype=release --prefix="$PREFIX/mpv" --libdir=lib \
        -Ddefault_library=shared "${PREFER_STATIC[@]}" \
        "${MPV_ARGS[@]}"
fi
run ninja -C "$MPV_BUILD"
run ninja -C "$MPV_BUILD" install

# ---------- 收集产物 ----------
if [[ "$PRINT_ONLY" == "1" ]]; then
    say "打印完成（--print-only，未真正编译）"
    exit 0
fi
say "收集产物到 $OUT"
if [[ "$PLATFORM" == "windows" ]]; then
    # vendor 里旧的 DLL 先清掉：换 FFmpeg 版本时别把两份 avcodec 一起装进包。
    find "$OUT" -maxdepth 1 -name '*.dll' -delete 2>/dev/null || true
    rm -f "$OUT/libmpv-deps.txt"
    ARTIFACT="$OUT/libmpv-2.dll"
    SRC_DLL="$(find "$PREFIX/mpv" "$MPV_BUILD" -name 'libmpv-2.dll' -type f 2>/dev/null | head -1 || true)"
    [[ -n "$SRC_DLL" ]] || { echo "构建成功但找不到 libmpv-2.dll" >&2; exit 5; }
    [[ "$PRINT_ONLY" == "1" ]] || cp -f "$SRC_DLL" "$ARTIFACT"

    # FFmpeg 是共享版：它的 DLL 在 $FF_BUILD/bin，ldd 需要先能找到。
    ls "$FF_BUILD"/bin/avcodec-*.dll >/dev/null 2>&1 || {
        echo "没找到共享版 FFmpeg DLL（$FF_BUILD/bin/avcodec-*.dll）" >&2
        exit 5
    }
    export PATH="$FF_BUILD/bin:$PATH"

    # libass / libplacebo / harfbuzz 等来自 MSYS2，是独立 DLL：
    # 递归把非系统的依赖拷进 vendor，包脚本按 deps.txt 一起装。
    DEPS=()
    if [[ "$PRINT_ONLY" != "1" ]]; then
        declare -A seen=([libmpv-2.dll]=1)
        queue=("$ARTIFACT")

        # FFmpeg 共享 DLL 是硬依赖；MSYS2 的 ldd 不一定会去 $FF_BUILD/bin 找，
        # 直接整目录拷进来，避免成品运行期缺 avcodec。
        for _dll in "$FF_BUILD"/bin/*.dll; do
            [[ -f "$_dll" ]] || continue
            _name="$(basename "$_dll")"
            cp -f "$_dll" "$OUT/$_name"
            seen["$_name"]=1
            DEPS+=("$_name")
            queue+=("$OUT/$_name")
        done
        while [[ ${#queue[@]} -gt 0 ]]; do
            current="${queue[0]}"; queue=("${queue[@]:1}")
            while IFS= read -r line; do
                dep="$(awk '{print $3}' <<<"$line")"
                [[ -f "$dep" ]] || continue
                # 系统 DLL 一律不随包：按路径排除（Windows 目录），
                # 再按文件名兜底（有些 ldd 会给出 System32 之外的写法）。
                dep_lc="$(printf '%s' "$dep" | tr '[:upper:]' '[:lower:]')"
                case "$dep_lc" in
                    /c/windows/*|c:/windows/*) continue ;;
                esac
                case "$(basename "$dep" | tr '[:upper:]' '[:lower:]')" in
                    kernel32.dll|kernelbase.dll|user32.dll|gdi32.dll|advapi32.dll|shell32.dll|ole32.dll|winmm.dll|ws2_32.dll|bcrypt.dll|ncrypt.dll|secur32.dll|dwmapi.dll|pathcch.dll|avrt.dll|imm32.dll|ntdll.dll|version.dll|powrprof.dll|uxtheme.dll|comdlg32.dll|comctl32.dll|shlwapi.dll|crypt32.dll|mfplat.dll|mfuuid.dll|d3d11.dll|dxgi.dll|d3d9.dll|opengl32.dll|setupapi.dll|cfgmgr32.dll|dbghelp.dll|msvcrt.dll|ucrtbase.dll|api-ms-win-*.dll|ext-ms-win-*.dll) continue ;;
                esac
                name="$(basename "$dep")"
                [[ -n "${seen[$name]:-}" ]] && continue
                # 防止把产物自己（ldd 的第一行有时就是它）拷到自己身上。
                [[ "$dep" -ef "$OUT/$name" ]] && continue
                seen["$name"]=1
                DEPS+=("$name")
                cp -f "$dep" "$OUT/$name"
                queue+=("$OUT/$name")
            done < <(ldd "$current" 2>/dev/null || true)
        done
        if [[ ${#DEPS[@]} -gt 0 ]]; then printf '%s\n' "${DEPS[@]}" > "$OUT/libmpv-deps.txt"; else : > "$OUT/libmpv-deps.txt"; fi
    fi
else
    ARTIFACT="$OUT/libmpv.2.dylib"
    SRC_DYLIB="$(find "$PREFIX/mpv/lib" -name 'libmpv*.dylib' -type f 2>/dev/null | head -1 || true)"
    [[ -n "$SRC_DYLIB" ]] || { echo "构建成功但找不到 libmpv dylib" >&2; exit 5; }
    [[ "$PRINT_ONLY" == "1" ]] || cp -f "$SRC_DYLIB" "$ARTIFACT"
    # 去掉调试符号（不动导出符号）；签名在打包阶段做，不会失效。
    [[ "$PRINT_ONLY" == "1" ]] || strip -x "$ARTIFACT" 2>/dev/null || true
    # 自检：静态链接必须真的把 FFmpeg 吃进 dylib，否则说明 pkg-config 走到了动态库。
    if [[ "$PRINT_ONLY" != "1" ]] && otool -L "$ARTIFACT" | grep -F "$PREFIX" >/dev/null; then
        echo "静态链接失败：$ARTIFACT 仍依赖构建目录里的 FFmpeg dylib" >&2
        otool -L "$ARTIFACT" | grep -F "$PREFIX" >&2
        exit 6
    fi
fi

# ---------- 自检 + 指纹 ----------
say "写入裁剪指纹并自检"
EXTRA="built by scripts/build-libmpv.sh on $(uname -s)/$(uname -m)"
if [[ ${#DAV1D_FLAG[@]} -gt 0 ]]; then EXTRA="$EXTRA; dav1d=enabled"; else EXTRA="$EXTRA; dav1d=disabled"; fi
"$PY" "$FEATURES" sidecar --platform "$PLATFORM" --file "$ARTIFACT" \
    --mpv-version "$MPV_VER" --ffmpeg-version "$FF_VER" --extra "$EXTRA"
"$PY" "$FEATURES" verify --platform "$PLATFORM" --file "$ARTIFACT"

SIZE_MB="$(du -m "$ARTIFACT" | awk '{print $1}')"
say "完成：$ARTIFACT（约 ${SIZE_MB} MB）"
if [[ "$PLATFORM" == "windows" && -f "$OUT/libmpv-deps.txt" ]]; then
    echo "  运行时依赖：$(tr '\n' ' ' < "$OUT/libmpv-deps.txt")"
fi
