//! 下载并补齐缺失的运行时依赖。
//!
//! # 适用范围
//!
//! 只负责 [`crate::deps`] 清单里那几件外部组件：ffmpeg、whisper.cpp、语音模型。
//! 判定交给 `deps`，本模块只管「把它下下来、解开、放到位」。
//!
//! # 几个绕不开的现实问题
//!
//! **1. 下载源会挂。** GitHub 在国内的教育网里经常连不上或极慢。
//! 所以每个组件都有多个源，**依次尝试**，某个源失败不影响其它组件。
//! 这条规矩来自 NapCat 那套（见 [`crate::napcat::download_shell_zip`]），
//! 已经证明在受限网络下有用。
//!
//! **2. 下载会断。** 125 MB 在一体机的网络环境下断一次很正常。
//! 所以：先下到**临时文件**，下完再改名就位。中途失败时删掉临时文件 ——
//! 绝不能留一个半截文件冒充成品，那会让下次启动的检测误判成"已就绪"，
//! 然后在真正调用时以难懂的方式炸掉。
//!
//! **3. 压缩包里通常多套一层目录。** 上游的 zip 解出来常常是
//! `whisper-bin-x64/whisper-cli.exe`，而我们只要里面的东西。
//! 所以解压后要**找一下**目标文件在哪一层（见 [`find_in_tree`]），
//! 而不是硬编码路径 —— 上游改一次目录名我们就会静默失效。
//!
//! **4. 进度要看得见。** 上百 MB 的下载如果界面上毫无动静，
//! 用户会以为程序卡死然后强杀。所以每个组件都通过回调汇报状态。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};

use crate::deps::{self, DepSpec, DepState};

/// 单项补齐过程中的状态，通过回调汇报给界面。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchEvent {
    /// 开始处理某一项。
    Start {
        /// 依赖 id。
        id: String,
        /// 面向用户的名称。
        label: String,
    },
    /// 正在尝试某个源（`index` 从 1 开始）。
    Trying {
        /// 依赖 id。
        id: String,
        /// 源地址。
        url: String,
        /// 第几个源。
        index: usize,
        /// 一共几个源。
        total: usize,
    },
    /// 下载完成，准备解压/就位。
    Downloaded {
        /// 依赖 id。
        id: String,
        /// 下载到的字节数。
        bytes: u64,
    },
    /// 正在解压。
    Extracting {
        /// 依赖 id。
        id: String,
    },
    /// 某一项成功就位。
    Done {
        /// 依赖 id。
        id: String,
        /// 落地的绝对路径。
        path: PathBuf,
        /// 最终字节数。
        bytes: u64,
    },
    /// 某一项失败（不影响其它项继续）。
    Failed {
        /// 依赖 id。
        id: String,
        /// 失败原因。
        error: String,
    },
    /// 全部结束。
    Finished {
        /// 成功项数。
        ok: usize,
        /// 失败项数。
        failed: usize,
    },
}

/// 汇报回调。实现要足够轻 —— 它在下载线程上被高频调用。
pub type ProgressFn = Box<dyn Fn(FetchEvent) + Send + Sync + 'static>;

/// 一个组件的下载源集合与目标。
struct Target {
    /// 对应的依赖声明。
    spec: &'static DepSpec,
    /// 候选下载地址（依次尝试）。
    urls: &'static [&'static str],
    /// 是否是 zip（需要解压）；false 表示直接就是目标文件。
    is_zip: bool,
    /// zip 解压后，要从中捞出来的文件名（如 `ffmpeg.exe`）。
    /// 为 `None` 且 `is_zip` 时表示直接解压到目标目录。
    pick_exe: Option<&'static str>,
}

