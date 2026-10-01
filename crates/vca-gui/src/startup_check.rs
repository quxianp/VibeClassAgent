//! 启动时的依赖自检（不是接口，是**启动路径上真的会跑**的那段）。
//!
//! # 为什么单独一个模块
//!
//! 判定逻辑（[`vca_platform::deps`]）和下载（[`vca_platform::fetch`]）早就有了，
//! 但它们原来只挂在 `/api/deps` 上 —— **没有任何人主动调用**。
//! 后果是：如果界面没去问，用户永远不知道缺东西，直到某天录屏失败才发现。
//! 「启动自检」必须发生在**启动时**，而不是等谁来问。
//!
//! 用户明确要求的正是这一条：*开屏阶段的缺失文件自检功能目前没有加入*。
//!
//! # 为什么不开在最前面
//!
//! 自检要在**窗口出来之后**才跑（异步、后台线程）。理由：
//!
//! - 检测本身要 stat 几个文件，很快；但**下载**是几十到上百 MB，
//!   放在窗口出来之前会让启动卡住几十秒 —— 那正是之前被投诉的问题。
//! - 缺 ffmpeg 也**不影响打开界面**（界面、配置、课表都能看），
//!   所以没有理由为了它推迟开窗。
//!
//! 所以顺序是：起服务 → 开窗口（用户看到界面）→ 后台自检 → 有缺失就提示。
//!
//! # 提示的形态
//!
//! 走**界面内提示**（写进事件日志 + 一个待办标记），而不是模态弹窗。
//! 模态弹窗会挡住界面，而"缺 whisper"这种非致命项不该拦住用户干活。
//! 致命的（缺 ffmpeg）由界面的 `need_notice` 分级规则自己决定怎么强调 ——
//! 分级策略是后端算的（见 [`vca_platform::deps::need_notice`]），这里只管把
//! "该提醒了"这件事**推到界面能看见的地方**。

use std::path::{Path, PathBuf};

/// 自检结果，给日志和界面用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupCheck {
    /// 全部依赖的状态。
    pub states: Vec<vca_platform::deps::DepState>,
    /// 这次该提醒用户的那些（已按必需/可选分级）。
    pub notice: Vec<vca_platform::deps::DepState>,
}

impl StartupCheck {
    /// 有没有缺东西（不分必需可选）。
    pub fn has_missing(&self) -> bool {
        !self.notice.is_empty()
    }

    /// 缺的是不是全是"必需的"。
    ///
    /// 用来决定提示语气：全必需 = 程序真的不能用，要显眼；
    /// 有可选 = 先说明"还能用，只是少了某个功能"，别吓人。
    pub fn all_required(&self) -> bool {
        !self.notice.is_empty() && self.notice.iter().all(|s| s.required)
    }

    /// 渲染成一行给日志/界面看的摘要。
    pub fn summary(&self) -> String {
        if self.notice.is_empty() {
            return "依赖自检通过".to_string();
        }
        let names: Vec<&str> = self.notice.iter().map(|s| s.id).collect();
        format!("缺少 {} 项依赖：{}", self.notice.len(), names.join("、"))
    }
}

/// 跑一次自检。**不下载，只看。**
///
/// `config_root` 用来读「可选项是否已经提醒过」的持久标记 ——
/// 没有它的话每次启动都会为同一个缺失的 whisper 弹一次，
/// 用户改不掉又关不掉，只能忍着（这就是"骚扰"的定义）。
pub fn run(tools_root: &Path, config_root: &Path) -> StartupCheck {
    let states = vca_platform::deps::check_all(tools_root);
    let notified = vca_platform::fetch::notify_state::optional_notified(config_root);
    let notice = vca_platform::deps::need_notice(&states, notified);
    StartupCheck { states, notice }
}

/// `tools/` 目录在哪。
///
/// 以 **exe 所在目录**为基准，不能用当前工作目录 —— 用户从桌面快捷方式
/// 双击时工作目录是任意的（甚至是 `C:\Windows\System32`）。
pub fn tools_root_from_exe() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    vca_platform::deps::tools_root(&exe_dir)
}

