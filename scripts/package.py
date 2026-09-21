# -*- coding: utf-8 -*-
"""打包发布：产出可以直接拷到一体机上运行的目录。

为什么要单独打包
----------------
`target/release/vca.exe` 只是主程序（约 5.5 MB），它还需要：

    tools/ffmpeg/     屏幕采集与音视频合并
    tools/whisper/    本地语音转写（含模型）

这些加起来接近 190 MB，不可能编进 exe 里 —— 每次启动都解压一遍不现实。
所以发布形态是**一个自带运行时的目录**：整个目录拷走就能换机器用，
程序与数据都在同一个文件夹里，不碰系统盘。

目录结构
--------
    VibeClassAgent/
      vca.exe                 主程序
      启动.cmd                双击即进交互界面
      首次设置.cmd            第一次用跑这个
      tools/ffmpeg/…          采集与合并
      tools/whisper/…         本地转写
      config/…                配置模板（首次运行会拷成实际配置）
      data/…                  数据（录像、文档、作业）—— 运行时生成
      README.txt              给使用者的说明

用法
----
    python scripts/package.py                 # 打完整包
    python scripts/package.py --no-model      # 不带模型（体积小，转写走云端）
    python scripts/package.py --out D:\\dist    # 指定输出位置
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

for _s in (sys.stdout, sys.stderr):
    try:
        _s.reconfigure(encoding="utf-8")  # type: ignore[union-attr]
    except Exception:  # noqa: BLE001
        pass

APP_NAME = "VibeClassAgent"


def log(msg: str) -> None:
    print(msg, flush=True)


def run_cargo_release() -> Path:
    """编译 release，返回 vca.exe 路径。"""
    env = os.environ.copy()
    toolchain = ROOT / ".toolchain"
    if toolchain.is_dir():
        env["CARGO_HOME"] = str(toolchain / "cargo")
        env["RUSTUP_HOME"] = str(toolchain / "rustup")
        env["PATH"] = str(toolchain / "cargo" / "bin") + os.pathsep + env.get("PATH", "")
        self_contained = (
            toolchain
            / "rustup/toolchains/stable-x86_64-pc-windows-gnu/lib/rustlib"
            / "x86_64-pc-windows-gnu/bin/self-contained"
        )
        if self_contained.is_dir():
            env["PATH"] = str(self_contained) + os.pathsep + env["PATH"]
        # C 依赖（ring 等）需要一个真正的 C 编译器，但**不能**把 MinGW 放进 PATH，
        # 否则它的 ld 会被 rustc 当链接器。只用 CC/AR 告诉 cc-rs 去哪找。
        for cand in [
            toolchain / "mingw-cc/gcc.exe",
            Path(r"D:\Dev-Cpp\MinGW64\bin\gcc.exe"),
            Path(r"C:\mingw64\bin\gcc.exe"),
        ]:
            if cand.is_file():
                env["CC"] = str(cand)
                for tool in ("ar", "ranlib"):
                    p = cand.parent / f"{tool}.exe"
                    if p.is_file():
                        env[tool.upper()] = str(p)
                break

    # 关键：把旧的 MinGW 从 PATH 里剔掉。
    # 它的 ld 会被 rustc 当成链接器，然后找不到 rustup 自带的运行库，
    # 报一串 "ld: cannot find -lntdll / crtend.o"。
    # 剔除发生在取完 CC 之后，所以两者不冲突。
    parts = [p for p in env.get("PATH", "").split(os.pathsep) if p]
    parts = [
        p
        for p in parts
        if "Dev-Cpp" not in p and not p.rstrip("\\").lower().endswith("mingw\\bin")
    ]
    env["PATH"] = os.pathsep.join(parts)

    log("[1/4] 编译 release …")
    # -j 1：本机 8 GB 内存，release 开着 thin LTO，并行两个 codegen 单元会
    # 直接吃爆内存（表现为 rustc 报 `memory allocation failed` 或 0xc0000409）。
    r = subprocess.run(
        ["cargo", "build", "--release", "-p", "vca-cli", "-j", "1"],
        cwd=str(ROOT),
        env=env,
    )
    if r.returncode != 0:
        raise SystemExit("编译失败")
    exe = ROOT / "target" / "release" / "vca.exe"
    if not exe.is_file():
        raise SystemExit(f"没有找到 {exe}")
    return exe


def copy_tree(src: Path, dst: Path, pattern: str = "*") -> int:
    """把 src 下匹配的文件复制到 dst（扁平），返回文件数。"""
    if not src.is_dir():
        return 0
    dst.mkdir(parents=True, exist_ok=True)
    n = 0
    for f in src.rglob(pattern):
        if f.is_file():
            shutil.copy2(f, dst / f.name)
            n += 1
    return n


LAUNCH_CMD = """@echo off
REM VibeClassAgent - 双击启动交互界面
REM 说明：内容保持 ASCII，cmd.exe 按控制台代码页解析，非 ASCII 注释会破坏解析。
chcp 65001 >nul 2>&1
cd /d "%~dp0"
vca.exe %*
if errorlevel 1 pause
"""

SETUP_CMD = """@echo off
REM VibeClassAgent - 首次设置：环境自检 + 自动补齐目录与配置
chcp 65001 >nul 2>&1
cd /d "%~dp0"
echo ============================================================
echo   VibeClassAgent  FIRST-TIME SETUP
echo   This will check the environment and create missing files.
echo ============================================================
echo.
vca.exe doctor --fix
echo.
echo ------------------------------------------------------------
echo   Next: edit config\\profiles\\default\\settings.yaml
echo   (model API, push channel, class schedule)
echo ------------------------------------------------------------
pause
"""

README_TXT = """VibeClassAgent —— 静默课堂录制与课后总结
========================================