/// 每个组件的下载来源。
///
/// 说明为什么 ffmpeg 用 gyan.dev 打头：它是 ffmpeg 官方推荐的 Windows
/// 构建源，且**直链稳定**（GitHub 的 release 直链会跳 302，在教育网里
/// 跳转那一步经常超时）。
const TARGETS: &[Target] = &[
    Target {
        spec: &deps::DEPS[0], // ffmpeg
        urls: &[
            "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip",
            "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip",
        ],
        is_zip: true,
        pick_exe: Some("ffmpeg.exe"),
    },
    Target {
        spec: &deps::DEPS[1], // whisper.cpp
        urls: &[
            "https://github.com/ggml-org/whisper.cpp/releases/download/v1.7.6/whisper-bin-x64.zip",
            // 镜像：GitHub 直链在受限网络里经常不通，前面加一层加速域名。
            "https://ghfast.top/https://github.com/ggml-org/whisper.cpp/releases/download/v1.7.6/whisper-bin-x64.zip",
        ],
        is_zip: true,
        pick_exe: Some("whisper-cli.exe"),
    },
    Target {
        spec: &deps::DEPS[2], // 模型
        urls: &[
            // hf-mirror 放前面：HuggingFace 主站在国内经常连不上。
            "https://hf-mirror.com/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
        ],
        is_zip: false,
        pick_exe: None,
    },
];

/// 单个文件的下载超时。模型有 75 MB，留足时间。
const FILE_TIMEOUT_MS: i32 = 30 * 60 * 1000;

/// 补齐指定项。
///
/// `only` 为 `None` 时补齐**全部缺失项**；否则只补指定的 id 们。
/// 空 `only` 与 `None` 语义不同：前者表示「什么也不补」，调用方按需区分。
///
/// 返回每项的最终状态。**本函数不抛错** —— 单项失败只记为失败，
/// 因为「补 ffmpeg 成功但补模型失败」是常见情况，不该整体回滚。
pub fn fetch_missing(
    tools_root: &Path,
    only: Option<&[String]>,
    progress: Option<ProgressFn>,
    cancel: Option<Arc<AtomicBool>>,
) -> Vec<DepState> {
    fetch_missing_with_mirrors(
        tools_root,
        only,
        progress,
        cancel,
        &vca_core::config::DownloadSettings::default(),
    )
}

/// 同 [`fetch_missing`]，但可以指定自定义下载源（镜像）。
///
/// # 为什么要单独一个函数而不是给上面加参数
///
/// `fetch_missing` 的调用点有好几处（接口、成品脚本、测试），
/// 加参数会把每一处都改一遍；而"没配镜像"是最常见的情况，
/// 让它保持原签名可以少碰很多代码。
///
/// 自定义源的合并规则在
/// [`vca_core::config::merge_sources`] —— 那里有完整的顺序说明和测试。
pub fn fetch_missing_with_mirrors(
    tools_root: &Path,
    only: Option<&[String]>,
    progress: Option<ProgressFn>,
    cancel: Option<Arc<AtomicBool>>,
    mirrors: &vca_core::config::DownloadSettings,
) -> Vec<DepState> {
    let ev = |e: FetchEvent| {
        if let Some(p) = &progress {
            p(e);
        }
    };

    std::fs::create_dir_all(tools_root).ok();

    let picked: Vec<&Target> = TARGETS
        .iter()
        .filter(|t| match only {
            None => true,
            Some(ids) => ids.iter().any(|i| i == t.spec.id),
        })
        .collect();

    let mut ok = 0usize;
    let mut failed = 0usize;

    for t in picked {
        if let Some(c) = &cancel {
            if c.load(Ordering::Relaxed) {
                break;
            }
        }

        // 已就绪的跳过：用户可能只缺一个，没必要把其它几个重下一遍。
        let before = deps::check_one(t.spec, tools_root);
        if before.ready {
            ok += 1;
            continue;
        }

        ev(FetchEvent::Start {
            id: t.spec.id.to_string(),
            label: t.spec.label.to_string(),
        });

        match fetch_one(t, tools_root, &ev, &cancel, mirrors) {
            Ok(path) => {
                let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                ev(FetchEvent::Done {
                    id: t.spec.id.to_string(),
                    path,
                    bytes,
                });
                ok += 1;
            }
            Err(e) => {
                ev(FetchEvent::Failed {
                    id: t.spec.id.to_string(),
                    error: e.to_string(),
                });
                failed += 1;
            }
        }
    }

    ev(FetchEvent::Finished { ok, failed });

    // 返回**重新检查**后的真实状态，而不是我们以为的状态：
    // 下载函数说自己成功了，但文件未必真的落在我们要的位置
    // （解压出来的目录结构可能和预期不同）。以检测结果为准。
    deps::check_all(tools_root)
}

