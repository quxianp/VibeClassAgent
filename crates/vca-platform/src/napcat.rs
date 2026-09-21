//! NapCat（QQ 机器人）的本地安装与进程管理。
//!
//! # 这块要解决什么
//!
//! 「课后文档推到 QQ 群」这条链路上，最劝退的一步不是填配置，而是**装机器人**：
//! 用户得先知道 OneBot 11 是什么、去 GitHub 翻 Release、解压、
//! 再对着文档拼一个启动脚本。本模块把这几步收进程序里——
//! 界面上点一下就能装、能起、能停、能看日志（含扫码登录用的二维码）。
//!
//! # 为什么是「管理」而不是「打包进安装包」
//!
//! 三个理由，任何一个都足够：
//!
//! 1. NapCat 是 **QQNT 的注入式插件**，本机没装 QQ 时它一行也跑不起来，
//!    打进来只是多几十 MB 死重量；
//! 2. 它有自己的许可证与更新节奏，把别人的二进制塞进我们的发行包并不合适；
//! 3. 机器人协议实现迭代很快，跟着上游走比跟着我们发版走对用户更有利。
//!
//! # 目录约定
//!
//! 装在 `<tools>/napcat/`，与 ffmpeg、whisper 一个待遇——都在
//! [`crate::proc::tool_dirs`] 能找到的位置。**绝不往 `C:` 写**：
//! 目标机器是教室一体机，系统盘常被还原卡管着，重启就没了。
//!
//! # 能识别哪些形态
//!
//! | 形态 | 判据 | 能不能直接启动 |
//! |---|---|---|
//! | Shell | 有 `launcher.bat` / `NapCatWinBootMain.exe` | 能，但需要本机装好 QQ |
//! | 便携包 | 同 Shell，且目录里带 `QQ.exe` | 能，**自带 QQ，不依赖本机安装** |
//! | Framework | 只有 `napcat.mjs` / `package.json` | 不能，必须由 QQ 本体加载 |
//!
//! 入口文件名是**上游改过好几次**的东西（早期 `NapCatWinBootMain.exe`，
//! 后来换成 `launcher.bat` 包一层环境准备），所以这里按候选列表逐个试，
//! 而不是写死一个。都找不到时返回 `None`，由界面提示用户手动指定目录。

use std::fs::File;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

use crate::proc;

/// 启动入口的候选文件名，按优先级排列。
///
/// `.mjs` 也在列表里，但它是 node 脚本、必须由 QQ 本体加载，
/// 所以单独归到 [`Flavor::Framework`]，[`start`] 会明确拒绝执行它。
pub const ENTRY_CANDIDATES: &[&str] = &[
    "NapCatWinBootMain.exe",
    "launcher.bat",
    "launcher-win10.bat",
    "start.bat",
    "napcat.mjs",
];

/// 官方下载源候选（按顺序试，第一个成功的就用）。
///
/// 第一项是 GitHub 官方的固定别名地址——`releases/latest/download/<资产名>`
/// 会 302 到当前最新版，不用我们维护版本号。
///
/// 后两项是社区反代（路径与官方完全一致，只是加了前缀）。**校园网访问
/// GitHub 经常不稳**，而目标用户正是学校，所以留了兜底。这些镜像属于
/// 第三方服务，可用性随时会变；全试失败时程序会退回「手动下载」指引，
/// 不会假装成功。
pub const SOURCES: &[&str] = &[
    "https://github.com/NapNeko/NapCatQQ/releases/latest/download/NapCat.Shell.zip",
    "https://ghfast.top/https://github.com/NapNeko/NapCatQQ/releases/latest/download/NapCat.Shell.zip",
    "https://gh-proxy.com/https://github.com/NapNeko/NapCatQQ/releases/latest/download/NapCat.Shell.zip",
];

/// 下载源清单：(给人看的标签, 地址)。
///
/// 给界面做下拉用。标签写清「官方 / 镜像」，因为校园网多半通不了官方 ——
/// 让用户自己挑比让程序逐个超时试要快得多。
pub fn sources() -> Vec<(&'static str, &'static str)> {
    vec![
        ("官方 GitHub", SOURCES[0]),
        ("镜像 ghfast.top", SOURCES[1]),
        ("镜像 gh-proxy.com", SOURCES[2]),
    ]
}

