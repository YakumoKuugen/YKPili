#requires -Version 5.1
<#
.SYNOPSIS
    打包 YKPili 的 Windows 绿色版与 NSIS 安装包。

.DESCRIPTION
    1. release 构建 app/crates/yk-app 的 ykpili.exe；
    2. 按 packaging/libmpv/features.toml 裁剪编译 libmpv（已有最新产物则跳过）；
    3. 暂存 exe + libmpv 及其依赖 DLL + 许可证 + 说明；
    4. 压成 dist/YKPili-<版本>-windows-x64.zip；
    5. 调便携版 NSIS 生成 setup.exe（找不到 makensis 时跳过）。

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
# Python/libmpv_features.py 输出 UTF-8；不设这个，中文提示会变乱码。
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }

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

function Get-PythonCommand {
    foreach ($candidate in @("python", "python3")) {
        $cmd = Get-Command $candidate -ErrorAction SilentlyContinue
        if ($cmd) { return $cmd.Source }
    }
    return $null
}

function Find-Msys2Bash {
    $stateFile = Join-Path $env:LOCALAPPDATA "ykpili/msys2-root.txt"
    $roots = @()
    if ($env:MSYS2_ROOT) { $roots += $env:MSYS2_ROOT }
    if (Test-Path $stateFile) {
        $saved = Get-Content $stateFile -TotalCount 1 -ErrorAction SilentlyContinue
        if ($saved) { $roots += $saved.Trim() }
    }
    $roots += "C:\msys64"
    if ($env:LOCALAPPDATA) { $roots += (Join-Path $env:LOCALAPPDATA "ykpili/msys64") }

    foreach ($root in $roots) {
        $c = Join-Path $root "usr\bin\bash.exe"
        if (Test-Path $c) { return $c }
    }
    return $null
}

function Install-Msys2IfNeeded {
    $bash = Find-Msys2Bash
    if ($bash) { return $bash }

    Write-Step "安装 MSYS2 工具链（首次约 400–600 MB，之后复用）"
    $installer = Join-Path $root "scripts/install-msys2.ps1"
    if (-not (Test-Path $installer)) { throw "缺少 MSYS2 安装脚本：$installer" }
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $installer
    if ($LASTEXITCODE -ne 0) { throw "MSYS2 安装失败（exit $LASTEXITCODE）" }

    $bash = Find-Msys2Bash
    if (-not $bash) { throw "MSYS2 安装后仍找不到 usr\bin\bash.exe" }
    return $bash
}

function Test-LibmpvArtifact {
    param([string]$Path, [string]$Platform, [switch]$Quiet)
    $py = Get-PythonCommand
    if (-not $py) {
        Write-Warning "找不到 Python，跳过 libmpv 裁剪校验。"
        return 0
    }
    # Out-Host：别让 python 的输出混进函数返回值，否则 $libmpvCheck 会变成数组。
    $script = Join-Path $root "scripts/libmpv_features.py"
    if ($Quiet) {
        & $py $script verify --platform $Platform --file $Path | Out-Null
    } else {
        & $py $script verify --platform $Platform --file $Path | Out-Host
    }
    return $LASTEXITCODE
}

$version = if ($Version) { $Version } else { Get-WorkspaceVersion }
$commit = Get-GitCommit
$buildProfile = if ($DebugBuild) { "debug" } else { "release" }
$dist = Join-Path $root "dist"
$stage = Join-Path $dist "YKPili-$version-windows-x64"
$zipPath = "$stage.zip"
$setupPath = Join-Path $dist "YKPili-$version-windows-x64-setup.exe"
$libmpvDir = Join-Path $root "packaging/windows/vendor"
$libmpv = Join-Path $libmpvDir "libmpv-2.dll"

Write-Host "YKPili Windows 打包" -ForegroundColor Green
Write-Host "  版本：$version（commit $commit）"
Write-Host "  配置：$buildProfile"

Write-Step "裁剪 libmpv（只编译清单启用的功能）"
$libmpvCheck = Test-LibmpvArtifact -Path $libmpv -Platform "windows" -Quiet
if ($libmpvCheck -eq 0) {
    Write-Host "  已是最新裁剪产物，跳过编译。"
} else {
    $msysBash = Install-Msys2IfNeeded
    Push-Location $root
    try {
        $env:MSYSTEM = "UCRT64"
        & $msysBash -lc "source /etc/profile; ./scripts/build-libmpv.sh --platform windows"
        if ($LASTEXITCODE -ne 0) { throw "libmpv 编译失败（exit $LASTEXITCODE）" }
    } finally {
        Pop-Location
    }
    $libmpvCheck = Test-LibmpvArtifact -Path $libmpv -Platform "windows"
    if ($libmpvCheck -ne 0) {
        throw "libmpv 裁剪校验未通过（exit $libmpvCheck）。排错见 packaging/libmpv/README.md。"
    }
}
$libmpvMb = [math]::Round((Get-Item $libmpv).Length / 1MB, 1)
if ($libmpvMb -gt 60) {
    Write-Warning "libmpv 校验通过但仍有 $libmpvMb MB，检查 features.toml 是否启用了多余能力。"
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
# 裁剪构建的 libmpv 依赖 MSYS2 侧的 libass/libplacebo 等 DLL；
# build-libmpv.sh 会把它们和依赖清单一并放进 vendor，这里跟着装。
Get-ChildItem $libmpvDir -Filter *.dll -File |
    Where-Object { $_.Name -ne "libmpv-2.dll" } |
    ForEach-Object { Copy-Item $_.FullName (Join-Path $stage $_.Name) }
Copy-Item (Join-Path $root "packaging/windows/README-Windows.txt") $stage
Copy-Item (Join-Path $root "packaging/windows/licenses") (Join-Path $stage "LICENSES") -Recurse
# GPL 要求能对应到实际构建配置：把裁剪清单随包带上。
$featuresToml = Join-Path $root "packaging/libmpv/features.toml"
if (Test-Path $featuresToml) {
    Copy-Item $featuresToml (Join-Path $stage "LICENSES/LIBMPV-FEATURES.toml")
}

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
