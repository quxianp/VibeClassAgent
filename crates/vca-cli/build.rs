//! 构建后把 `WebView2Loader.dll` 拷到 exe 旁边。
//!
//! # 为什么必须做这件事
//!
//! `wry` 在 Windows 上是**动态链接** `WebView2Loader.dll` 的。
//! 少了这个 DLL，程序**连 main 都进不去** —— 加载器直接报
//! `STATUS_DLL_NOT_FOUND (0xC0000135)`，双击闪一下就没，没有任何日志。
//! 这是最难查的一类故障：看起来像"程序坏了"，其实是少了一个文件。
//!
//! 所以让 cargo 在链接后自动把它放到 exe 旁边，用户拷贝
//! `target/release/` 下的东西时不会漏。
//!
//! # 为什么不是静态链接
//!
//! `webview2-com-sys` 提供了 `WebView2LoaderStatic.lib`（10 MB），
//! 静态链进去可以让 exe 不依赖这个 DLL。但那样 exe 会大 10 MB，
//! 而我们更希望 exe 小、把 DLL 放在旁边（打包脚本本来就是整目录拷）。
//! 如果哪天要发布单文件，把 `webview2-com` 的 `static` feature 打开即可。

use std::path::{Path, PathBuf};

fn main() {
    // 构建脚本在 cargo 里跑，只关心目标平台是不是 Windows。
    // 非 Windows 上什么都不做（wry 在那边用系统 WebKit，不需要这个）。
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    // 只在**最终可执行文件**的 crate 上动手。
    //
    // 为什么不是每个 crate 都做：构建脚本会为库里每个 crate 各跑一遍，
    // 那样会把同一个 DLL 拷很多次，而且路径各不相同。这里只认
    // `vca-cli` —— 它就是那个产出 `vca.exe` 的 crate。
    let pkg = std::env::var("CARGO_PKG_NAME").unwrap_or_default();
    if pkg != "vca-cli" {
        return;
    }

    println!("cargo:rerun-if-changed=build.rs");

    let Some(loader) = find_webview2_loader() else {
        // 找不到就打警告，**不要让构建失败** ——
        // 这样在没有网络/缓存的机器上还能编译（只是运行时要自己补 DLL）。
        println!(
            "cargo:warning=没找到 WebView2Loader.dll，界面将无法启动。\
             请确认 webview2-com-sys 已编译过。"
        );
        return;
    };

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR 未设置"));

    // 目标目录：优先用 CARGO_TARGET_DIR，否则从 OUT_DIR 反推。
    //
    // 不能只按固定层级数上溯 —— 本项目设了 `build-dir`（为了躲开 C 盘空间
    // 不足），OUT_DIR 的层级和默认布局不一样，数层数必然错。
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .ok()
        .filter(|p| p.is_dir())
        .or_else(|| infer_exe_dir(&out_dir));

    let Some(target_dir) = target_dir else {
        println!("cargo:warning=推导输出目录失败，跳过拷贝 WebView2Loader.dll");
        return;
    };

    // profile 名：从 OUT_DIR 里找 "release"/"debug" 这一段。
    //
    // 注意不能只看有没有 release —— 依赖里可能同时出现两个词。
    // 取"最后一个出现的"，它才是离 OUT_DIR 最近的那层 profile。
    let profile = {
        let parts: Vec<String> = out_dir
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
            .collect();
        if parts.iter().any(|p| p == "release") {
            "release"
        } else {
            "debug"
        }
    };

    let dest_dir = target_dir.join(profile);
    if let Err(e) = std::fs::create_dir_all(&dest_dir) {
        println!("cargo:warning=创建 {} 失败：{e}", dest_dir.display());
        return;
    }

    let dest = dest_dir.join("WebView2Loader.dll");
    match std::fs::copy(&loader, &dest) {
        Ok(_) => println!(
            "cargo:warning=已拷贝 WebView2Loader.dll -> {}",
            dest.display()
        ),
        Err(e) => println!("cargo:warning=拷贝 WebView2Loader.dll 失败：{e}"),
    }
}

/// 从 OUT_DIR 反推"exe 应该放的那个目录"（CARGO_TARGET_DIR 没设时的兜底）。
///
/// 判据是**这个目录里已经有 vca.exe**：那就是 rustc 最终输出可执行文件的地方，
/// DLL 也必须放那儿。比按目录层级数（`build-dir` 会改变层数）可靠得多。
///
/// 找不到 exe 时退回"最深一个含 `build/` 的子目录"—— 对第一次构建
/// （exe 还没生成）也算合理猜测。
fn infer_exe_dir(out_dir: &Path) -> Option<PathBuf> {
    // 先找已经有 vca.exe 的目录
    for ancestor in out_dir.ancestors().take(10) {
        if ancestor.join("vca.exe").is_file() {
            return Some(ancestor.to_path_buf());
        }
        // 有时 exe 在兄弟目录（build-dir 把中间产物和产物分开了）
        if let Some(parent) = ancestor.parent() {
            if parent.join("vca.exe").is_file() {
                return Some(parent.to_path_buf());
            }
        }
    }
    // 兜底：最深一个含 build/ 且像 target 的目录
    let mut best = None;
    for ancestor in out_dir.ancestors().take(10) {
        if ancestor.join("build").is_dir() {
            best = Some(ancestor.to_path_buf());
        }
    }
    best
}

/// 在 target/ 里找 x64 的 `WebView2Loader.dll`。
///
/// # 为什么是"爬着找"而不是写死路径
///
/// 两个原因都会让写死的路径失效：
/// 1. 那串 `webview2-com-sys-<hash>` 的 hash 随依赖图变化；
/// 2. 本项目的 `target/` 布局本身就不标准 —— `cargo/config.toml` 里设了
///    `build-dir`（为了躲开 C 盘空间不足），于是构建脚本的 OUT_DIR 是
///    `target/build/<profile>/build/...` 而不是默认的
///    `target/<profile>/build/...`。
///
/// 所以做法是：从 OUT_DIR 往上找到 `target/` 附近，再往下递归找
/// `x64/WebView2Loader.dll`。有深度上限，不会在巨大的 target/ 里爬太久。
fn find_webview2_loader() -> Option<PathBuf> {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").ok()?);

    // 从 OUT_DIR 逐级上溯，每个祖先都试着找一遍。
    // 上溯到盘根为止太多，限制 8 层足够覆盖各种布局。
    for (depth, ancestor) in out_dir.ancestors().take(8).enumerate() {
        // 优先找和当前 profile 一致的（release 别去拿 debug 的），
        // 但找不到就放宽 —— 内容其实是一样的，能用比"精确"重要。
        if let Some(found) = scan_for_loader(&ancestor.join("build"), 5) {
            return Some(found);
        }
        let _ = depth;
    }
    None
}

/// 递归找一个 `x64/WebView2Loader.dll`。
fn scan_for_loader(dir: &Path, depth: usize) -> Option<PathBuf> {
    if depth == 0 || !dir.is_dir() {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        if entry.file_name().to_string_lossy() == "x64" {
            let dll = p.join("WebView2Loader.dll");
            if dll.is_file() {
                return Some(dll);
            }
        }
        subdirs.push(p);
    }
    // 先广度后深度意义不大，直接递归下去即可
    for sub in subdirs {
        if let Some(found) = scan_for_loader(&sub, depth - 1) {
            return Some(found);
        }
    }
    None
}
