#requires -Version 5.1
<#
.SYNOPSIS
    打包 YKPili 的 Windows 绿色版与 NSIS 安装包。

.DESCRIPTION
    1. release 构建 app/crates/yk-app 的 ykpili.exe；
    2. 暂存 exe + libmpv-2.dll + 许可证 + 说明；
    3. 压成 dist/YKPili-<版本>-windows-x64.zip；
    4. 调便携版 NSIS 生成 setup.exe（找不到 makensis 时跳过）。

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\scripts\package-windows.ps1

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\scripts\package-windows.ps1 -SkipBuild -NoInstaller
#>
[CmdletBinding()]
param(
    [string]$Version = "",
    [switch]$SkipBuild,
    [switch]$DebugBuild,
    [switch]$NoInstaller
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Write-Step([string]$Message) {
    Write-Host ""
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Get-WorkspaceVersion {
    $text = Get-Content (Join-Path $root "app/Cargo.toml") -Raw
    $m = [regex]::Match($text, '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"')
    if (-not $m.Success) { throw "无法从 app/Cargo.toml 读取 workspace.package.version" }
    return $m.Groups[1].Value
}

function Get-GitCommit {
    try {
        $commit = (& git rev-parse --short HEAD 2>$null).Trim()
        if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($commit)) { return "unknown" }
        $dirty = (& git status --porcelain 2>$null)
        if ($dirty) { return "$commit-dirty" }
        return $commit
    } catch {
        return "unknown"
    }
}

function Resolve-CargoArgs {
    # gpui-fast 0.1.10 用了 std::hint::cold_path，Rust 1.94 编译不过。
    #
    # rustc -V 的输出形如：
    #   rustc 1.99.1 (58cf8f9c3 2026-10-13)
    #   rustc 1.99.0-nightly (012345678 2026-01-02)
    # 提交日期本身带连字符，不能按 '-' 切分（会切出
    # "1.99.1 (58cf8f9c3 2026" 这种非法版本号），必须用正则只取前三段数字。
    foreach ($candidate in @("1.99.1", "stable")) {
        $probe = @()
        if ($candidate -ne "stable") { $probe += "+$candidate" }
        $probe += "-V"
        $out = & rustc @probe 2>$null
        if ($LASTEXITCODE -ne 0 -or -not $out) { continue }
        $versionText = ($out | Select-Object -First 1) -replace '^rustc\s+', ''
        $m = [regex]::Match($versionText, '^\s*(\d+\.\d+\.\d+)')
        if (-not $m.Success) { continue }
        try {
            $v = [version]$m.Groups[1].Value
        } catch {
            continue
        }
        if ($v -lt [version]"1.95.0") { continue }
        $cargoArgs = @()
        if ($candidate -ne "stable") { $cargoArgs += "+$candidate" }
        Write-Host "使用 Rust 工具链：$versionText（$candidate）"
        return $cargoArgs
    }
    throw "需要 Rust 1.95+（当前默认工具链过旧）。请 rustup update stable 或安装 1.99.1。"
}

function Find-SevenZip {
    $candidates = @(
        (Join-Path $env:ProgramFiles "7-Zip/7z.exe"),
        (Join-Path ${env:ProgramFiles(x86)} "7-Zip/7z.exe"),
        "7z"
    )
    foreach ($c in $candidates) {
        if (-not $c) { continue }
        if (Test-Path $c) { return (Resolve-Path $c).Path }
        $cmd = Get-Command $c -ErrorAction SilentlyContinue
        if ($cmd) { return $cmd.Source }
    }
    return $null
}

$version = if ($Version) { $Version } else { Get-WorkspaceVersion }
$commit = Get-GitCommit
$buildProfile = if ($DebugBuild) { "debug" } else { "release" }
$dist = Join-Path $root "dist"
$stage = Join-Path $dist "YKPili-$version-windows-x64"
$zipPath = "$stage.zip"
$setupPath = Join-Path $dist "YKPili-$version-windows-x64-setup.exe"
$libmpv = Join-Path $root "packaging/windows/vendor/libmpv-2.dll"

Write-Host "YKPili Windows 打包" -ForegroundColor Green
Write-Host "  版本：$version（commit $commit）"
Write-Host "  配置：$buildProfile"

if (-not (Test-Path $libmpv)) {
    throw "缺少 libmpv：$libmpv。见 packaging/windows/README.md 的「libmpv」一节。"
}

# ---------- 1. 构建 ----------
if (-not $SkipBuild) {
    Write-Step "编译 ykpili.exe（$buildProfile）"
    $cargoArgs = Resolve-CargoArgs
    $buildArgs = @()
    $buildArgs += $cargoArgs
    $buildArgs += @("build", "--manifest-path", "app/Cargo.toml", "-p", "yk-app", "--bin", "ykpili")
    if (-not $DebugBuild) { $buildArgs += "--release" }
    & cargo @buildArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo build 失败（exit $LASTEXITCODE）" }
} else {
    Write-Step "跳过构建（-SkipBuild）"
}

$exe = Join-Path $root "app/target/$buildProfile/ykpili.exe"
if (-not (Test-Path $exe)) { throw "找不到构建产物：$exe" }

# ---------- 2. 暂存 ----------
Write-Step "准备暂存目录"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force $stage | Out-Null

Copy-Item $exe (Join-Path $stage "ykpili.exe")
Copy-Item $libmpv (Join-Path $stage "libmpv-2.dll")
Copy-Item (Join-Path $root "packaging/windows/README-Windows.txt") $stage
Copy-Item (Join-Path $root "packaging/windows/licenses") (Join-Path $stage "LICENSES") -Recurse

# SOURCE.txt 里填版本与提交，便于分发时追溯对应源码。
# 必须用显式 UTF-8 读写：Windows PowerShell 5.1 的 Get-Content/Set-Content
# 会按系统 ANSI 代码页解码，全角冒号后面的下划线会被当成双字节字符吃掉，
# 导致 __APP_VERSION__ 替换不上。
$sourcePath = Join-Path $stage "LICENSES/SOURCE.txt"
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$utf8Bom = New-Object System.Text.UTF8Encoding($true)
$sourceText = [System.IO.File]::ReadAllText($sourcePath, $utf8NoBom)
$sourceText = $sourceText.Replace("__APP_VERSION__", $version).Replace("__GIT_COMMIT__", $commit)
[System.IO.File]::WriteAllText($sourcePath, $sourceText, $utf8Bom)

# 说明文件同样补 BOM，老版记事本打开不会乱码。
$readmePath = Join-Path $stage "README-Windows.txt"
$readmeText = [System.IO.File]::ReadAllText($readmePath, $utf8NoBom)
[System.IO.File]::WriteAllText($readmePath, $readmeText, $utf8Bom)

Write-Host ("  暂存：{0}（{1:N1} MB）" -f $stage, ((Get-ChildItem $stage -Recurse -File | Measure-Object Length -Sum).Sum / 1MB))

# ---------- 3. ZIP ----------
Write-Step "生成绿色包 zip"
if (Test-Path $zipPath) { Remove-Item $zipPath -Force }
$seven = Find-SevenZip
if ($seven) {
    & $seven a -tzip -mx=7 -bso0 -bsp0 $zipPath (Join-Path $stage "*") | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "7-Zip 压缩失败（exit $LASTEXITCODE）" }
} else {
    Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zipPath -CompressionLevel Optimal
}
Write-Host ("  {0}（{1:N1} MB）" -f $zipPath, ((Get-Item $zipPath).Length / 1MB))