/// 下载页（自动下载全失败时给用户手动走）。
pub const DOWNLOAD_PAGE: &str = "https://github.com/NapNeko/NapCatQQ/releases";

/// 一个装好的 NapCat。
#[derive(Debug, Clone)]
pub struct NapCat {
    /// 安装根目录（`<tools>/napcat`，可能已被压平一层）。
    pub root: PathBuf,
    /// 实际可执行的入口文件。
    pub entry: PathBuf,
    /// 形态。
    pub flavor: Flavor,
    /// 版本号（尽力从 `package.json` 读，读不到是 `None`）。
    pub version: Option<String>,
}

/// NapCat 的安装形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    /// Shell 包：需要本机已安装 QQ。
    Shell,
    /// 便携包：自带 QQ，解压即用。
    Portable,
    /// Framework：只是 QQ 的插件，必须由 QQ 本体加载。
    Framework,
}

impl Flavor {
    /// 给界面看的中文名。
    pub fn label(self) -> &'static str {
        match self {
            Self::Shell => "Shell（需要本机已装 QQ）",
            Self::Portable => "便携包（自带 QQ）",
            Self::Framework => "Framework（需由 QQ 本体加载）",
        }
    }

    /// 能不能被 [`start`] 直接拉起来。
    pub fn is_launchable(self) -> bool {
        matches!(self, Self::Shell | Self::Portable)
    }
}

/// 默认安装目录：第一个可用的 tools 目录下的 `napcat/`。
///
/// 找不到任何 tools 目录（比如程序被单独拷走）时退回当前目录，
/// 宁可装到工作目录下，也不要写到 `C:`。
pub fn default_root() -> PathBuf {
    // 优先挑**已经存在**的 tools 目录。发布形态下 exe 旁边就有 tools/；
    // 开发形态下 exe 在 target/debug/、真正常用的 tools/ 在仓库根。
    // 只取第一个候选的话，开发时会把安装目录指到 target/debug/tools/napcat ——
    // 用户手动放进去的机器人不在那儿，界面上就成「明明放了却说没装」。
    let mut dirs = proc::tool_dirs();
    let base = match dirs.iter().position(|d| d.is_dir()) {
        Some(i) => dirs.remove(i),
        None => dirs
            .into_iter()
            .next()
            .unwrap_or_else(|| PathBuf::from("tools")),
    };
    base.join("napcat")
}

/// 运行时用的安装目录：环境变量 `VCA_NAPCAT_DIR` 优先。
///
/// 留这个口子是为了两件事：同一台机器上跑两套互不干扰（多实例），
/// 以及**测试**——冒烟测试必须把安装目录指到临时目录去，不能碰真实的 `tools/`。
pub fn root() -> PathBuf {
    std::env::var("VCA_NAPCAT_DIR")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(default_root)
}

/// 在给定目录里识别一个已装好的 NapCat。
///
/// 返回 `None` 表示「这个目录不是 NapCat 的安装目录」——
/// 包括目录不存在、目录是空的、只有几个零散文件的情况。
pub fn detect(root: &Path) -> Option<NapCat> {
    if !root.is_dir() {
        return None;
    }
    let (root, entry) = find_entry(root).or_else(|| {
        // 本层找不到入口，就往下找**一层**：上游的 zip 解出来常常多套一层
        // 同名目录（`napcat/NapCat.Shell/launcher.bat`），而且那一层里
        // 往往还躺着一个 readme —— 只按「唯一子目录」压平会漏掉这种情况。
        // 只钻一层、只认入口文件：万一用户把目录指错了，
        // 递归扫描会把整块硬盘翻一遍。
        std::fs::read_dir(root)
            .ok()?
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .take(8)
            .find_map(|e| find_entry(&e.path()))
    })?;

    let is_mjs = entry
        .extension()
        .map(|e| e.eq_ignore_ascii_case("mjs"))
        .unwrap_or(false);
    let flavor = if root.join("QQ.exe").is_file() {
        Flavor::Portable
    } else if is_mjs {
        Flavor::Framework
    } else {
        Flavor::Shell
    };

    Some(NapCat {
        version: read_version(&root),
        root,
        entry,
        flavor,
    })
}

