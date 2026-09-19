<#
.SYNOPSIS
    构建并运行 VibeClassAgent（开发用）。

.DESCRIPTION
    1) 若工作区内存在 .toolchain/，优先使用它（适配未装 rustup 或用户目录无写权限的环境）；
    2) 自动从 PATH 中剔除过旧的第三方 MinGW（如 Dev-Cpp），
       否则 rustc 会误用其旧链接器导致 "unrecognized option '--high-entropy-va'"；
    3) 随后构建并运行 CLI。

.PARAMETER Args
    透传给 vca 的参数，例如 doctor。

.PARAMETER Release
    使用 release 配置构建并运行。

.PARAMETER Check
    只执行 cargo check，不运行程序。

.EXAMPLE
    .\scripts\dev.ps1
    .\scripts\dev.ps1 -Args doctor
    .\scripts\dev.ps1 -Args timetable,list
    .\scripts\dev.ps1 -Release
    .\scripts\dev.ps1 -Check
#>
[CmdletBinding()]
param(
    [string[]]$Args = @(),
    [switch]$Release,
    [switch]$Check
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot

# ---- 1) 优先使用工作区本地工具链 ----
$localToolchain = Join-Path $root ".toolchain"
if (Test-Path $localToolchain) {
    $env:CARGO_HOME  = Join-Path $localToolchain "cargo"
    $env:RUSTUP_HOME = Join-Path $localToolchain "rustup"
    $cargoBin = Join-Path $env:CARGO_HOME "bin"
    if (Test-Path $cargoBin) { $env:PATH = "$cargoBin;$env:PATH" }
    Write-Host "[info] 使用工作区本地工具链: $localToolchain" -ForegroundColor DarkGray
}

# ---- 2) 剔除会干扰链接的旧 MinGW ----
# 说明：若 PATH 中存在 Dev-Cpp 等自带的老版本 MinGW，rustc 可能优先选中它，
# 其 ld 不支持 rustc 传入的 --high-entropy-va，导致链接失败。
$badPatterns = @("*\Dev-Cpp\*", "*\MinGW\bin*")
$before = $env:PATH
$parts = $env:PATH -split ';' | Where-Object {
    $p = $_
    if ([string]::IsNullOrWhiteSpace($p)) { return $false }
    foreach ($pat in $badPatterns) { if ($p -like $pat) { return $false } }
    return $true
}
$env:PATH = ($parts -join ';')
if ($env:PATH -ne $before) {
    Write-Host "[info] 已从 PATH 中剔除过旧的 MinGW（避免链接器被误用）" -ForegroundColor DarkGray
}

# ---- 3) 为 dlltool 准备 as.exe（windows crate 的 raw-dylib 链接必需） ----
# 背景：windows crate 大量使用 #[link(kind = "raw-dylib")]，rustc 在
# windows-gnu 目标下必须调用 dlltool 生成导入库。而 dlltool **忽略 AS 环境
# 变量**，只在自己所在目录里找 `as`；rustup 的 GNU 工具链自带
# dlltool.exe 与 ld.exe，却**不带 as.exe**，于是报
#     dlltool.exe: CreateProcess
# 处理：把一个可用的 as.exe 放进 self-contained 目录（只需一次）。
$selfContained = Join-Path $root ".toolchain\rustup\toolchains\stable-x86_64-pc-windows-gnu\lib\rustlib\x86_64-pc-windows-gnu\bin\self-contained"
if (Test-Path $selfContained) {
    if (-not (Test-Path (Join-Path $selfContained "as.exe"))) {
        $localAs = Join-Path $root ".toolchain\mingw-as\as.exe"
        if (Test-Path $localAs) {
            Copy-Item $localAs (Join-Path $selfContained "as.exe") -Force
            Write-Host "[info] 已为 dlltool 准备 as.exe" -ForegroundColor DarkGray
        }
        else {
            Write-Host "[warn] dlltool 缺少 as.exe。若链接报 'dlltool.exe: CreateProcess'，" -ForegroundColor Yellow
            Write-Host "       请把任意 MinGW 的 as.exe 复制到：$selfContained" -ForegroundColor Yellow
        }
    }
    $env:PATH = "$selfContained;$env:PATH"
}

Push-Location $root
try {
    if ($Check) {
        Write-Host "[info] cargo check" -ForegroundColor Cyan
        & cargo check
    }
    elseif ($Release) {
        Write-Host "[info] cargo build --release" -ForegroundColor Cyan
        & cargo build --release
        & cargo run --release -p vca-cli -- @Args
    }
    else {
        Write-Host "[info] cargo run -p vca-cli -- $($Args -join ' ')" -ForegroundColor Cyan
        & cargo run -p vca-cli -- @Args
    }
}
finally {
    Pop-Location
}
