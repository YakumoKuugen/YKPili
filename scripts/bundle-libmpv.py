#!/usr/bin/env python3
"""把 libmpv 及其 Homebrew 依赖闭包塞进 .app/Contents/Frameworks。

为什么需要这个脚本：libmpv 由 Homebrew 安装，dylib 的 install_name 指向
`/opt/homebrew/...`。直接分发 .app 时，用户机器上没有 Homebrew 就播不了视频。
正确做法是把整棵依赖树拷进 bundle，并把每个 install_name 重写成
`@loader_path/<文件名>`，让它们只从 .app 内部加载。

脚本只处理 Homebrew 前缀（/opt/homebrew、/usr/local）下的库；系统库
（/usr/lib、/System、/Library）保持原样。改写后由调用方重新签名。

用法：
    python3 scripts/bundle-libmpv.py --app dist/YKPili.app \
        --libmpv /opt/homebrew/opt/mpv/lib/libmpv.2.dylib
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys

# 允许从这些前缀收集 dylib；其余绝对路径视为系统库。
BUNDLE_PREFIXES = ("/opt/homebrew", "/usr/local")
SYSTEM_PREFIXES = ("/usr/lib", "/System", "/Library")


def run(*args: str) -> str:
    return subprocess.run(args, check=True, text=True, capture_output=True).stdout


def otool_libraries(path: str) -> list[str]:
    """返回 otool -L 里除自身 id 之外的所有依赖。"""
    lines = run("otool", "-L", path).splitlines()[1:]
    return [line.strip().split(" (compat")[0] for line in lines]


def rpaths(path: str) -> list[str]:
    """读取 LC_RPATH。"""
    out = run("otool", "-l", path)
    result: list[str] = []
    lines = out.splitlines()
    for i, line in enumerate(lines):
        if line.strip() == "cmd LC_RPATH":
            for follow in lines[i + 1 : i + 4]:
                stripped = follow.strip()
                if stripped.startswith("path "):
                    result.append(stripped[len("path ") :].split(" (offset")[0])
                    break
    return result


def resolve_dep(dep: str, owner: str) -> str | None:
    """把 @rpath/@loader_path 依赖解析成真实文件路径。"""
    owner_dir = os.path.dirname(owner)
    if dep.startswith("@loader_path/"):
        candidate = os.path.join(owner_dir, dep[len("@loader_path/") :])
        return os.path.realpath(candidate) if os.path.exists(candidate) else None
    if dep.startswith("@executable_path/"):
        # 这里的“可执行文件”对 dylib 来说就是 .app 里的主二进制。
        candidate = os.path.join(owner_dir, dep[len("@executable_path/") :])
        return os.path.realpath(candidate) if os.path.exists(candidate) else None
    if dep.startswith("@rpath/"):
        rest = dep[len("@rpath/") :]
        for rp in rpaths(owner):
            if rp.startswith("@loader_path/"):
                base = os.path.join(owner_dir, rp[len("@loader_path/") :])
            elif rp.startswith("@executable_path/"):
                base = os.path.join(owner_dir, rp[len("@executable_path/") :])
            elif rp.startswith("/"):
                base = rp
            else:
                continue
            candidate = os.path.join(base, rest)
            if os.path.exists(candidate):
                return os.path.realpath(candidate)
        return None
    if dep.startswith("/"):
        return os.path.realpath(dep) if os.path.exists(dep) else None
    return None


def collect_closure(root: str) -> tuple[dict[str, str], list[tuple[str, str]]]:
    """遍历依赖树。

    返回：
      realpath -> 依赖字符串（取第一次见到的写法）
      [(owner realpath, 依赖字符串)]，供重写 install_name 使用
    """
    libs: dict[str, str] = {}
    edges: list[tuple[str, str]] = []
    stack = [root]
    while stack:
        current = os.path.realpath(stack.pop())
        if current in libs:
            continue
        libs[current] = current
        for dep in otool_libraries(current):
            edges.append((current, dep))
            resolved = resolve_dep(dep, current)
            if resolved is None:
                # @rpath 解析不了通常意味着该库只在运行期可选；记下来但不致命。
                if dep.startswith(("/opt/homebrew", "/usr/local")):
                    print(f"  ! 找不到依赖：{dep}（来自 {current}）", file=sys.stderr)
                continue
            if resolved.startswith(BUNDLE_PREFIXES):
                stack.append(resolved)
            elif not resolved.startswith(SYSTEM_PREFIXES):
                print(f"  ! 跳过未知前缀的依赖：{dep}", file=sys.stderr)
    return libs, edges


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--app", required=True, help="目标 .app 路径")
    parser.add_argument("--libmpv", required=True, help="libmpv dylib 路径")
    args = parser.parse_args()

    frameworks = os.path.join(args.app, "Contents", "Frameworks")
    os.makedirs(frameworks, exist_ok=True)

    root = os.path.realpath(args.libmpv)
    if not os.path.exists(root):
        print(f"找不到 libmpv：{root}", file=sys.stderr)
        return 1

    libs, edges = collect_closure(root)
    print(f"依赖闭包：{len(libs)} 个 dylib")

    # 分配 bundle 内的文件名（用真实文件名，避免 symlink 别名造成重复）。
    by_realpath: dict[str, str] = {}
    used: dict[str, str] = {}
    for real in libs:
        name = os.path.basename(real)
        if name in used and used[name] != real:
            raise SystemExit(f"文件名冲突：{used[name]} 与 {real} 都叫 {name}")
        used[name] = real
        by_realpath[real] = name

    for real, name in by_realpath.items():
        shutil.copy2(real, os.path.join(frameworks, name))

    # 重写每个 dylib 自己的 id 和它引用到的闭包内依赖。
    changed = 0
    for real, name in by_realpath.items():
        target = os.path.join(frameworks, name)
        run("install_name_tool", "-id", f"@loader_path/{name}", target)
        for owner, dep in edges:
            if owner != real:
                continue
            resolved = resolve_dep(dep, owner)
            if resolved is None or resolved not in by_realpath:
                continue
            new = f"@loader_path/{by_realpath[resolved]}"
            if new != dep:
                run("install_name_tool", "-change", dep, new, target)
                changed += 1
        # 去掉可能残留的 Homebrew rpath，防止运行时又回落到系统安装的副本。
        for rp in rpaths(target):
            if rp.startswith(("/opt/homebrew", "/usr/local")):
                run("install_name_tool", "-delete_rpath", rp, target)

    # 自检：bundle 内不应再有指向 Homebrew 的绝对依赖。
    leftovers: list[str] = []
    for name in by_realpath.values():
        for dep in otool_libraries(os.path.join(frameworks, name)):
            if dep.startswith(("/opt/homebrew", "/usr/local")):
                leftovers.append(f"{name} -> {dep}")
    if leftovers:
        print("改写后仍有 Homebrew 绝对路径：", file=sys.stderr)
        for item in leftovers:
            print(f"  {item}", file=sys.stderr)
        return 1

    print(f"已拷入 {len(by_realpath)} 个 dylib，重写 {changed} 处依赖")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