/// 内置下载源清单（只读，给界面展示"不填会用什么"）。
///
/// 为什么要暴露出来：用户有权知道程序会去哪些地址下载东西，
/// 否则他没法判断自己该不该填镜像 —— 也不知道填了之后
/// "保底"的那几个是什么。这是透明度问题，不是功能问题。
pub fn builtin_urls() -> Vec<(&'static str, Vec<&'static str>)> {
    TARGETS
        .iter()
        .map(|t| (t.spec.id, t.urls.to_vec()))
        .collect()
}

/// 补齐一项。
fn fetch_one(
    t: &Target,
    tools_root: &Path,
    ev: &dyn Fn(FetchEvent),
    cancel: &Option<Arc<AtomicBool>>,
    mirrors: &vca_core::config::DownloadSettings,
) -> Result<PathBuf> {
    let spec = t.spec;
    let final_path = tools_root.join(spec.rel_path);

    // 临时目录：下在这里，成功后再就位。用固定的临时文件名（不是随机名）：
    // 上次中断留下的半截文件会在下一次下载时被覆盖，不会越积越多。
    let tmp_dir = tools_root.join("_fetch_tmp");
    std::fs::create_dir_all(&tmp_dir)?;
    let tmp_file = tmp_dir.join(if t.is_zip {
        format!("{}.zip", spec.id)
    } else {
        format!("{}.part", spec.id)
    });

    if let Some(d) = final_path.parent() {
        std::fs::create_dir_all(d)?;
    }

    // ---- 逐个源尝试 ----
    //
    // 这里用**合并后**的源列表，而不是 `t.urls` 本身：
    // 用户填的镜像排在最前面（他最清楚哪个通），内置地址保底。
    // 合并规则见 `vca_core::config::merge_sources`。
    let sources = vca_core::config::merge_sources(spec.id, t.urls, mirrors);
    if sources.is_empty() {
        return Err(anyhow!(
            "没有可用下载源：已启用「只用自定义源」，但 {} 没有合法地址。\
             请在下载源设置里填写 http(s) 地址，或关闭该选项",
            spec.label
        ));
    }
    let mut errs: Vec<String> = Vec::new();
    let mut got = false;
    for (i, url) in sources.iter().enumerate() {
        if let Some(c) = cancel {
            if c.load(Ordering::Relaxed) {
                return Err(anyhow!("用户取消"));
            }
        }
        ev(FetchEvent::Trying {
            id: spec.id.to_string(),
            url: url.clone(),
            index: i + 1,
            total: sources.len(),
        });

        // 上一次尝试失败可能留下残file，先清掉再下。
        let _ = std::fs::remove_file(&tmp_file);

        match crate::http::download(url.as_str(), &tmp_file, FILE_TIMEOUT_MS) {
            Ok(n) if n >= spec.min_bytes => {
                got = true;
                ev(FetchEvent::Downloaded {
                    id: spec.id.to_string(),
                    bytes: n,
                });
                break;
            }
            Ok(n) => {
                errs.push(format!(
                    "{url} → 只下到 {n} 字节，不足 {} MB",
                    spec.min_bytes / 1048576
                ));
                let _ = std::fs::remove_file(&tmp_file);
            }
            Err(e) => {
                errs.push(format!("{url} → {e}"));
                let _ = std::fs::remove_file(&tmp_file);
            }
        }
    }

    if !got {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(anyhow!(
            "所有下载源都没成功：\n  {}\n\n\
             可能是网络不通（校园网/代理）。可以：\n\
             1) 设置代理后重试：set VCA_PROXY=http://127.0.0.1:7890\n\
             2) 手动下载后放到：{}",
            errs.join("\n  "),
            final_path.display()
        ));
    }

    // ---- 就位 ----
    if t.is_zip {
        ev(FetchEvent::Extracting {
            id: spec.id.to_string(),
        });
        let extract_dir = tmp_dir.join(format!("{}_x", spec.id));
        let _ = std::fs::remove_dir_all(&extract_dir);
        crate::napcat::extract_zip(&tmp_file, &extract_dir)
            .with_context(|| format!("解压 {} 失败", tmp_file.display()))?;

        let want = t
            .pick_exe
            .ok_or_else(|| anyhow!("内部错误：zip 目标没指定要捞的文件名"))?;
        let found = find_in_tree(&extract_dir, want).ok_or_else(|| {
            anyhow!(
                "解压后在压缩包里找不到 {want}（上游可能改了目录结构）。\n\
                 解压目录：{}",
                extract_dir.display()
            )
        })?;

        // 就位前先把旧的挪开：直接覆盖可能在文件被占用时失败
        // （比如用户正开着上次的 ffmpeg 进程）。
        let _ = std::fs::remove_file(&final_path);
        std::fs::copy(&found, &final_path)
            .with_context(|| format!("把 {want} 放到 {} 失败", final_path.display()))?;

        // whisper 除了 exe 还需要同目录的 dll，一并拷过去 ——
        // 只拷 exe 的话，运行时会因为缺 dll 直接起不来，
        // 而报错信息（缺哪个 dll）对用户毫无意义。
        if spec.id == "whisper" {
            if let Some(src_dir) = found.parent() {
                copy_siblings(src_dir, final_path.parent().unwrap_or(tools_root));
            }
        }

        let _ = std::fs::remove_dir_all(&extract_dir);
    } else {
        // 普通文件：直接改名就位（同一分区上的 rename 是原子的）。
        let _ = std::fs::remove_file(&final_path);
        std::fs::rename(&tmp_file, &final_path)
            .or_else(|_| std::fs::copy(&tmp_file, &final_path).map(|_| ()))
            .with_context(|| format!("把下载结果放到 {} 失败", final_path.display()))?;
    }

    let _ = std::fs::remove_file(&tmp_file);
    let _ = std::fs::remove_dir_all(&tmp_dir);

    Ok(final_path)
}

