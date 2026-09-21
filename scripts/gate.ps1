<#
.SYNOPSIS
    提交前的三道关：格式 / 静态检查 / 单元测试。

.DESCRIPTION
    AGENTS.md 里写着「改动过三关再提交」，但三关是三行命令，
    手敲既慢、又总有人漏掉 clippy。这里一次跑完并在结尾给一句结论。

    1) cargo fmt --all -- --check   格式（不过就先跑 cargo fmt --all）
    2) cargo clippy --workspace --all-targets -- -D warnings
       —— 加 -D warnings 是刻意的：本项目的标准是**零告警**，
          只把警告打印出来，攒到几十条之后就再也没人看了。
    3) cargo test --workspace       单元测试

.PARAMETER SkipFmt
    跳过格式检查（比如你在中途只想看看测试过不过）。

.PARAMETER SkipTest
    跳过单元测试（比如只想快速看 clippy）。

.EXAMPLE
    .\scripts\gate.cmd
    .\scripts\gate.cmd -SkipTest
#>
[CmdletBinding()]
param(
    [switch]$SkipFmt,
    [switch]$SkipTest
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot "env.ps1")

# 本机只有 8 GB 内存，并行链接会把 rustc 打到 "Allocation failed"。
# 只在用户没显式设置时兜底，免得覆盖掉他自己调的并行度。
if (-not $env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS = "1" }

# PowerShell 有个老坑必须绕开：$ErrorActionPreference = "Stop" 时，
# 原生程序往 stderr 写一行就会被当作 terminating error 抛出去，
# 而 cargo 的编译错误与 clippy 告警**全都走 stderr**。
# 于是脚本在半路被打断：该跑的检查没跑完，结尾也不给结论，
# 只留下一屏看不懂的编译输出（本项目实测踩过）。
# 处理：每次调外部命令前把偏好切回 Continue，用 $LASTEXITCODE 判成败。
function Invoke-Cargo {
    param([string[]]$CargoArgs)
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        # 这里必须 `| Out-Default`，不能光写 `& cargo @CargoArgs`：
        # PowerShell 会把函数体里**所有没被消费的输出**拼成返回值，
        # 于是 cargo test 的 stdout（Running.../test result: ok...）
        # 会和退出码一起塞进 $code —— $code 变成数组之后，
        # `if ($code -ne 0)` 走的是「过滤」语义（返回非空数组即真），
        # 结果就是：八个测试二进制全绿，gate 却报「未通过：test」。
        # 实测踩过。clippy 那一步蒙对是因为 cargo 的进度信息走 stderr。
        & cargo @CargoArgs | Out-Default
        return $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $prev
    }
}

$failed = @()
Push-Location $root
try {
    if (-not $SkipFmt) {
        Write-Host "[1/3] cargo fmt --all -- --check" -ForegroundColor Cyan
        $code = Invoke-Cargo @("fmt", "--all", "--", "--check")
        if ($code -ne 0) {
            Write-Host "      格式不合规。跑 cargo fmt --all 修一下即可（改动纯属排版，不会动逻辑）" -ForegroundColor Yellow
            $failed += "fmt"
        }
    }

    Write-Host "[2/3] cargo clippy --workspace --all-targets" -ForegroundColor Cyan
    $code = Invoke-Cargo @("clippy", "--workspace", "--all-targets", "--", "-D", "warnings")
    if ($code -ne 0) { $failed += "clippy" }

    if (-not $SkipTest) {
        Write-Host "[3/3] cargo test --workspace" -ForegroundColor Cyan
        $code = Invoke-Cargo @("test", "--workspace")
        if ($code -ne 0) { $failed += "test" }
    }
}
finally {
    Pop-Location
}

if ($failed.Count -gt 0) {
    Write-Host ""
    Write-Host "未通过：$($failed -join ' / ')" -ForegroundColor Red
    exit 1
}
Write-Host ""
Write-Host "三关全过 ✓" -ForegroundColor Green
