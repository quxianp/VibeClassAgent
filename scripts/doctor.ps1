<#
.SYNOPSIS
    环境自检：检查构建与运行所需的依赖。

.DESCRIPTION
    依次检查 Rust 工具链、ffmpeg、Python、LibreOffice，输出缺失项与用途说明。
    仅做只读检测，不修改任何系统设置。

.EXAMPLE
    .\scripts\doctor.ps1
#>
[CmdletBinding()]
param()

$ErrorActionPreference = "Continue"
$root = Split-Path -Parent $PSScriptRoot

function Test-Tool {
    param(
        [string]$Name,
        [string]$Arg,
        [string]$Purpose,
        [bool]$Required
    )
    $cmd = Get-Command $Name -ErrorAction SilentlyContinue
    if ($null -eq $cmd) {
        $tag = if ($Required) { "必需" } else { "可选" }
        Write-Host ("[{0,-4}] {1,-10} 未找到（{2}；用途：{3}）" -f "MISS", $Name, $tag, $Purpose) -ForegroundColor Yellow
        return $false
    }
    try { $ver = (& $Name $Arg 2>&1 | Select-Object -First 1) } catch { $ver = "已安装" }
    Write-Host ("[{0,-4}] {1,-10} {2}" -f "OK", $Name, $ver) -ForegroundColor Green
    return $true
}

Write-Host "VibeClassAgent 环境自检" -ForegroundColor Cyan
Write-Host ("-" * 64)

# 本机适配：优先本地工具链，并剔除干扰的旧 MinGW
$localToolchain = Join-Path $root ".toolchain"
if (Test-Path $localToolchain) {
    $env:CARGO_HOME  = Join-Path $localToolchain "cargo"
    $env:RUSTUP_HOME = Join-Path $localToolchain "rustup"
    $cargoBin = Join-Path $env:CARGO_HOME "bin"
    if (Test-Path $cargoBin) { $env:PATH = "$cargoBin;$env:PATH" }
    Write-Host "[info] 检测到工作区本地工具链 .toolchain/" -ForegroundColor DarkGray
}
$env:PATH = (($env:PATH -split ';' | Where-Object {
    $_ -and ($_ -notlike "*\Dev-Cpp\*") -and ($_ -notlike "*\MinGW\bin*")
}) -join ';')

Test-Tool -Name "cargo"   -Arg "--version" -Purpose "构建 Rust 宿主"  -Required $true  | Out-Null
Test-Tool -Name "rustc"   -Arg "--version" -Purpose "Rust 编译器"      -Required $true  | Out-Null
Test-Tool -Name "ffmpeg"  -Arg "-version"  -Purpose "录屏录音"         -Required $false | Out-Null
Test-Tool -Name "python"  -Arg "--version" -Purpose "转写与文档生成"    -Required $false | Out-Null
Test-Tool -Name "soffice" -Arg "--version" -Purpose "Word 转 PDF"      -Required $false | Out-Null

Write-Host ("-" * 64)
Write-Host "缺失项不影响框架骨架运行，仅在对应功能实现后才需要。" -ForegroundColor DarkGray
Write-Host "程序内部自检：.\scripts\dev.ps1 -Args doctor" -ForegroundColor DarkGray
