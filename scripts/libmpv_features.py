#!/usr/bin/env python3
"""把 packaging/libmpv/features.toml 翻译成构建参数，并校验随包产物。

为什么要这个脚本：libmpv 是**单体库**，包脚本没法从 121 MB 的预编译 DLL 里
「删掉」用不到的编解码器；唯一能真正做到「只编译启用的功能」的办法是按清单
重新编译 FFmpeg + mpv。构建参数不能手写在两个平台的脚本里（迟早漂移），
所以集中在这里生成，包脚本只负责校验产物是不是按这份清单编出来的。

命令：
    python3 scripts/libmpv_features.py plan      --platform windows
    python3 scripts/libmpv_features.py ffmpeg    --platform windows   # 每行一个参数
    python3 scripts/libmpv_features.py mpv       --platform windows   # 每行一个 -D 参数
    python3 scripts/libmpv_features.py fingerprint --platform windows
    python3 scripts/libmpv_features.py verify    --file <libmpv> [--sidecar <json>]
    python3 scripts/libmpv_features.py sidecar   --file <libmpv> --mpv-version ... 

verify 的退出码（包脚本靠它决定告警还是失败）：
    0  产物与清单一致，体积在预算内
    2  没有 sidecar：无法证明是裁剪构建（通常是还没跑 build-libmpv.sh）
    3  指纹不符：清单改过但产物没重建
    4  体积超预算
    5  产物损坏 / 缺关键导出
"""

from __future__ import annotations

import argparse
import hashlib
import json
import struct
import subprocess
import sys
from pathlib import Path
from typing import Any

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11
    sys.exit("需要 Python 3.11+（tomllib）。当前：%s" % sys.version.split()[0])

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "packaging" / "libmpv" / "features.toml"

# 构建参数生成逻辑的版本：改了 ffmpeg_args/mpv_args 的取舍就 +1，
# 这样旧产物会因为指纹变化被包脚本判定为「需要重建」。
GENERATOR_VERSION = 4

# 产物里必须存在的符号：少一个就说明链接被裁过头了。
REQUIRED_EXPORTS = (
    "mpv_create",
    "mpv_initialize",
    "mpv_set_option_string",
    "mpv_command",
    "mpv_wait_event",
    "mpv_terminate_destroy",
    "mpv_render_context_create",
    "mpv_render_context_render",
    "mpv_render_context_report_swap",
)

# mpv 侧永远关闭的功能：应用不用，或者要在别的平台才编。
# 注意 v0.41.0 没有 subrandr，别把 master 的新选项写进来（build 脚本会拦）。
MPV_ALWAYS_OFF = {
    "cplayer": "false",          # 只要库，不要 mpv.exe / 命令行 / yt-dlp
    "lua": "disabled",
    "javascript": "disabled",
    "cplugins": "disabled",
    "dvbin": "disabled",
    "dvdnav": "disabled",
    "libbluray": "disabled",
    "libarchive": "disabled",
    "libavdevice": "disabled",
    "cdda": "disabled",
    "vapoursynth": "disabled",
    "rubberband": "disabled",
    "zimg": "disabled",
    "sdl2-video": "disabled",
    "sdl2-audio": "disabled",
    "sdl2-gamepad": "disabled",
    "caca": "disabled",
    "sixel": "disabled",
    "lcms2": "disabled",
    "jpeg": "disabled",          # 不截图
    "uchardet": "disabled",
    "x11": "disabled",
    "wayland": "disabled",
    "drm": "disabled",
    "gbm": "disabled",
    "egl": "disabled",
    "gl": "disabled",            # 只用 render API sw
    "plain-gl": "disabled",
    "vulkan": "disabled",
    "shaderc": "disabled",
    "spirv-cross": "disabled",
    "vaapi": "disabled",
    "vdpau": "disabled",
    "cuda-hwaccel": "disabled",
    "cuda-interop": "disabled",
    "android-media-ndk": "disabled",
    "oss-audio": "disabled",
    "pipewire": "disabled",
    "pulse": "disabled",
    "sndio": "disabled",
    "jack": "disabled",
    "openal": "disabled",
    "audiounit": "disabled",
    "audiotrack": "disabled",
    "aaudio": "disabled",
    "opensles": "disabled",
}