/// 自检并把结果记进日志；返回结果供调用方决定要不要提示。
///
/// # 为什么"记日志"和"判定"要在一起
///
/// 这个函数的调用点在启动路径上，**多一行都可能被忘掉**。
/// 把它做成一件事（检查 + 记账），调用方只需要接结果，
/// 不会出现"检查了但没记日志"或"记了日志但没判定"的半吊子状态。
pub fn run_and_log(tools_root: &Path, config_root: &Path) -> StartupCheck {
    let result = run(tools_root, config_root);

    if result.has_missing() {
        // 逐项记，方便事后查"到底缺哪个、缺到什么程度"。
        for s in &result.notice {
            tracing::warn!(
                "依赖缺失：{}（{}）→ {}；期望路径 {}",
                s.label,
                if s.required { "必需" } else { "可选" },
                s.consequence,
                s.path.display()
            );
        }
        tracing::warn!("{}", result.summary());
    } else {
        tracing::info!("依赖自检通过（{} 项全部就绪）", result.states.len());
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "vca-startup-check-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&p).expect("建临时目录");
        p
    }

    /// 空目录下：三项全缺，ffmpeg 是必需的，所以**必须**出现在提醒里。
    ///
    /// 这条守的是"启动自检真的会报" —— 如果哪天 `run` 被改成
    /// 只查不报，或者 need_notice 的分级被改坏，这里会红。
    #[test]
    fn missing_ffmpeg_is_always_reported() {
        let tools = tmpdir("empty");
        let cfg = tmpdir("cfg");
        let r = run(&tools, &cfg);

        assert!(r.has_missing(), "空目录下应当报缺失");
        let ids: Vec<&str> = r.notice.iter().map(|s| s.id).collect();
        assert!(
            ids.contains(&"ffmpeg"),
            "必需项 ffmpeg 缺失必须每次都提醒，实际 {ids:?}"
        );
        assert!(!r.all_required(), "还有可选项缺失，不该说全是必需的");

        std::fs::remove_dir_all(&tools).ok();
        std::fs::remove_dir_all(&cfg).ok();
    }

    /// 摘要要能一眼看出缺什么。
    #[test]
    fn summary_names_the_missing_items() {
        let tools = tmpdir("sum");
        let cfg = tmpdir("sumcfg");
        let r = run(&tools, &cfg);

        let s = r.summary();
        assert!(s.contains("缺少"), "摘要没说要紧的话：{s}");
        assert!(s.contains("ffmpeg"), "摘要没点名 ffmpeg：{s}");

        std::fs::remove_dir_all(&tools).ok();
        std::fs::remove_dir_all(&cfg).ok();
    }

    /// 全就绪时不该报任何东西。
    ///
    /// 造一个"假 ffmpeg"（够大就行 —— 判定只看存在与大小下限）
    /// 来验证"就绪了就不再提醒"，避免用户被反复打扰。
    #[test]
    fn ready_ffmpeg_is_not_reported() {
        let tools = tmpdir("ready");
        let cfg = tmpdir("readycfg");

        // 按清单里的相对路径放一个够大的假文件
        let spec = vca_platform::deps::DEPS
            .iter()
            .find(|d| d.id == "ffmpeg")
            .expect("清单里应当有 ffmpeg");
        let dst = tools.join(spec.rel_path);
        std::fs::create_dir_all(dst.parent().expect("有父目录")).ok();
        let fake = vec![0u8; (spec.min_bytes + 1024) as usize];
        std::fs::write(&dst, &fake).expect("写假文件");

        let r = run(&tools, &cfg);
        let ids: Vec<&str> = r.notice.iter().map(|s| s.id).collect();
        assert!(!ids.contains(&"ffmpeg"), "ffmpeg 已就绪还被提醒了：{ids:?}");

        std::fs::remove_dir_all(&tools).ok();
        std::fs::remove_dir_all(&cfg).ok();
    }

    /// 可选项提醒过一次之后就不该再提醒（否则每次启动都弹，很招人烦）。
    ///
    /// 这是「骚扰」和「尽责」的分界，值得一条单独的测试守住。
    #[test]
    fn optional_items_are_not_reported_twice() {
        let tools = tmpdir("twice");
        let cfg = tmpdir("twicecfg");

        // 首次：可选项没提醒过，whisper/model 应当出现
        let first = run(&tools, &cfg);
        let first_optional: Vec<&str> = first
            .notice
            .iter()
            .filter(|s| !s.required)
            .map(|s| s.id)
            .collect();
        assert!(
            !first_optional.is_empty(),
            "首次启动可选项缺失应当提醒（否则用户根本不知道）"
        );

        // 标记为"已提醒"
        vca_platform::fetch::notify_state::mark_optional_notified(&cfg);

        // 再次：可选项不该出现，但必需项（ffmpeg）仍然要报
        let second = run(&tools, &cfg);
        let second_optional = second.notice.iter().filter(|s| !s.required).count();
        assert_eq!(second_optional, 0, "可选项提醒过一次后不该再提醒");
        assert!(
            second.notice.iter().any(|s| s.required),
            "必需项始终要提醒，不能被'提醒过'消掉"
        );

        std::fs::remove_dir_all(&tools).ok();
        std::fs::remove_dir_all(&cfg).ok();
    }

    /// `tools_root_from_exe` 必须返回绝对路径、且以 `tools` 结尾。
    ///
    /// 相对路径会让"缺什么"的提示里显示一个看不懂的路径，
    /// 用户拿着它去找不到文件。
    #[test]
    fn tools_root_is_absolute_and_named_tools() {
        let r = tools_root_from_exe();
        assert!(r.is_absolute(), "tools 路径必须是绝对的：{}", r.display());
        assert_eq!(
            r.file_name().and_then(|s| s.to_str()),
            Some("tools"),
            "末级目录应当是 tools：{}",
            r.display()
        );
    }
}