怎么开始
--------
1. 双击「启动.cmd」—— 会打开一个**图形界面窗口**（没有地址栏，观感上就是个原生程序）。
2. 在界面里依次配好：模型 API → 推送 → 时间表 → 课表。
   每页都有「保存」；推送页还有「发送测试消息」，配完立刻能验证通不通。
   想推到 QQ 群：推送页下半部分那张「QQ 机器人（NapCat）」卡片能一键装好机器人
   （不用自己去 GitHub 翻 Release），首次启动扫码登录即可。
   嫌麻烦就用企业微信机器人 —— 只要一个 Webhook 地址，什么都不用装。
3. 配完之后挂着就行：上课时段自动录，午休与晚餐时段自动处理并推送。

> 想用命令行：`vca.exe chat` 进交互式 CLI，`vca.exe --help` 看全部子命令。
> CLI 功能完整保留，只是 GUI 更适合日常使用。

界面窗口与后台服务
------------------
界面是程序在本机回环地址（127.0.0.1）上起的一个小服务，只有本机能访问，
而且带一次性随机令牌。**直接关掉窗口不会退出后台服务** ——
要彻底退出请点界面右上角的「退出」按钮。

数据放在哪
----------
程序**不会**把数据放到 C 盘用户目录。默认就在本文件夹里：

    data\\      录像、音频、截图、生成的文档、作业记录
    config\\    配置

所以整个文件夹拷走就能换一台机器继续用。
想换位置的话：

    vca.exe --data-dir D:\\课堂数据 --config-dir D:\\课堂配置

常用命令
--------
    启动.cmd                打开图形界面（推荐）
    vca.exe doctor          环境自检
    vca.exe doctor --fix    自检并自动生成目录与配置
    vca.exe chat            交互式命令行界面
    vca.exe debug e2e       端到端冒烟测试（录 15 秒跑完整流程）
    vca.exe run             挂后台按课表自动录制与处理
    vca.exe --help          全部子命令

运行环境
--------
* Windows 10 及以上（64 位）。程序本身只依赖系统自带的 DLL；
  whisper 需要的 VC++ 运行库已经放在 tools\\whisper\\ 里，**不必另外安装**。
* 录屏必须**在登录后的桌面会话里运行**（不能做成系统服务启动），
  否则采集不到画面 —— 这也是它被设计成"双击启动"的原因。
* 如果录不到麦克风，去「设置 → 隐私和安全性 → 麦克风」里确认
  已允许桌面应用访问。

提醒
----
* 录屏与音频**不会上传**。只有转写成文字之后，才会发送给你自己配置的模型。
* 录像在推送成功 72 小时后才会被删除；推送一直失败就一直留着。
* 浏览器模式（用网页版聊天机器人做要点提取）属于实验特性，
  默认关闭，且可能违反第三方服务条款，请自行评估。
"""


NAPCAT_NOTE = """这个目录是给 QQ 机器人（NapCat）留的。

界面「推送」页那张「QQ 机器人（NapCat）」卡片可以一键下载安装，
装好之后就落在本目录里。

自动下载失败时（校园网访问 GitHub 常常不通）手动做也可以：

  1. 打开 https://github.com/NapNeko/NapCatQQ/releases
  2. 下载 NapCat.Shell.zip（本机已装 QQ 的话）
     或便携包版本（自带 QQ，不依赖本机安装）
  3. 解压到本目录 —— 让 launcher.bat 或 NapCatWinBootMain.exe
     直接躺在本目录下。多套一层同名子目录也没关系，程序会往下找一层
  4. 回到界面点「刷新」，卡片会认出它