# ---------- 4. 安装包 ----------
if ($NoInstaller) {
    Write-Step "跳过安装包（-NoInstaller）"
} else {
    Write-Step "生成 NSIS 安装包"
    $makensis = Get-ChildItem (Join-Path $root "packaging/windows/tools") -Recurse -Filter "makensis.exe" -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if (-not $makensis) {
        Write-Warning "找不到 makensis.exe，跳过安装包。"
        Write-Warning "下载 NSIS 便携版解压到 packaging/windows/tools/nsis-*/Bin/makensis.exe 后重跑，或加 -NoInstaller。"
    } else {
        if (Test-Path $setupPath) { Remove-Item $setupPath -Force }
        Push-Location (Join-Path $root "packaging/windows")
        try {
            & $makensis.FullName /INPUTCHARSET UTF8 "/DAPP_VERSION=$version" "/DSOURCE_DIR=$stage" "/DOUTFILE=$setupPath" "installer.nsi"
            if ($LASTEXITCODE -ne 0) { throw "makensis 失败（exit $LASTEXITCODE）" }
        } finally {
            Pop-Location
        }
        Write-Host ("  {0}（{1:N1} MB）" -f $setupPath, ((Get-Item $setupPath).Length / 1MB))
    }
}

Write-Host ""
Write-Host "完成。产物：" -ForegroundColor Green
Write-Host "  $stage"
Write-Host "  $zipPath"
if ((Test-Path $setupPath) -and -not $NoInstaller) { Write-Host "  $setupPath" }
