//! 运行时依赖清单与「缺什么」的判定。
//!
//! # 这一层解决什么问题
//!
//! 程序正常运行需要几个**不在仓库里**的外部组件（加起来约 125 MB，
//! 塞进 Git 不合适）：ffmpeg、whisper.cpp、语音模型。部署到一台新的一体机时
//! 它们可能是缺的，缺了的后果**并不一样**：
//!
//! - 缺 ffmpeg → **彻底不能用**（录不了屏），必须每次提醒；
//! - 缺 whisper / 模型 → 只是本地转写不可用，还能走云端，提醒一次就够。
//!
//! 「弹窗提示 + 一键补齐」要成立，前提是先把这份清单说清楚：缺什么、
//! 要不要紧、怎么判定它真的可用。本模块只负责**声明与判定**，
//! 下载交给 [`crate::fetch`]，弹窗交给界面层。
//!
//! # 为什么判定要看「文件存在」还要看「大小」
//!
//! 网络中断留下的**半截文件**是真实发生过的故障：文件在、路径对、
//! 但内容不完整，调用时才会以难以理解的方式失败（`whisper-cli` 直接崩、
//! zip 解压报损坏）。所以每项都带一个**下限字节数**，小于它一律当缺失 ——
//! 宁可让用户重下一遍，也不要把半截文件当成"已就绪"。

use std::path::{Path, PathBuf};

/// 一项运行时依赖。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepSpec {
    /// 稳定标识（用于日志、界面与测试，不要随意改）。
    pub id: &'static str,
    /// 面向用户的名称。
    pub label: &'static str,
    /// 缺失的后果说明（告诉用户"不补会怎样"）。
    pub consequence: &'static str,
    /// 是否必需：必需项每次启动都提醒，可选项只提醒一次。
    pub required: bool,
    /// 对应 `tools/` 下的相对路径。
    pub rel_path: &'static str,
    /// 文件大小下限（字节）。小于它视为**不完整**，等同于缺失。
    pub min_bytes: u64,
}

/// 全部可自动补齐的依赖。
///
/// 顺序即界面上的显示顺序，也基本是「重要性从高到低」。
/// 把 ffmpeg 放第一位是因为它是唯一真正的硬依赖。
pub const DEPS: &[DepSpec] = &[
    DepSpec {
        id: "ffmpeg",
        label: "ffmpeg（录屏与音视频合并）",
        consequence: "无法录制屏幕，程序完全不能工作",
        required: true,
        rel_path: r"ffmpeg\ffmpeg.exe",
        // 官方 essentials 构建约 80 MB；这里留足余量只挡住"半截文件"。
        min_bytes: 5 * 1024 * 1024,
    },
    DepSpec {
        id: "whisper",
        label: "whisper.cpp（本地语音转写）",
        consequence: "本地转写不可用（可以改走云端转写）",
        required: false,
        rel_path: r"whisper\whisper-cli.exe",
        // 实测本机（上游 v1.7.6 的 whisper-bin-x64）是 **469 KB**。
        // 这个下限只能明显低于它：原先设成 512 KB，结果把一个**完好**的安装
        // 判成了缺失 —— 阈值 512KB 反而比实际文件 480KB 还大。
        //
        // 教训：这类下限的唯一职责是挡住**半截文件**（几 KB 甚至 0 字节），
        // 不是去猜"正常应该多大"。上游一换构建方式体积就变，
        // 猜得越准越容易在版本变化时误报。留 64 KB 只挡下载中断的碎片。
        min_bytes: 64 * 1024,
    },
    DepSpec {
        id: "model",
        label: "语音模型 tiny（本地转写）",
        consequence: "本地转写不可用（可以改走云端转写）",
        required: false,
        rel_path: r"whisper\models\ggml-tiny.bin",
        // tiny 模型约 75 MB；下限设成 20 MB，能被半截下载挡住，
        // 又不会因为上游换了个稍小的构建就误报。
        min_bytes: 20 * 1024 * 1024,
    },
];

