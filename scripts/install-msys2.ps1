#requires -Version 5.1
<#
.SYNOPSIS
    确保本机有能编译裁剪版 libmpv 的 MSYS2 UCRT64 工具链（仅 Windows）。

.DESCRIPTION
    打包脚本 / build-libmpv.sh 发现没有 MSYS2 时会调用本脚本：

      1. 先找现成的 MSYS2：MSYS2_ROOT → %LOCALAPPDATA%\ykpili\msys2-root.txt
         → C:\msys64 → %LOCALAPPDATA%\ykpili\msys64；
      2. 没有就下载固定版本的基础包（SHA256 校验）静默解压到
         %LOCALAPPDATA%\ykpili\msys64（不需要管理员权限）；
      3. 首次初始化 keyring / 包库，再用 pacman 装编译依赖（gcc/meson/ninja/
         libass/libplacebo/...），已装的用 --needed 跳过；
      4. 最后输出一行 `MSYS2_ROOT=<路径>`，调用方据此拿到 bash.exe。

    幂等：依赖已经齐全时只做一次 `pacman -Q` 检查，不联网。

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\scripts\install-msys2.ps1
#>
[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }

# 固定版本 + 官方发布包的 SHA256：升级时同时改 URL 与哈希。
$MsysVersion = "20260927"
$MsysSha256  = "ad336cccfda47758b5e15cda993fbba421115cb0b126697daef1ee4dfe37209f"
$MsysUrl     = "https://repo.msys2.org/distrib/x86_64/msys2-base-x86_64-$MsysVersion.sfx.exe"

# build-libmpv.sh 需要的包：编译器/构建系统 + mpv 的硬依赖 + AV1 解码器。
$Packages = @(
    "make",
    "git",
    "curl",
    "mingw-w64-ucrt-x86_64-gcc",
    "mingw-w64-ucrt-x86_64-ninja",
    "mingw-w64-ucrt-x86_64-meson",
    "mingw-w64-ucrt-x86_64-pkgconf",
    "mingw-w64-ucrt-x86_64-nasm",
    "mingw-w64-ucrt-x86_64-libass",
    "mingw-w64-ucrt-x86_64-libplacebo",
    "mingw-w64-ucrt-x86_64-harfbuzz",
    "mingw-w64-ucrt-x86_64-freetype",
    "mingw-w64-ucrt-x86_64-fribidi",
    "mingw-w64-ucrt-x86_64-dav1d"
)

$StateDir  = Join-Path $env:LOCALAPPDATA "ykpili"
$StateFile = Join-Path $StateDir "msys2-root.txt"