/// 依次在多个候选目录里找，返回第一个命中的。
pub fn detect_in(dirs: &[PathBuf]) -> Option<NapCat> {
    dirs.iter().find_map(|d| detect(d))
}

/// 在某一层目录里按候选顺序找入口，返回（入口所在目录, 入口文件）。
fn find_entry(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    ENTRY_CANDIDATES
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.is_file())
        .map(|p| (dir.to_path_buf(), p))
}

/// 从 `package.json` 里读版本号。读不到就是 `None`（不猜）。
fn read_version(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("package.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("version")
        .and_then(|x| x.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 启动器自己的日志文件。
///
/// 为什么要有：NapCat 首次登录会把**登录二维码**打在控制台上。
/// 若按常规做法把子进程输出丢进 `nul`，用户就永远扫不到码，
/// 只会看到一个「启动了但一直连不上」的黑箱。落成文件之后，
/// 界面就能把这段（含二维码的字符画）直接显示出来。
pub fn launcher_log(nc: &NapCat) -> PathBuf {
    launcher_log_at(&nc.root)
}

/// 同上，但只按根目录算——还没识别出安装时（比如刚装完一半、目录里
/// 还没有可执行入口）也要能读到日志，否则用户根本看不到失败原因。
pub fn launcher_log_at(root: &Path) -> PathBuf {
    root.join("logs").join("vca-launcher.log")
}

/// 启动 NapCat，返回进程 id。
///
/// 进程是**故意不等待**的：它要一直挂在后台，随课堂录制一起存活。
pub fn start(nc: &NapCat) -> Result<u32> {
    if !nc.flavor.is_launchable() {
        return Err(anyhow!(
            "这个目录是 {}，不能被直接启动。\n\
             它需要由 QQ 本体加载：请用 NapCat.Shell 或便携包重新安装。",
            nc.flavor.label()
        ));
    }

    // .bat 必须经 cmd.exe 才能拉起来，直接 CreateProcess 会失败
    let is_bat = nc
        .entry
        .extension()
        .map(|e| e.eq_ignore_ascii_case("bat") || e.eq_ignore_ascii_case("cmd"))
        .unwrap_or(false);

    let mut cmd = if is_bat {
        let mut c = proc::silent("cmd.exe");
        c.arg("/c").arg(&nc.entry);
        c
    } else {
        proc::silent(&nc.entry)
    };
    cmd.current_dir(&nc.root);

    // 输出重定向到日志文件（二维码就在里面），而不是丢进 nul
    let log = launcher_log(nc);
    if let Some(d) = log.parent() {
        std::fs::create_dir_all(d).ok();
    }
    if let Ok(f) = File::create(&log) {
        if let Ok(f2) = f.try_clone() {
            cmd.stdout(Stdio::from(f2));
            cmd.stderr(Stdio::from(f));
        }
    }

    let child = cmd
        .spawn()
        .with_context(|| format!("启动 {} 失败", nc.entry.display()))?;
    let pid = child.id();
    // 这里**故意不 wait**：NapCat 要一直挂在后台。
    // 顺带说明一个容易误解的点——把 Child drop 掉并不会结束进程，
    // 它只关掉我们这边的句柄，进程照常活着（Windows 语义）。
    drop(child);
    Ok(pid)
}

/// 结束一个进程。
///
/// 语义是「确保它没了」：本来就不在跑也算成功，否则界面上的「停止」
/// 会对着一个已经退出的机器人报错，看着莫名其妙。
///
/// 用 `TerminateProcess` 而不是 `taskkill`：后者和 `tasklist` 一样在受限
/// 环境里会被拒（实测），而且它本身也是个子进程，多一层没必要的开销。
#[cfg(windows)]
pub fn stop(pid: u32) -> Result<()> {
    use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

    if !is_alive(pid) {
        return Ok(());
    }
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, false, pid)
            .map_err(|e| anyhow!("打开进程 {pid} 失败（可能需要管理员权限）：{e}"))?;
        let r = TerminateProcess(h, 1);
        let _ = windows::Win32::Foundation::CloseHandle(h);
        r.map_err(|e| anyhow!("结束进程 {pid} 失败：{e}"))?;
    }
    Ok(())
}

/// 非 Windows 平台：本项目只跑 Windows，这里给个保守实现让别处能编过。
#[cfg(not(windows))]
pub fn stop(_pid: u32) -> Result<()> {
    Ok(())
}

/// 进程是否还活着。
///
/// 一开始用的是 `tasklist`（不用碰 unsafe），但实测在受限环境里它直接回
/// `ERROR: Access denied` —— 连「我自己这个进程活着吗」都答不对，
/// 而且每问一次就要起一个进程，界面刷新状态时纯属浪费。
/// 改成 Win32 一次调用：`PROCESS_QUERY_LIMITED_INFORMATION` 是权限要求
/// 最低的查询权限，不需要提权。
#[cfg(windows)]
pub fn is_alive(pid: u32) -> bool {
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            // 打不开通常就是「没这个进程」；也可能是权限不够，但那种情况
            // 我们本来也管不了，当作不在跑更安全（不会去重复启动 / 误杀）。
            return false;
        };
        let mut code: u32 = 0;
        let ok = GetExitCodeProcess(h, &mut code).is_ok();
        let _ = windows::Win32::Foundation::CloseHandle(h);
        // 259 = STILL_ACTIVE。这里写数字而不是引常量，是为了不依赖
        // windows crate 里那个常量到底是什么整数类型（改版换过）。
        ok && code == 259
    }
}