/// 某项的判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepState {
    /// 对应 [`DepSpec::id`]。
    pub id: &'static str,
    /// 面向用户的名称。
    pub label: &'static str,
    /// 缺失的后果。
    pub consequence: &'static str,
    /// 是否必需。
    pub required: bool,
    /// 是否已就绪。
    pub ready: bool,
    /// 缺失原因（已就绪时为空）。直接给用户看，所以要写人话。
    pub reason: String,
    /// 应放置的绝对路径。
    pub path: PathBuf,
}

impl DepState {
    /// 是否可以由程序自动补齐。
    ///
    /// 目前清单里的项**都**能自动下载，所以恒为真。留着这个函数是因为
    /// 将来若有「只能手动装」的依赖（比如需要许可的第三方组件），
    /// 界面上要能把它排除在「一键补齐」之外 —— 与其那时再改调用点，
    /// 不如现在就把语义留出来。
    pub fn auto_fixable(&self) -> bool {
        DEPS.iter()
            .any(|d| d.id == self.id && !d.rel_path.is_empty())
    }
}

/// 检查清单里的一项。
///
/// `tools_root` 是 `tools/` 目录（不存在也算缺失，不报错）。
pub fn check_one(spec: &DepSpec, tools_root: &Path) -> DepState {
    let path = tools_root.join(spec.rel_path);
    let (ready, reason) = judge(&path, spec.min_bytes);
    DepState {
        id: spec.id,
        label: spec.label,
        consequence: spec.consequence,
        required: spec.required,
        ready,
        reason,
        path,
    }
}

/// 把字节数写成人话。
///
/// 为什么要分单位而不是一律 `n / 1048576`：whisper 的下限是 64 KB，
/// 整数除法会算成「0 MB」，于是提示变成"文件不完整（0 MB，至少应有 0 MB）"——
/// 用户看了只会更困惑。小于 1 MB 时改用 KB。
fn humanize(bytes: u64) -> String {
    if bytes >= 1048576 {
        format!("{} MB", bytes / 1048576)
    } else if bytes >= 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{bytes} 字节")
    }
}