# 每个平台额外开/关的 mpv 选项。
MPV_PLATFORM = {
    "windows": {
        "wasapi": "enabled",
        "d3d-hwaccel": "enabled",   # hwdec=auto-copy-safe 需要
        "d3d9-hwaccel": "disabled", # 只用 DXVA2 的 copy-back，不碰 D3D9 互操作
        "d3d11": "disabled",        # vo_gpu 的 D3D11 上下文，应用不用
        "gl-win32": "disabled",
        "gl-dxinterop": "disabled",
        "egl-angle": "disabled",
        "egl-angle-lib": "disabled",
        "egl-angle-win32": "disabled",
        "uwp": "disabled",
        "win32-smtc": "disabled",   # 媒体控制（SMTC），应用没接
        "coreaudio": "disabled",
        "avfoundation": "disabled",
    },
    "macos": {
        "avfoundation": "enabled",
        "coreaudio": "enabled",
        # 不用 wid、不建 NSWindow：Cocoa/Swift 全部关掉，省掉 Swift 工具链依赖。
        "cocoa": "disabled",
        "swift-build": "disabled",
        "macos-cocoa-cb": "disabled",
        "macos-media-player": "disabled",
        "macos-touchbar": "disabled",
        # VT 的 GL / libplacebo 互操作都不需要：帧要回读到 CPU，走 copy-back。
        "videotoolbox-gl": "disabled",
        "videotoolbox-pl": "disabled",
        "ios-gl": "disabled",
        "gl-cocoa": "disabled",
        "alsa": "disabled",
        "wasapi": "disabled",
    },
}

# FFmpeg hwaccel：家族 -> 支持该家族的编码器（只对清单里实际启用的编码器生效）。
HWACCEL_SUPPORT = {
    "d3d11va": {"h264", "hevc", "av1", "vp9", "mpeg2video", "vc1"},
    "dxva2": {"h264", "hevc", "av1", "vp9", "mpeg2video", "vc1"},
    "videotoolbox": {"h264", "hevc", "av1", "vp9", "mpeg4"},
}

# d3d11va 有两套纹理路径：老的 d3d11va 与 d3d11va2，两个都留。
HWACCEL_ALIASES = {"d3d11va": ("d3d11va", "d3d11va2")}

TLS_FLAGS = {
    "schannel": "--enable-schannel",
    "securetransport": "--enable-securetransport",
}


def _stdout_utf8() -> None:
    # Windows 控制台默认 GBK，包脚本按 UTF-8 读，这里统一输出编码。
    for stream in (sys.stdout, sys.stderr):
        try:
            # newline="\n"：Windows 下别把参数行写成 CRLF，bash 的 read 会带上 \r。
            stream.reconfigure(encoding="utf-8", errors="replace", newline="\n")
        except (AttributeError, ValueError):
            pass


def detect_platform() -> str:
    if sys.platform == "win32":
        return "windows"
    if sys.platform == "darwin":
        return "macos"
    if sys.platform.startswith("linux"):
        return "linux"
    raise SystemExit(f"不认识的平台：{sys.platform}")


def load_manifest(path: Path = MANIFEST) -> dict[str, Any]:
    with path.open("rb") as fh:
        doc = tomllib.load(fh)
    if doc.get("schema") != 1:
        raise SystemExit(f"{path} 的 schema 应为 1")
    for key in ("versions", "common", "platform"):
        if key not in doc:
            raise SystemExit(f"{path} 缺少 [{key}]")
    return doc


def resolve(platform: str, manifest: dict[str, Any] | None = None) -> dict[str, Any]:
    """把 common 与 platform 合并成一份有效配置。"""
    manifest = manifest or load_manifest()
    if platform not in manifest["platform"]:
        raise SystemExit(
            f"清单里没有 platform.{platform}（现有：{', '.join(manifest['platform'])}）"
        )
    common = dict(manifest["common"])
    plat = dict(manifest["platform"][platform])
    merged: dict[str, Any] = {"versions": dict(manifest["versions"])}
    merged.update(common)
    merged.update(plat)
    merged["platform"] = platform
    merged.setdefault("gpl", True)

    for key in ("video_decoders", "audio_decoders", "demuxers", "protocols"):
        if not merged.get(key):
            raise SystemExit(f"清单 {platform}.{key} 不能为空（应用至少需要一路流）")
    return merged


def canonical(config: dict[str, Any]) -> str:
    return json.dumps(config, sort_keys=True, ensure_ascii=False, separators=(",", ":"))