/// 非 Windows 平台：本项目只跑 Windows，这里给个保守值让别处能编过。
#[cfg(not(windows))]
pub fn is_alive(_pid: u32) -> bool {
    false
}

/// 本机某个端口上有没有东西在监听（用来判断 OneBot 的 HTTP 服务起没起）。
pub fn port_open(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(400)).is_ok()
}

/// 把 zip 解压到 `dest`，返回解压出的文件数。
///
/// 走 `enclosed_name()` 而不是直接拼路径：那是防 Zip Slip 的官方做法，
/// 恶意/损坏的压缩包用 `..\..\Windows\...` 这样的条目就能写到目录外。
pub fn extract_zip(zip: &Path, dest: &Path) -> Result<usize> {
    let file = File::open(zip).with_context(|| format!("打开 {} 失败", zip.display()))?;
    let mut ar = zip::ZipArchive::new(file).context("这个文件不是有效的 zip")?;
    std::fs::create_dir_all(dest)?;

    let mut n = 0usize;
    for i in 0..ar.len() {
        let mut entry = ar.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else {
            continue; // 越界条目：跳过，不报错也不落盘
        };
        let out = dest.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(d) = out.parent() {
            std::fs::create_dir_all(d)?;
        }
        let mut w = File::create(&out).with_context(|| format!("写入 {} 失败", out.display()))?;
        std::io::copy(&mut entry, &mut w)?;
        n += 1;
    }
    Ok(n)
}

/// 从候选源里挑一个能用的，下载到 `dest_zip`。
///
/// 返回实际使用的地址，方便把「用了镜像」这件事如实告诉用户。
pub fn download_shell_zip(dest_zip: &Path, timeout_ms: i32, only: Option<usize>) -> Result<String> {
    if let Some(d) = dest_zip.parent() {
        std::fs::create_dir_all(d)?;
    }
    // `only = Some(i)` 表示用户指定了某一个源；None 就是依次试。
    let picked: Vec<&str> = match only {
        Some(i) => SOURCES.get(i).copied().into_iter().collect(),
        None => SOURCES.to_vec(),
    };
    if picked.is_empty() {
        return Err(anyhow!("没有第 {only:?} 个下载源"));
    }

    let mut errs = Vec::new();
    for url in picked {
        match crate::http::download(url, dest_zip, timeout_ms) {
            Ok(n) if n > 64 * 1024 => return Ok(url.to_string()),
            Ok(n) => errs.push(format!("{url} → 只下到 {n} 字节，不像安装包")),
            Err(e) => errs.push(format!("{url} → {e}")),
        }
    }
    Err(anyhow!(
        "所有下载源都没成功：\n  {}\n\n\
         可以手动下载后放到安装目录里，程序会自动识别：\n  {DOWNLOAD_PAGE}",
        errs.join("\n  ")
    ))
}