function Write-Step([string]$Message) {
    Write-Host ""
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Get-InstalledRoot {
    $candidates = @()
    if ($env:MSYS2_ROOT) { $candidates += $env:MSYS2_ROOT }
    if (Test-Path $StateFile) {
        $saved = (Get-Content $StateFile -TotalCount 1 -ErrorAction SilentlyContinue)
        if ($saved) { $candidates += $saved.Trim() }
    }
    $candidates += "C:\msys64"
    if ($env:LOCALAPPDATA) { $candidates += (Join-Path $StateDir "msys64") }

    foreach ($candidate in $candidates) {
        if (-not $candidate) { continue }
        if (Test-Path (Join-Path $candidate "usr\bin\bash.exe")) { return $candidate }
    }
    return $null
}

function Invoke-MsysBash {
    param([string]$Root, [string]$Command)
    # UCRT64：/etc/profile 会据此把 /ucrt64/bin 放进 PATH。
    $env:MSYSTEM = "UCRT64"
    $bash = Join-Path $Root "usr\bin\bash.exe"
    & $bash -lc $Command
    return $LASTEXITCODE
}

function Test-ToolchainReady {
    param([string]$Root)
    $pkgArgs = ($Packages -join " ")
    if ((Invoke-MsysBash $Root "pacman -Q $pkgArgs >/dev/null 2>&1") -ne 0) { return $false }
    # 版本/依赖能对上才算真的可用（装了包但 PATH 不对也会在这里暴露）。
    $probe = "pkg-config --exists libass && pkg-config --exists libplacebo && " +
             "gcc --version >/dev/null && meson --version >/dev/null && " +
             "ninja --version >/dev/null && make --version >/dev/null"
    return ((Invoke-MsysBash $Root $probe) -eq 0)
}

function Install-Msys2 {
    $parent = $StateDir
    New-Item -ItemType Directory -Force $parent | Out-Null
    $root = Join-Path $parent "msys64"

    # 只在「我们自己的」安装目录里做删除重建；C:\msys64 之类的既有安装不动。
    if (Test-Path $root) {
        Write-Step "清理不完整的 MSYS2 安装：$root"
        Remove-Item $root -Recurse -Force
    }

    $sfx = Join-Path $env:TEMP "msys2-base-$MsysVersion.sfx.exe"
    $needDownload = $true
    if (Test-Path $sfx) {
        $existing = (Get-FileHash $sfx -Algorithm SHA256).Hash.ToLower()
        $needDownload = ($existing -ne $MsysSha256)
    }
    if ($needDownload) {
        Write-Step "下载 MSYS2 $MsysVersion（约 42 MB）"
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        $part = "$sfx.part"
        if (Test-Path $part) { Remove-Item $part -Force }
        $curl = Get-Command curl.exe -ErrorAction SilentlyContinue
        if ($curl) {
            & $curl.Source -fL --retry 3 --connect-timeout 20 -o $part $MsysUrl
            if ($LASTEXITCODE -ne 0) { throw "下载 MSYS2 失败（curl exit $LASTEXITCODE）" }
        } else {
            Invoke-WebRequest -Uri $MsysUrl -OutFile $part -UseBasicParsing
        }
        $hash = (Get-FileHash $part -Algorithm SHA256).Hash.ToLower()
        if ($hash -ne $MsysSha256) {
            Remove-Item $part -Force
            throw "MSYS2 安装包校验失败：$hash（期望 $MsysSha256）"
        }
        Move-Item $part $sfx -Force
    }

    Write-Step "解压 MSYS2 到 $parent"
    $outLog = Join-Path $env:TEMP "msys2-extract.out.log"
    $errLog = Join-Path $env:TEMP "msys2-extract.err.log"
    $proc = Start-Process -FilePath $sfx `
        -ArgumentList @("-y", ('-o"' + $parent + '"')) `
        -Wait -PassThru -NoNewWindow `
        -RedirectStandardOutput $outLog -RedirectStandardError $errLog
    if ($proc.ExitCode -ne 0) {
        throw "MSYS2 解压失败（exit $($proc.ExitCode)），日志：$outLog / $errLog"
    }
    if (-not (Test-Path (Join-Path $root "usr\bin\bash.exe"))) {
        throw "解压后找不到 $root\usr\bin\bash.exe"
    }

    # 首次启动只做 skeleton 初始化（不会执行传入的命令），所以先空跑一次。
    Write-Step "初始化 MSYS2"
    Invoke-MsysBash $root "true" | Out-Null
    Invoke-MsysBash $root "true" | Out-Null
    return $root
}

# ---------- 主流程 ----------
if ($env:OS -ne "Windows_NT") {
    throw "本脚本只用于 Windows；macOS 请用 Homebrew 装 meson/ninja/libass/libplacebo。"
}

$root = Get-InstalledRoot
if (-not $root) {
    $root = Install-Msys2
}

if (Test-ToolchainReady $root) {
    Write-Host "MSYS2 工具链就绪：$root"
} else {
    Write-Step "同步 MSYS2 包库"
    # 只 -Sy 不 -Syu：升级 msys2-runtime 时 bash.exe 正被占用，容易升级到一半失败。
    # 基础包是固定版本，直接装依赖足够；真需要滚动升级时请手动开 UCRT64 终端。
    $null = Invoke-MsysBash $root "pacman-key --init && pacman-key --populate msys2"
    if ((Invoke-MsysBash $root "pacman -Sy --noconfirm --disable-download-timeout") -ne 0) {
        throw "pacman 同步包库失败。检查网络后在 UCRT64 终端执行 pacman -Sy。"
    }

    Write-Step "安装编译依赖（首次约 400–600 MB）"
    $pkgArgs = ($Packages -join " ")
    $install = Invoke-MsysBash $root "pacman -S --needed --noconfirm --disable-download-timeout $pkgArgs"
    if ($install -ne 0) {
        Write-Warning "第一次安装失败，重试一次…"
        $install = Invoke-MsysBash $root "pacman -S --needed --noconfirm --disable-download-timeout $pkgArgs"
    }
    if ($install -ne 0) {
        throw "pacman 安装依赖失败（exit $install）。可在 MSYS2 UCRT64 终端手动执行：" +
              "pacman -S --needed --noconfirm $pkgArgs"
    }

    if (-not (Test-ToolchainReady $root)) {
        throw "依赖装完了但工具链自检不通过（gcc/meson/pkg-config/libass/libplacebo）。"
    }
    Write-Host "MSYS2 工具链就绪：$root"
}

New-Item -ItemType Directory -Force $StateDir | Out-Null
Set-Content -Path $StateFile -Value $root -Encoding UTF8
Write-Output "MSYS2_ROOT=$root"
