<#
.SYNOPSIS
    构建环境准备（dev.ps1 与 gate.ps1 共用）。

.DESCRIPTION
    这段逻辑原先长在 dev.ps1 里。抽出来的原因很简单：多了一个 gate.ps1
    之后，两份拷贝迟早会漂移 —— 到时候改了 CC 的查找顺序，
    「开发能跑、质量关却报找不到 gcc」这种事就会发生，而且极难查。

    做四件事：
    1) 工作区里若有 .toolchain/ 就优先用它；
    2) 从 PATH 里剔除过旧的第三方 MinGW（Dev-Cpp 等）；
    3) 给 dlltool 准备 as.exe（windows crate 的 raw-dylib 链接必需）；
    4) 用 CC/AR/RANLIB 指向一个可用的 C 编译器（ring 要编 C）。

    调用方式（必须 dot-source，否则环境变量出不去这个文件）：
        $root = Split-Path -Parent $PSScriptRoot
        . (Join-Path $PSScriptRoot "env.ps1")
#>

if (-not $root) { $root = Split-Path -Parent $PSScriptRoot }

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

# ---- 4) 给 cc-rs 指定 C 编译器（ring 等依赖需要） ----
# 背景：rustls/ring 含 C 与汇编，编译期需要 C 编译器（cc-rs 先看 CC 再找 gcc）。
# 但 rustup 的 GNU 工具链只带一个 gcc 驱动的**链接器壳**，没有 cc1，编不了 C；
# 所以必须借一个外部 MinGW 的 gcc。
#
# 关键约束：**不能把它放进 PATH**。它的 ld/gcc 会被 rustc 当成链接器，
# 结果就是 "ld: cannot find crt2.o / -lwinhttp / -luser32"（实测踩过）。
# 正确做法：PATH 里剔除它，只用 CC/AR/RANLIB 环境变量告诉 cc-rs 去哪找。
$ccCandidates = @(
    (Join-Path $root ".toolchain\mingw-cc\gcc.exe"),
    "D:\Dev-Cpp\MinGW64\bin\gcc.exe",
    "C:\mingw64\bin\gcc.exe"
)
foreach ($cand in $ccCandidates) {
    if (Test-Path $cand) {
        $env:CC = $cand
        $ccDir = Split-Path -Parent $cand
        foreach ($tool in @('AR', 'RANLIB')) {
            $p = Join-Path $ccDir "$($tool.ToLower()).exe"
            if (Test-Path $p) { Set-Item -Path "env:$tool" -Value $p }
        }
        Write-Host "[info] C 依赖编译器(CC): $cand" -ForegroundColor DarkGray
        break
    }
}
if (-not $env:CC) {
    Write-Host "[warn] 未找到 C 编译器。若报 'failed to find tool gcc.exe'（ring 等），" -ForegroundColor Yellow
    Write-Host "       请安装一个 MinGW-w64 并把 gcc.exe 放到 .toolchain\mingw-cc\ 下。" -ForegroundColor Yellow
}