def fingerprint(platform: str) -> str:
    payload = {
        "generator": GENERATOR_VERSION,
        "config": resolve(platform),
    }
    return hashlib.sha256(canonical(payload).encode("utf-8")).hexdigest()


def ffmpeg_args(platform: str) -> list[str]:
    cfg = resolve(platform)
    decoders = (
        list(cfg["video_decoders"])
        + list(cfg["audio_decoders"])
        + list(cfg.get("subtitle_decoders", []))
    )
    hwaccels: list[str] = []
    for family in cfg.get("hwdec_families", []):
        if family not in HWACCEL_SUPPORT:
            raise SystemExit(f"未知的 hwdec 家族：{family}")
        for name in HWACCEL_ALIASES.get(family, (family,)):
            for dec in cfg["video_decoders"]:
                if dec in HWACCEL_SUPPORT[family]:
                    hwaccels.append(f"{dec}_{name}")

    args = [
        "--disable-everything",
        "--disable-autodetect",
        "--disable-programs",
        "--disable-doc",
        "--disable-debug",
        "--disable-avdevice",
        "--disable-encoders",
        "--disable-muxers",
        "--disable-devices",
        "--enable-network",
        "--enable-swscale",
        "--enable-swresample",
        "--enable-avfilter",
        "--enable-protocol=" + ",".join(cfg["protocols"]),
        "--enable-demuxer=" + ",".join(cfg["demuxers"]),
        "--enable-decoder=" + ",".join(decoders),
        "--enable-parser=" + ",".join(cfg["parsers"]),
        "--enable-filter=" + ",".join(cfg["video_filters"] + cfg["audio_filters"]),
    ]
    # 链接形态按平台选：
    #   Windows 用共享版 FFmpeg —— MSYS2 的 libass/libplacebo 本来就是 DLL，
    #   共享链接省掉 pkg-config --static 的私有依赖坑，包脚本按 deps 清单收 DLL；
    #   macOS 用静态版 —— 只留一个 libmpv.dylib，依赖仍由 bundle-libmpv.py 处理
    #   （Homebrew 的 libass/libplacebo 仍是动态库）。
    if platform == "windows":
        args += ["--enable-shared", "--disable-static", "--enable-pic"]
    else:
        args += ["--enable-static", "--disable-shared", "--enable-pic"]
    if cfg.get("bitstream_filters"):
        args.append("--enable-bsf=" + ",".join(cfg["bitstream_filters"]))
    if hwaccels:
        args.append("--enable-hwaccel=" + ",".join(hwaccels))
        for family in cfg["hwdec_families"]:
            # 家族开关（--enable-d3d11va / --enable-dxva2 / --enable-videotoolbox）
            args.append(f"--enable-{family}")
    tls = cfg.get("tls")
    if tls:
        if tls not in TLS_FLAGS:
            raise SystemExit(f"未知的 TLS 后端：{tls}")
        args.append(TLS_FLAGS[tls])
    return args


def mpv_args(platform: str) -> list[str]:
    cfg = resolve(platform)
    opts: dict[str, str] = {
        "libmpv": "true",
        "build-date": "false",
        "tests": "false",
        "gpl": "true" if cfg.get("gpl", True) else "false",
        "iconv": "auto",
        "zlib": "auto",
    }
    opts.update(MPV_ALWAYS_OFF)
    for ao in cfg.get("audio_outputs", []):
        if ao not in ("wasapi", "avfoundation", "coreaudio"):
            raise SystemExit(f"清单里出现未支持的 AO：{ao}")
        opts[ao] = "enabled"
    opts.update(MPV_PLATFORM.get(platform, {}))
    return [f"-D{k}={v}" for k, v in opts.items()]


