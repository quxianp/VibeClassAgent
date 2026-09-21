<#
.SYNOPSIS
    构建并运行 VibeClassAgent（开发用）。

.DESCRIPTION
    1) 若工作区内存在 .toolchain/，优先使用它（适配未装 rustup 或用户目录无写权限的环境）；
    2) 自动从 PATH 中剔除过旧的第三方 MinGW（如 Dev-Cpp），
       否则 rustc 会误用其旧链接器导致 "unrecognized option '--high-entropy-va'"；
    3) 随后构建并运行 CLI。

.PARAMETER Command
    透传给 vca 的参数，例如 doctor。
    注意：这里**刻意不叫 `$Args`** —— 那是 PowerShell 的自动变量，
    拿它当参数名既容易冲突，又会在命令行只传 `-Args`（后面没值）时报
    "Missing an argument for parameter 'Args'"。改用剩余参数绑定之后，
    `dev.cmd doctor` 这种直接位置传入也能接住。

.PARAMETER Release
    使用 release 配置构建并运行。

.PARAMETER Check
    只执行 cargo check，不运行程序。

.EXAMPLE
    .\scripts\dev.cmd
    .\scripts\dev.cmd doctor
    .\scripts\dev.cmd run --dry-run
    .\scripts\dev.cmd -Check
#>
[CmdletBinding()]
param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Command = @(),
    [switch]$Release,
    [switch]$Check
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
# ---- 环境准备（本地工具链 / 剔除旧 MinGW / dlltool 的 as / C 编译器）----
# 具体逻辑在 env.ps1 里，与 scripts\gate.ps1 共用，免得两份拷贝漂移。
. (Join-Path $PSScriptRoot "env.ps1")

Push-Location $root
try {
    if ($Check) {
        Write-Host "[info] cargo check" -ForegroundColor Cyan
        & cargo check
    }
    elseif ($Release) {
        Write-Host "[info] cargo build --release" -ForegroundColor Cyan
        & cargo build --release
        & cargo run --release -p vca-cli -- @Command
    }
    else {
        Write-Host "[info] cargo run -p vca-cli -- $($Command -join ' ')" -ForegroundColor Cyan
        & cargo run -p vca-cli -- @Command
    }
}
finally {
    Pop-Location
}