/// 判定一个文件是否算「就绪」。
///
/// 抽出来是为了能单独测试 —— 尤其是「半截文件算缺失」那条，
/// 用真实文件系统构造比在检查逻辑里绕圈子更清楚。
pub fn judge(path: &Path, min_bytes: u64) -> (bool, String) {
    match std::fs::metadata(path) {
        Ok(m) => {
            if !m.is_file() {
                return (false, "路径存在但不是文件".to_string());
            }
            let size = m.len();
            if size < min_bytes {
                return (
                    false,
                    format!(
                        "文件不完整（{}，至少应有 {}，可能上次下载中断）",
                        humanize(size),
                        humanize(min_bytes)
                    ),
                );
            }
            (true, String::new())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (false, "文件不存在".to_string()),
        Err(e) => (false, format!("无法读取：{e}")),
    }
}

/// 检查清单里的全部项。
pub fn check_all(tools_root: &Path) -> Vec<DepState> {
    DEPS.iter().map(|s| check_one(s, tools_root)).collect()
}

/// 从检查结果里挑出**需要提醒用户**的那些。
///
/// 这是「按严重程度区分」规则的落点：
/// - **必需项**缺失 → 每次都提醒（程序真的不能用，藏起来才是害人）；
/// - **可选项**缺失 → 只在**第一次**提醒（用户已经知道了，再弹就是骚扰）。
///
/// `optional_notified` 表示「可选项已经提醒过至少一次了」。
/// 它由调用方持久化（见 [`crate::fetch::notify_state`]），
/// 本函数只做纯判定，方便测试。
pub fn need_notice(states: &[DepState], optional_notified: bool) -> Vec<DepState> {
    states
        .iter()
        .filter(|s| !s.ready)
        .filter(|s| s.required || !optional_notified)
        .cloned()
        .collect()
}

/// 缺失项是否可以**全部**由程序自动补齐。
pub fn all_auto_fixable(missing: &[DepState]) -> bool {
    missing.iter().all(|s| s.auto_fixable())
}

/// `tools/` 目录的位置。
///
/// 与 ffmpeg / whisper 的既有查找规则保持一致（见 [`crate::proc::find_tool`]）：
/// 以 exe 所在目录为基准，开发时回退到仓库根。**不能**用当前工作目录 ——
/// 用户从任意位置双击 exe 时，工作目录是不确定的。
pub fn tools_root(exe_dir: &Path) -> PathBuf {
    let beside = exe_dir.join("tools");
    if beside.is_dir() {
        return beside;
    }
    // 开发形态：exe 在 target/debug 或 target/release 下，往上找到仓库根。
    for up in [2, 3] {
        let mut p = exe_dir.to_path_buf();
        for _ in 0..up {
            p = match p.parent() {
                Some(x) => x.to_path_buf(),
                None => return beside,
            };
        }
        let cand = p.join("tools");
        if cand.is_dir() {
            return cand;
        }
    }
    beside
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "vca-deps-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&p);
        p
    }

    /// ffmpeg 是唯一必需项 —— 这条一旦被改坏，提醒策略就整错。
    #[test]
    fn ffmpeg_is_the_only_required_dep() {
        let req: Vec<_> = DEPS.iter().filter(|d| d.required).map(|d| d.id).collect();
        assert_eq!(req, vec!["ffmpeg"], "必需项应当只有 ffmpeg");
    }

    /// 缺文件 → 缺失，且原因说人话。
    #[test]
    fn missing_file_is_not_ready() {
        let dir = tmpdir("missing");
        let (ok, why) = judge(&dir.join("nope.exe"), 1024);
        assert!(!ok);
        assert!(why.contains("不存在"), "原因应当说明不存在：{why}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **半截文件必须算缺失。**
    ///
    /// 这是本模块存在的核心理由之一：文件在、名字对，但内容不完整，
    /// 当成就绪只会在真正调用时以难懂的方式炸掉。
    #[test]
    fn truncated_file_is_not_ready() {
        let dir = tmpdir("trunc");
        let f = dir.join("half.bin");
        std::fs::write(&f, b"only a few bytes").unwrap();
        let (ok, why) = judge(&f, 20 * 1024 * 1024);
        assert!(!ok, "半截文件不该被当成就绪");
        assert!(why.contains("不完整"), "原因应当提示不完整：{why}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 够大的文件才算就绪。
    #[test]
    fn big_enough_file_is_ready() {
        let dir = tmpdir("ok");
        let f = dir.join("good.bin");
        std::fs::write(&f, vec![0u8; 2048]).unwrap();
        let (ok, why) = judge(&f, 1024);
        assert!(ok, "应当就绪，原因：{why}");
        assert!(why.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 目录不能冒充文件。
    #[test]
    fn directory_is_not_ready() {
        let dir = tmpdir("isdir");
        let sub = dir.join("ffmpeg.exe");
        std::fs::create_dir_all(&sub).unwrap();
        let (ok, why) = judge(&sub, 1);
        assert!(!ok);
        assert!(why.contains("不是文件"), "{why}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 可选项只提醒一次；必需项每次提醒。
    #[test]
    fn notice_policy_respects_severity() {
        let states = vec![
            DepState {
                id: "ffmpeg",
                label: "ffmpeg",
                consequence: "不能用",
                required: true,
                ready: false,
                reason: "文件不存在".into(),
                path: PathBuf::from("x"),
            },
            DepState {
                id: "whisper",
                label: "whisper",
                consequence: "本地转写不可用",
                required: false,
                ready: false,
                reason: "文件不存在".into(),
                path: PathBuf::from("y"),
            },
        ];

        // 第一次：两项都要提醒
        let first = need_notice(&states, false);
        assert_eq!(first.len(), 2, "第一次应当两项都提醒");

        // 已经提醒过可选项：只剩必需的 ffmpeg
        let later = need_notice(&states, true);
        assert_eq!(later.len(), 1, "可选项不该反复提醒");
        assert_eq!(later[0].id, "ffmpeg", "必需项必须每次提醒");
    }

    /// 都就绪时不需要任何提醒。
    #[test]
    fn nothing_ready_needs_no_notice() {
        let states = vec![DepState {
            id: "ffmpeg",
            label: "ffmpeg",
            consequence: "x",
            required: true,
            ready: true,
            reason: String::new(),
            path: PathBuf::from("x"),
        }];
        assert!(need_notice(&states, false).is_empty());
    }

    /// 清单里每项都能自动补齐（否则界面上的「一键补齐」会漏掉它）。
    #[test]
    fn every_dep_is_auto_fixable() {
        let dir = tmpdir("fixable");
        for s in check_all(&dir) {
            assert!(s.auto_fixable(), "{} 不在可自动补齐范围内", s.id);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 路径必须都落在 tools/ 下面（不然会把文件下到莫名其妙的地方）。
    #[test]
    fn all_paths_live_under_tools() {
        let root = PathBuf::from("TOOLS");
        for s in DEPS {
            let p = root.join(s.rel_path);
            assert!(
                p.starts_with(&root),
                "{} 的路径逃出了 tools/：{}",
                s.id,
                p.display()
            );
        }
    }

    /// id 不能重复（重复会让状态文件互相覆盖）。
    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<_> = DEPS.iter().map(|d| d.id).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "依赖 id 有重复");
    }

    /// **下限不能高到把真实文件挡住。**
    ///
    /// 这条是被真实故障逼出来的：whisper-cli.exe 实际 469 KB，
    /// 而我一度把下限设成 512 KB —— 结果在一台**完全健康**的机器上
    /// 弹出了"缺少组件"。下限的唯一职责是挡住半截文件（几 KB / 0 字节），
    /// 不是去猜正常体积。
    ///
    /// 这里用实测值钉住：只要有人把下限调到接近或超过真实体积，
    /// 这条测试就会失败。
    #[test]
    fn min_bytes_leaves_room_for_real_files() {
        // 各组件在真实环境里的实测大小（字节）。改了阈值就要重新量。
        let real_sizes: &[(&str, u64)] = &[
            ("whisper", 480_256),    // tools/whisper/whisper-cli.exe
            ("ffmpeg", 102_900_000), // ~98 MB
            ("model", 77_700_000),   // ~74 MB
        ];
        for (id, real) in real_sizes {
            let spec = DEPS.iter().find(|d| d.id == *id).expect("依赖必须存在");
            assert!(
                spec.min_bytes < *real,
                "{id}: 下限 {} 字节 >= 真实文件 {real} 字节 —— \
                 这会把完好的安装误判成缺失",
                spec.min_bytes
            );
        }
    }

    /// 下限也不能小到形同虚设：半截文件（几 KB）必须能被挡住。
    #[test]
    fn min_bytes_still_catches_fragments() {
        for spec in DEPS {
            assert!(
                spec.min_bytes >= 16 * 1024,
                "{} 的下限 {} 太小，挡不住下载中断留下的碎片",
                spec.id,
                spec.min_bytes
            );
        }
    }

    /// 提示里的体积要按量级选单位。
    ///
    /// 否则 whisper 那种 KB 级下限会显示成「至少应有 0 MB」——
    /// 一句让人更糊涂的话。
    #[test]
    fn humanize_picks_sensible_units() {
        assert_eq!(humanize(0), "0 字节");
        assert_eq!(humanize(512), "512 字节");
        assert_eq!(humanize(64 * 1024), "64 KB");
        assert_eq!(humanize(20 * 1024 * 1024), "20 MB");
    }

    /// 不完整的提示不能出现「0 MB」这种没意义的话。
    #[test]
    fn incomplete_message_is_informative() {
        let dir = tmpdir("msg");
        let f = dir.join("tiny.bin");
        std::fs::write(&f, b"abc").unwrap();
        let (ok, why) = judge(&f, 64 * 1024);
        assert!(!ok);
        assert!(why.contains("64 KB"), "应当用 KB 表达下限，实际：{why}");
        assert!(!why.contains("0 MB"), "不该出现 0 MB：{why}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