def plan_text(platform: str) -> str:
    cfg = resolve(platform)
    fp = fingerprint(platform)
    hw = ", ".join(cfg.get("hwdec_families", [])) or "无"
    off = "、".join(sorted(MPV_ALWAYS_OFF))
    lines = [
        f"libmpv 裁剪清单（{platform}）  指纹 {fp[:16]}",
        f"  mpv {cfg['versions'].get('mpv')} / FFmpeg {cfg['versions'].get('ffmpeg')}",
        f"  视频解码（{len(cfg['video_decoders'])}）：{' '.join(cfg['video_decoders'])}",
        f"  音频解码（{len(cfg['audio_decoders'])}）：{' '.join(cfg['audio_decoders'])}",
        f"  解复用（{len(cfg['demuxers'])}）：{' '.join(cfg['demuxers'])}",
        f"  协议（{len(cfg['protocols'])}）：{' '.join(cfg['protocols'])}",
        f"  音频输出：{'、'.join(cfg.get('audio_outputs', [])) or '无'}；TLS：{cfg.get('tls', '系统默认')}",
        f"  硬件解码：{hw}（只用 copy-back，软件渲染回读）",
        f"  体积预算：{cfg.get('size_budget_mb')} MB",
        f"  永久关闭：{off}",
    ]
    return "\n".join(lines)


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def pe_exports(path: Path) -> set[str]:
    """不依赖 pefile 的 PE 导出表解析（只看名字）。"""
    data = path.read_bytes()
    if data[:2] != b"MZ":
        raise ValueError("不是 PE 文件")
    e_lfanew = struct.unpack_from("<I", data, 0x3C)[0]
    if data[e_lfanew : e_lfanew + 4] != b"PE\0\0":
        raise ValueError("PE 头损坏")
    coff = e_lfanew + 4
    nsec = struct.unpack_from("<H", data, coff + 2)[0]
    opt_size = struct.unpack_from("<H", data, coff + 16)[0]
    opt = coff + 20
    magic = struct.unpack_from("<H", data, opt)[0]
    dd = opt + (112 if magic == 0x20B else 96)
    exp_rva, exp_size = struct.unpack_from("<II", data, dd)
    if not exp_rva:
        return set()
    sec = opt + opt_size
    sections = []
    for i in range(nsec):
        off = sec + 40 * i
        vsize, vaddr, raw_size, raw_ptr = struct.unpack_from("<IIII", data, off + 8)
        sections.append((vaddr, max(vsize, raw_size), raw_ptr))

    def to_offset(rva: int) -> int:
        for vaddr, size, raw_ptr in sections:
            if vaddr <= rva < vaddr + size:
                return raw_ptr + (rva - vaddr)
        raise ValueError(f"RVA {rva:#x} 不在任何节里")

    exp = to_offset(exp_rva)
    n_names = struct.unpack_from("<I", data, exp + 24)[0]
    names_rva = struct.unpack_from("<I", data, exp + 32)[0]
    names_off = to_offset(names_rva)
    out: set[str] = set()
    for i in range(n_names):
        rva = struct.unpack_from("<I", data, names_off + 4 * i)[0]
        start = to_offset(rva)
        end = data.index(b"\0", start)
        out.add(data[start:end].decode("ascii", "replace"))
    return out


def verify_artifact(path: Path, platform: str, sidecar: Path | None) -> int:
    cfg = resolve(platform)
    if not path.exists():
        print(f"[libmpv] 找不到产物：{path}", file=sys.stderr)
        return 5
    size = path.stat().st_size
    budget = int(cfg.get("size_budget_mb") or 0) * 1024 * 1024

    # 1) 结构自检：至少得是能加载的库，且关键符号没被裁掉。
    try:
        with path.open("rb") as fh:
            head = fh.read(4)
        if head[:2] == b"MZ":
            exports = pe_exports(path)
            missing = [name for name in REQUIRED_EXPORTS if name not in exports]
            if missing:
                print(f"[libmpv] 产物缺少关键导出：{', '.join(missing)}", file=sys.stderr)
                return 5
        elif head in (b"\xcf\xfa\xed\xfe", b"\xce\xfa\xed\xfe", b"\xca\xfe\xba\xbe"):
            # Mach-O：用 nm 查导出（没有 nm 就跳过，指纹/体积仍会校验）。
            try:
                nm = subprocess.run(
                    ["nm", "-gU", str(path)], capture_output=True, text=True, check=True
                ).stdout
            except (OSError, subprocess.CalledProcessError):
                nm = ""
            if nm:
                symbols = {line.split()[-1] for line in nm.splitlines() if line.strip()}
                missing = [f"_{n}" for n in REQUIRED_EXPORTS if f"_{n}" not in symbols]
                if missing:
                    print(f"[libmpv] 产物缺少关键导出：{', '.join(missing)}", file=sys.stderr)
                    return 5
        else:
            print("[libmpv] 产物不是 PE / Mach-O 动态库", file=sys.stderr)
            return 5
    except (ValueError, OSError, struct.error, IndexError) as exc:
        print(f"[libmpv] 产物无法解析：{exc}", file=sys.stderr)
        return 5

    # 2) 指纹：清单改了但没重建，包脚本必须知道。
    sidecar = sidecar or path.with_suffix(path.suffix + ".build.json")
    if not sidecar.exists():
        print(f"[libmpv] 未裁剪：{path.name} {size / 1048576:.1f} MB，缺 {sidecar.name}", file=sys.stderr)
        print("[libmpv] 打包时会自动调用 scripts/build-libmpv.sh 重编。", file=sys.stderr)
        return 2
    meta = json.loads(sidecar.read_text(encoding="utf-8"))
    if meta.get("fingerprint") != fingerprint(platform):
        print(
            "[libmpv] 指纹不符：features.toml 改过，但产物没重建。\n"
            f"  清单：{fingerprint(platform)}\n  产物：{meta.get('fingerprint')}",
            file=sys.stderr,
        )
        return 3
    if meta.get("sha256") and meta["sha256"] != _sha256(path):
        print("[libmpv] 产物和 sidecar 记录的 sha256 不一致（被替换过？）", file=sys.stderr)
        return 3

    # 3) 体积：超预算说明有人塞回了全量构建。
    if budget and size > budget:
        print(
            f"[libmpv] 体积超预算：{size / 1048576:.1f} MB > {budget / 1048576:.0f} MB",
            file=sys.stderr,
        )
        return 4

    print(
        f"[libmpv] 校验通过：{path.name} {size / 1048576:.1f} MB"
        f"（预算 {budget / 1048576:.0f} MB，mpv {meta.get('mpv_version', '?')}）"
    )
    return 0