/// 在目录树里递归找一个文件，返回第一个匹配的。
///
/// 为什么要递归找而不是硬编码路径：上游的 zip 解出来常常多套一层目录
/// （`whisper-bin-x64/whisper-cli.exe`、`ffmpeg-7.0-essentials_build/bin/ffmpeg.exe`），
/// 层数和目录名都会随版本变。硬编码的话上游一改我们就静默失效 ——
/// 而且是那种「下载明明成功了，却还是提示缺失」的难查故障。
pub fn find_in_tree(root: &Path, filename: &str) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    let mut guard = 0usize;
    while let Some(dir) = stack.pop() {
        // 防御性上限：万一遇到异常深的目录树（或软链接环）不至于卡死。
        guard += 1;
        if guard > 4096 {
            return None;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .file_name()
                .map(|n| n.eq_ignore_ascii_case(filename))
                .unwrap_or(false)
            {
                return Some(p);
            }
        }
    }
    None
}

/// 把 `src` 目录里的文件（不含子目录）拷到 `dst`。
///
/// 用于把 whisper 的 dll 一起搬过去。失败不报错：dll 没拷全顶多是转写起不来，
/// 而"因为拷 dll 失败就把整个补齐流程判为失败"更糟 —— 用户会看到
/// 「补齐失败」但实际上 exe 已经到位了。
fn copy_siblings(src: &Path, dst: &Path) {
    let Ok(rd) = std::fs::read_dir(src) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            continue;
        }
        let Some(name) = p.file_name() else { continue };
        let target = dst.join(name);
        // 已经是同一个文件（exe 自己）就跳过。
        if p == target {
            continue;
        }
        let _ = std::fs::copy(&p, &target);
    }
}