/// 从 zip 装到 `root`，返回识别出的安装信息。
pub fn install_from_zip(zip: &Path, root: &Path) -> Result<NapCat> {
    if root.exists() {
        // 覆盖前先挪走旧的，避免半新半旧混在一起（上游改过目录结构）
        let bak = root.with_extension(format!("bak-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&bak);
        let _ = std::fs::rename(root, &bak);
    }
    std::fs::create_dir_all(root)?;
    let n = extract_zip(zip, root)?;
    if n == 0 {
        return Err(anyhow!("压缩包里没有任何文件"));
    }
    detect(root).ok_or_else(|| {
        anyhow!(
            "解压出了 {n} 个文件，但没找到启动入口（找过：{}）。\n\
             可能是上游改了包结构，请把 {} 的内容发给我们。",
            ENTRY_CANDIDATES.join(" / "),
            root.display()
        )
    })
}

/// 在系统临时目录下载一个 Shell 包（便于失败后排查，也不污染安装目录）。
pub fn temp_zip_path() -> PathBuf {
    std::env::temp_dir().join("vca-napcat-shell.zip")
}

/// 把旧的安装目录清理掉（安装失败回滚用，尽力而为）。
pub fn cleanup_backup(root: &Path) {
    let bak = root.with_extension(format!("bak-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(bak);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("vca-napcat-test-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn default_root_lives_under_a_tools_dir() {
        // 刻意**不**断言 tools/ 存在：cargo test 的 cwd 是 package 根，
        // 三个候选目录一个都不存在，这时会退回第一个候选。
        // 能稳定断言的是末两段路径。
        let r = default_root();
        assert_eq!(r.file_name().unwrap(), "napcat");
        assert_eq!(r.parent().unwrap().file_name().unwrap(), "tools");
    }

    #[test]
    fn detect_returns_none_for_missing_dir() {
        assert!(detect(Path::new("Z:\\definitely-not-here\\napcat")).is_none());
    }

    #[test]
    fn detect_returns_none_for_empty_dir() {
        let d = tmp("empty");
        assert!(detect(&d).is_none());
    }

    #[test]
    fn detect_finds_shell_by_launcher_bat() {
        let d = tmp("shell");
        std::fs::write(d.join("launcher.bat"), "@echo off\n").unwrap();
        let nc = detect(&d).expect("应当识别出 Shell");
        assert_eq!(nc.flavor, Flavor::Shell);
        assert!(nc.flavor.is_launchable());
        assert_eq!(nc.root, d);
    }

    #[test]
    fn detect_prefers_win_boot_main_over_bat() {
        // 两个入口同时在时，按候选顺序取前者（exe 比 bat 少一层 shell）
        let d = tmp("both");
        std::fs::write(d.join("launcher.bat"), "").unwrap();
        std::fs::write(d.join("NapCatWinBootMain.exe"), "").unwrap();
        let nc = detect(&d).unwrap();
        assert_eq!(nc.entry.file_name().unwrap(), "NapCatWinBootMain.exe");
    }

    #[test]
    fn detect_marks_portable_when_qq_exe_present() {
        let d = tmp("portable");
        std::fs::write(d.join("NapCatWinBootMain.exe"), "").unwrap();
        std::fs::write(d.join("QQ.exe"), "").unwrap();
        let nc = detect(&d).unwrap();
        assert_eq!(nc.flavor, Flavor::Portable);
    }

    #[test]
    fn detect_marks_framework_for_mjs_only() {
        let d = tmp("framework");
        std::fs::write(d.join("napcat.mjs"), "// noop\n").unwrap();
        let nc = detect(&d).unwrap();
        assert_eq!(nc.flavor, Flavor::Framework);
        assert!(!nc.flavor.is_launchable());
    }

    #[test]
    fn detect_flattens_single_nested_dir() {
        // 上游 zip 解出来常常是 napcat/NapCat.Shell/launcher.bat
        let d = tmp("nested");
        let inner = d.join("NapCat.Shell");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(inner.join("launcher.bat"), "").unwrap();
        let nc = detect(&d).expect("应当钻进去找到入口");
        assert_eq!(nc.root, inner);
    }

    #[test]
    fn detect_reaches_into_subdir_even_when_a_readme_is_beside_it() {
        // 上游的包里常常是「一个同名目录 + 一个 readme」。
        // 只按「唯一子目录」压平会漏掉这种情况：入口明明就在眼皮底下，
        // 却报「没找到启动入口」，用户一头雾水。
        let d = tmp("nested-with-file");
        let inner = d.join("NapCat.Shell");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(inner.join("launcher.bat"), "").unwrap();
        std::fs::write(d.join("readme.txt"), "x").unwrap();
        assert_eq!(detect(&d).unwrap().root, inner);
    }

    #[test]
    fn detect_prefers_the_outer_layer_when_both_have_entries() {
        // 两层都有入口时用外面那层：它是用户明确指过来的目录
        let d = tmp("both-layers");
        let inner = d.join("sub");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(d.join("launcher.bat"), "").unwrap();
        std::fs::write(inner.join("launcher.bat"), "").unwrap();
        assert_eq!(detect(&d).unwrap().root, d);
    }

    #[test]
    fn detect_gives_up_when_only_a_grandchild_has_the_entry() {
        // 只钻一层，不做递归：否则目录指错一层就会去遍历整棵树
        let d = tmp("grandchild");
        let deep = d.join("a").join("b");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("launcher.bat"), "").unwrap();
        assert!(detect(&d).is_none());
    }

    #[test]
    fn reads_version_from_package_json() {
        let d = tmp("version");
        std::fs::write(d.join("launcher.bat"), "").unwrap();
        std::fs::write(
            d.join("package.json"),
            r#"{"name":"napcat","version":"4.8.124"}"#,
        )
        .unwrap();
        assert_eq!(detect(&d).unwrap().version.as_deref(), Some("4.8.124"));
    }

    #[test]
    fn missing_version_is_none_not_guess() {
        let d = tmp("noversion");
        std::fs::write(d.join("launcher.bat"), "").unwrap();
        assert!(detect(&d).unwrap().version.is_none());
    }

    #[test]
    fn start_refuses_framework() {
        let d = tmp("refuse");
        std::fs::write(d.join("napcat.mjs"), "").unwrap();
        let nc = detect(&d).unwrap();
        let err = start(&nc).unwrap_err().to_string();
        assert!(err.contains("不能被直接启动"), "{err}");
    }

    #[test]
    fn port_open_detects_a_listening_socket() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(port_open(port));
    }

    #[test]
    fn port_open_false_for_closed_port() {
        // 绑一下就放掉，拿到一个「刚刚还是空的」端口号
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        assert!(!port_open(port));
    }

    #[test]
    fn is_alive_true_for_self() {
        assert!(is_alive(std::process::id()));
    }

    #[test]
    fn is_alive_false_for_absurd_pid() {
        // 4 这个 pid 在 Windows 上永远是 System 之外的保留值，不可能是我们的进程
        assert!(!is_alive(4_000_000_000));
    }

    #[test]
    fn extract_zip_roundtrip_and_flatten() {
        let d = tmp("zip");
        let zip_path = d.join("pack.zip");
        {
            let f = File::create(&zip_path).unwrap();
            let mut w = zip::ZipWriter::new(f);
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            w.add_directory("NapCat.Shell/", opts).unwrap();
            w.start_file("NapCat.Shell/launcher.bat", opts).unwrap();
            use std::io::Write;
            w.write_all(b"@echo off\n").unwrap();
            w.finish().unwrap();
        }

        let out = d.join("out");
        let n = extract_zip(&zip_path, &out).unwrap();
        assert_eq!(n, 1);
        assert!(out.join("NapCat.Shell").join("launcher.bat").is_file());

        let nc = install_from_zip(&zip_path, &out).unwrap();
        assert_eq!(nc.flavor, Flavor::Shell);
        assert!(nc.root.ends_with("NapCat.Shell"), "{:?}", nc.root);
    }

    #[test]
    fn install_from_zip_reports_unrecognized_pack() {
        let d = tmp("badzip");
        let zip_path = d.join("bad.zip");
        {
            let f = File::create(&zip_path).unwrap();
            let mut w = zip::ZipWriter::new(f);
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            w.start_file("random.txt", opts).unwrap();
            use std::io::Write;
            w.write_all(b"nothing useful").unwrap();
            w.finish().unwrap();
        }
        let err = install_from_zip(&zip_path, &d.join("dest"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("没找到启动入口"), "{err}");
    }
}