def write_sidecar(path: Path, meta: dict[str, Any], platform: str) -> Path:
    cfg = resolve(platform)
    payload = {
        "schema": 1,
        "platform": platform,
        "fingerprint": fingerprint(platform),
        "sha256": _sha256(path),
        "size": path.stat().st_size,
        "size_budget_mb": cfg.get("size_budget_mb"),
        **meta,
    }
    target = path.with_suffix(path.suffix + ".build.json")
    target.write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    return target


def main(argv: list[str] | None = None) -> int:
    _stdout_utf8()
    parser = argparse.ArgumentParser(description="libmpv 裁剪清单 / 构建参数 / 产物校验")
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--platform", default="auto", help="windows / macos（默认按当前系统）")
    sub = parser.add_subparsers(dest="cmd", required=True)

    sub.add_parser("plan", parents=[common], help="打印裁剪计划")
    sub.add_parser("ffmpeg", parents=[common], help="输出 FFmpeg configure 参数（每行一个）")
    sub.add_parser("mpv", parents=[common], help="输出 mpv meson 参数（每行一个）")
    sub.add_parser("fingerprint", parents=[common], help="输出清单指纹")

    p_verify = sub.add_parser("verify", parents=[common], help="校验随包产物")
    p_verify.add_argument("--file", required=True)
    p_verify.add_argument("--sidecar", default="")

    p_side = sub.add_parser("sidecar", parents=[common], help="构建产物写 sidecar")
    p_side.add_argument("--file", required=True)
    p_side.add_argument("--mpv-version", default="")
    p_side.add_argument("--ffmpeg-version", default="")
    p_side.add_argument("--extra", default="", help="附加说明，写进 sidecar")

    args = parser.parse_args(argv)
    platform = detect_platform() if args.platform == "auto" else args.platform

    if args.cmd == "plan":
        print(plan_text(platform))
    elif args.cmd == "ffmpeg":
        print("\n".join(ffmpeg_args(platform)))
    elif args.cmd == "mpv":
        print("\n".join(mpv_args(platform)))
    elif args.cmd == "fingerprint":
        print(fingerprint(platform))
    elif args.cmd == "verify":
        sidecar = Path(args.sidecar) if args.sidecar else None
        return verify_artifact(Path(args.file), platform, sidecar)
    elif args.cmd == "sidecar":
        meta = {}
        if args.mpv_version:
            meta["mpv_version"] = args.mpv_version
        if args.ffmpeg_version:
            meta["ffmpeg_version"] = args.ffmpeg_version
        if args.extra:
            meta["extra"] = args.extra
        target = write_sidecar(Path(args.file), meta, platform)
        print(f"[libmpv] sidecar：{target}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