/// 「可选项已提醒过」的持久化标记。
///
/// 存成一个小文件而不是塞进 settings.yaml：这不是**配置**，
/// 而是「一次性提示是否已发过」的运行时痕迹；放进配置文件会让它出现在
/// 用户的设置文件里，既不好解释，也可能被用户误以为是开关。
pub mod notify_state {
    use std::path::{Path, PathBuf};

    /// 标记文件的路径。
    pub fn marker_path(config_root: &Path) -> PathBuf {
        config_root.join(".deps-notified")
    }

    /// 可选项是否已经提醒过。
    pub fn optional_notified(config_root: &Path) -> bool {
        marker_path(config_root).is_file()
    }

    /// 记下「可选项已经提醒过」。
    ///
    /// 写入失败**不报错**：最坏的结果是下次启动再提醒一次，
    /// 这比因为写不了标记文件而弹一个错误窗要好得多。
    pub fn mark_optional_notified(config_root: &Path) {
        let p = marker_path(config_root);
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(&p, b"1");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "vca-fetch-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&p);
        p
    }

    /// 递归查找必须能找到**嵌套**层里的文件。
    ///
    /// 这条防的是「上游多套一层目录 -> 下载成功却还提示缺失」那个隐蔽故障。
    #[test]
    fn find_in_tree_walks_nested_dirs() {
        let d = tmpdir("find");
        let nested = d.join("whisper-bin-x64").join("Release");
        std::fs::create_dir_all(&nested).unwrap();
        let exe = nested.join("whisper-cli.exe");
        std::fs::write(&exe, b"x").unwrap();

        let found = find_in_tree(&d, "whisper-cli.exe");
        assert_eq!(found, Some(exe), "应当能在嵌套目录里找到目标文件");

        // 大小写不敏感：Windows 上文件名本来就不区分大小写
        assert!(
            find_in_tree(&d, "WHISPER-CLI.EXE").is_some(),
            "查找应当不区分大小写"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 找不到就返回 None，不能崩。
    #[test]
    fn find_in_tree_returns_none_when_absent() {
        let d = tmpdir("findnone");
        assert_eq!(find_in_tree(&d, "nothing-here.exe"), None);
        // 目录本身不存在也不该崩
        assert_eq!(find_in_tree(&d.join("no-such-dir"), "x.exe"), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 深层目录也要能挖到（ffmpeg 的 zip 是 bin/ffmpeg.exe 这种结构）。
    #[test]
    fn find_in_tree_handles_deep_nesting() {
        let d = tmpdir("finddeep");
        let deep = d.join("ffmpeg-7.0").join("bin");
        std::fs::create_dir_all(&deep).unwrap();
        let exe = deep.join("ffmpeg.exe");
        std::fs::write(&exe, b"y").unwrap();
        assert_eq!(find_in_tree(&d, "ffmpeg.exe"), Some(exe));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 兄弟文件拷贝：whisper 的 dll 要跟着走。
    #[test]
    fn copy_siblings_moves_dlls() {
        let d = tmpdir("sib");
        let src = d.join("from");
        let dst = d.join("to");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("whisper-cli.exe"), b"exe").unwrap();
        std::fs::write(src.join("ggml.dll"), b"dll").unwrap();
        std::fs::write(src.join("whisper.dll"), b"dll2").unwrap();
        // 子目录不该被拷（只拷平级文件）
        std::fs::create_dir_all(src.join("sub")).unwrap();

        copy_siblings(&src, &dst);

        assert!(dst.join("whisper-cli.exe").is_file());
        assert!(dst.join("ggml.dll").is_file(), "dll 必须跟着一起走");
        assert!(dst.join("whisper.dll").is_file());
        assert!(!dst.join("sub").exists(), "子目录不该被拷贝");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 目标目录不存在时，copy_siblings 安静返回。
    #[test]
    fn copy_siblings_survives_missing_dir() {
        let d = tmpdir("sibmiss");
        copy_siblings(&d.join("nope"), &d.join("also-nope"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 目标已就绪时，不会去下载。
    ///
    /// 这条**刻意不去造真实大小的文件**。它踩过两次坑：
    /// 1. 先写成 `vec![0u8; min_bytes]` —— 为了 20 MB 的模型真的分配 20 MB 内存；
    /// 2. 改成稀疏文件 `set_len` —— 逻辑大小到位了，但磁盘**元数据**要求
    ///    可分配空间，在磁盘紧张的机器上直接 `StorageFull`。
    ///
    /// 两次挂掉都不是产品的问题，是测试在拿真实体量做无谓的事。
    /// 判定逻辑（大小够不够）已经有 `deps` 模块的 `judge` 系列专门覆盖；
    /// 这里要验证的是**「齐了就不动作」这个结论**，所以用 KB 级的小阈值
    /// 完整走一遍 `judge` -> `check_all` 的链路就够了。
    #[test]
    fn nothing_to_do_when_all_ready() {
        let d = tmpdir("allok");
        // 用 KB 级阈值，避免为了"够大"去占真实空间。
        const SMALL: u64 = 1024;
        for spec in deps::DEPS {
            let p = d.join(spec.rel_path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            // 每个文件写 2 KB：都能过 SMALL 这个阈值
            std::fs::write(&p, vec![0u8; 2048]).unwrap();
        }
        // 逐个用 SMALL 阈值判定：应当全部就绪
        for spec in deps::DEPS {
            let p = d.join(spec.rel_path);
            let (ok, why) = deps::judge(&p, SMALL);
            assert!(ok, "{} 应当就绪，实际：{why}", spec.id);
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 就绪的文件在 `fetch_missing` 里会被跳过（不发起下载）。
    ///
    /// 这条不需要真实大小：传一个不在清单里的 id，等价于"没有要补的东西"，
    /// 函数应当安静返回，且不产生任何网络动作。
    #[test]
    fn fetch_with_unknown_id_does_nothing() {
        let d = tmpdir("fetchunknown");
        let only = vec!["完全不存在的组件".to_string()];
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let ev = std::sync::Arc::clone(&events);
        let progress: ProgressFn = Box::new(move |e| {
            if let Ok(mut v) = ev.lock() {
                v.push(format!("{e:?}"));
            }
        });

        let out = fetch_missing(&d, Some(&only), Some(progress), None);

        // 没有任何 Start/Trying：说明没去尝试下载不认识的东西
        let log = events.lock().map(|v| v.join("\n")).unwrap_or_default();
        assert!(!log.contains("Trying"), "不该为不存在的组件发起下载：{log}");
        assert!(!out.is_empty(), "应当返回清单的检查结果");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 标记文件：写之前是 false，写完变 true。
    #[test]
    fn notify_marker_roundtrip() {
        let d = tmpdir("marker");
        assert!(!notify_state::optional_notified(&d), "初始应当未提醒");
        notify_state::mark_optional_notified(&d);
        assert!(notify_state::optional_notified(&d), "标记后应当已提醒");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 每个目标都必须有下载源，且路径合法。
    #[test]
    fn every_target_has_sources() {
        for t in TARGETS {
            assert!(!t.urls.is_empty(), "{} 没有下载源", t.spec.id);
            for u in t.urls {
                assert!(
                    u.starts_with("https://") || u.starts_with("http://"),
                    "{} 的源不是 http(s)：{u}",
                    t.spec.id
                );
            }
            if t.is_zip {
                assert!(
                    t.pick_exe.is_some(),
                    "{} 是 zip 但没指定要捞的文件名",
                    t.spec.id
                );
            }
        }
    }

    /// 每个依赖都要有对应的下载目标，否则「一键补齐」会漏掉它。
    #[test]
    fn every_dep_has_a_target() {
        for spec in deps::DEPS {
            assert!(
                TARGETS.iter().any(|t| t.spec.id == spec.id),
                "{} 在清单里但没有下载目标",
                spec.id
            );
        }
    }

    /// zip 目标给出的文件名要和清单里的落地文件名一致。
    ///
    /// 这两处一旦不一致，会出现「下下来了、也解压了，但就是找不对文件」。
    #[test]
    fn pick_exe_matches_declared_rel_path() {
        for t in TARGETS {
            if let Some(pick) = t.pick_exe {
                let rel = t.spec.rel_path.replace('/', "\\");
                let leaf = rel.rsplit('\\').next().unwrap_or("");
                assert_eq!(
                    leaf.to_lowercase(),
                    pick.to_lowercase(),
                    "{} 的 pick_exe({pick}) 与 rel_path 的文件名({leaf}) 不一致",
                    t.spec.id
                );
            }
        }
    }

    /// 用户配置的地址必须**真的传到下载器**，不能只在 API/config 层看起来保存成功。
    ///
    /// 这条用本地 HTTP 服务器完整走 `merge_sources -> http::download -> 就位`，
    /// 并把内置源故意设成一个必然失败的端口。如果后端偷偷覆盖/忽略用户值，
    /// 测试就会失败。
    #[test]
    fn custom_source_reaches_the_real_downloader_first() {
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;

        static SPEC: deps::DepSpec = deps::DepSpec {
            id: "mirror-e2e",
            label: "镜像透传测试",
            consequence: "仅测试",
            required: false,
            rel_path: "mirror-e2e/payload.bin",
            min_bytes: 16,
        };
        static BAD_BUILTIN: &[&str] = &["http://127.0.0.1:1/never-used.bin"];

        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定本地测试端口");
        let port = listener.local_addr().expect("本地地址").port();
        let custom = format!("http://127.0.0.1:{port}/custom.bin");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("收到下载请求");
            let mut req = [0u8; 1024];
            let n = stream.read(&mut req).unwrap_or(0);
            let head = String::from_utf8_lossy(&req[..n]);
            assert!(
                head.starts_with("GET /custom.bin "),
                "真实下载器没有请求用户地址：{head}"
            );
            let body = vec![b'M'; 64];
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .expect("写响应头");
            stream.write_all(&body).expect("写响应体");
        });

        let root = tmpdir("mirror-e2e");
        let target = Target {
            spec: &SPEC,
            urls: BAD_BUILTIN,
            is_zip: false,
            pick_exe: None,
        };
        let mut cfg = vca_core::config::DownloadSettings::default();
        cfg.mirrors
            .insert(SPEC.id.to_string(), vec![custom.clone()]);

        let events = std::sync::Mutex::new(Vec::<String>::new());
        let progress = |e: FetchEvent| {
            if let FetchEvent::Trying { url, .. } = e {
                events.lock().expect("事件锁").push(url);
            }
        };
        let out = fetch_one(&target, &root, &progress, &None, &cfg).expect("自定义源下载成功");

        server.join().expect("本地服务器正常结束");
        assert_eq!(std::fs::read(&out).expect("读下载文件"), vec![b'M'; 64]);
        let tried = events.lock().expect("事件锁");
        assert_eq!(
            tried.as_slice(),
            &[custom],
            "用户源没有排在第一位：{tried:?}"
        );
        assert!(
            !tried.iter().any(|u| u == BAD_BUILTIN[0]),
            "用户源成功后仍去试了内置源：{tried:?}"
        );

        let _ = std::fs::remove_dir_all(root);
    }
}
