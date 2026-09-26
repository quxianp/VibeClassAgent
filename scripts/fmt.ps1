<#
.SYNOPSIS
    按工作区工具链跑 cargo fmt --all（就地格式化）。

.DESCRIPTION
    直接敲 `cargo fmt` 会走 rustup 的默认工具链，而本仓库用的是
    `.toolchain/` 里的本地工具链（见 scripts\env.ps1）——
    实测会报 `rustup could not choose a version of cargo to run`。
    所以格式化也统一走脚本，跟 dev.cmd / gate.cmd 共用同一份环境准备。

.EXAMPLE
    .\scripts\fmt.cmd
#>
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot "env.ps1")

Push-Location $root
try {
    # 原生程序往 stderr 写字会被 Stop 偏好当成终止错误（见 gate.ps1 里的说明）
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & cargo fmt --all | Out-Default
        $code = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $prev
    }
}
finally {
    Pop-Location
}
if ($code -ne 0) { exit $code }
Write-Host "已格式化 ✓" -ForegroundColor Green