不用 QQ 的话，这一步完全可以跳过：
企业微信机器人只需要一个 Webhook 地址，不装任何东西。
"""


def main() -> int:
    ap = argparse.ArgumentParser(description="打包 VibeClassAgent 发布目录")
    ap.add_argument("--out", default=str(ROOT / "dist"), help="输出根目录")
    ap.add_argument("--no-model", action="store_true", help="不打进 whisper 模型")
    ap.add_argument("--skip-build", action="store_true", help="跳过编译（复用现有产物）")
    args = ap.parse_args()

    exe = ROOT / "target" / "release" / "vca.exe"
    if not args.skip_build:
        exe = run_cargo_release()
    elif not exe.is_file():
        raise SystemExit("没有现成的 release 产物，去掉 --skip-build 重新打包")

    out = Path(args.out) / APP_NAME
    if out.exists():
        log(f"[2/4] 清理旧的 {out}")
        shutil.rmtree(out, ignore_errors=True)
    out.mkdir(parents=True, exist_ok=True)

    # ---- 主程序与启动脚本 ----
    shutil.copy2(exe, out / "vca.exe")
    (out / "启动.cmd").write_text(LAUNCH_CMD, encoding="utf-8")
    (out / "首次设置.cmd").write_text(SETUP_CMD, encoding="utf-8")
    (out / "README.txt").write_text(README_TXT, encoding="utf-8")

    # 机器人目录只留位置与说明，不打进实际文件（理由见 NAPCAT_NOTE）
    napcat = out / "tools" / "napcat"
    napcat.mkdir(parents=True, exist_ok=True)
    (napcat / "先看这个.txt").write_text(NAPCAT_NOTE, encoding="utf-8")
    log("已留出 tools\\napcat\\（QQ 机器人，按需下载，包内不含本体）")

    # ---- 资源与语言文件 ----
    # 图标是独立文件：拿到正式 LOGO 后直接覆盖 assets/icon.ico 即可，代码不用动。
    # 语言文件同理：改文案不必重新编译。
    n_icon = copy_tree(ROOT / "assets", out / "assets", "*.ico")
    n_locale = copy_tree(ROOT / "locales", out / "locales", "*.json")
    log(f"      图标 {n_icon} 个 / 语言文件 {n_locale} 个")

    # ---- 运行时组件 ----
    log("[3/4] 收集运行时组件")
    ff = copy_tree(ROOT / "tools" / "ffmpeg", out / "tools" / "ffmpeg", "ffmpeg.exe")
    wh = copy_tree(ROOT / "tools" / "whisper", out / "tools" / "whisper", "*.exe")
    wh += copy_tree(ROOT / "tools" / "whisper", out / "tools" / "whisper", "*.dll")
    md = 0
    if not args.no_model:
        md = copy_tree(
            ROOT / "tools" / "whisper" / "models",
            out / "tools" / "whisper" / "models",
            "*.bin",
        )
    log(f"      ffmpeg {ff} 个 / whisper {wh} 个 / 模型 {md} 个")

    # ---- VC++ 运行库 ----
    # whisper.cpp 是 MSVC 编译的，依赖 MSVCP140 / VCRUNTIME140 / VCOMP140 这几个 DLL。
    # 一体机上未必装过 Visual C++ Redistributable，缺了它本地转写会直接失败
    # （报「找不到 MSVCP140.dll」），而且往往是在课后处理时才炸 —— 那时课早就录完了。
    # 这几个是微软允许随应用再发行的组件，放进 exe 同目录即可被优先加载，用户不必再装。
    sys32 = Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32"
    vc_names = [
        "MSVCP140.dll",
        "MSVCP140_1.dll",
        "MSVCP140_2.dll",
        "VCRUNTIME140.dll",
        "VCRUNTIME140_1.dll",
        "VCOMP140.DLL",
        "CONCRT140.dll",
    ]
    vc_dir = out / "tools" / "whisper"
    vc_dir.mkdir(parents=True, exist_ok=True)
    got = 0
    for name in vc_names:
        p = sys32 / name
        if p.is_file():
            shutil.copy2(p, vc_dir / name)
            got += 1
    log(f"      VC++ 运行库 {got}/{len(vc_names)} 个")
    if got < 4:
        log("      警告：VC++ 运行库不全，目标机可能需要另装 Visual C++ Redistributable")

    # ---- 配置模板 ----
    # 放 `config/` 根下，而不是 profiles/default/：程序找不到课表时会按
    # `config/schedule.example.yaml` 这条路径回退，放错地方等于没放。
    # （模板本身也编在 exe 里，这里放一份是为了让用户能直接看到内容照着改。）
    cfg_dst = out / "config"
    cfg_dst.mkdir(parents=True, exist_ok=True)
    copied = 0
    src_cfg = ROOT / "config"
    for name in ["settings.example.yaml", "timetable.example.yaml", "schedule.example.yaml"]:
        p = src_cfg / name
        if p.is_file():
            shutil.copy2(p, cfg_dst / name)
            copied += 1
    if copied == 0:
        log("      警告：没有找到配置模板（config/*.example.yaml）")
    # 真正生效的配置在 profiles/default/ 下，由「首次设置」生成。
    (out / "config" / "profiles" / "default").mkdir(parents=True, exist_ok=True)

    # ---- 汇总 ----
    total = sum(f.stat().st_size for f in out.rglob("*") if f.is_file())
    n = sum(1 for f in out.rglob("*") if f.is_file())
    log(f"[4/4] 完成：{out}")
    log(f"      {n} 个文件，合计 {total / 1048576:.1f} MB")
    log("")
    log("把整个文件夹拷到一体机上，双击「首次设置.cmd」即可。")
    log("数据默认落在该文件夹的 data\\ 下，不碰 C 盘用户目录。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
