//! # vca-cli
//!
//! 命令行入口。对应策划书 5.6 的命令集合：
//! `config` / `timetable` / `schedule` / `record` / `task` / `plugin` / `market` /
//! `log` / `doctor` / `profile`。
//!
//! 已实现：doctor / config / overlay / run / clean / plugin / import / debug。
//! 预留：timetable / schedule / record / task / market / log / profile。
//!
//! # 为什么没有 `#![windows_subsystem = "windows"]`
//!
//! 那个属性会把 exe 编成 GUI 子系统，双击确实不会弹控制台 —— 但它是
//! **编译期**的，而这个程序同时要当命令行工具（`vca doctor`、`vca plugin list`
//! 都是靠 stdout 干活的，要能重定向进脚本和计划任务）。
//! 编成 GUI 子系统后 stdout 就没有接收端了，`vca doctor > out.txt` 会得到空文件，
//! `| Select-String` 也拿不到东西 —— 拿"少一个窗口"换掉整个 CLI 用法不划算。
//!
//! 所以改成**运行期**判断：GUI 模式下调 [`vca_platform::console::hide()`] 把
//! 窗口藏掉，CLI 模式完全不碰。同一个二进制两种行为，两边都不牺牲。
//!
//! 相关的取舍细节见 `hide_console_for_gui` 与 `vca_platform::console`。

#![forbid(unsafe_code)]

use anyhow::Result;
use clap::{Parser, Subcommand};

use vca_core::paths::Layout;
use vca_core::{PRODUCT_NAME, VERSION};

mod cmd;
mod repl;

/// 顶层命令行。
#[derive(Debug, Parser)]
#[command(
    name = "vca",
    version = VERSION,
    about = "轻量级静默课堂录制与课后总结系统",
    long_about = None,
)]
struct Cli {
    /// 指定 profile（多教师共用时的数据隔离键）。
    #[arg(long, global = true)]
    profile: Option<String>,

    /// 覆盖配置目录。
    #[arg(long, global = true)]
    config_dir: Option<String>,

    /// 覆盖数据目录。
    #[arg(long, global = true)]
    data_dir: Option<String>,

    /// 子命令。
    #[command(subcommand)]
    command: Option<Command>,
}

/// 子命令集合。
#[derive(Debug, Subcommand)]
enum Command {
    /// 配置读写与管理。
    Config {
        /// 动作：`show` / `get` / `set` / `path`。
        action: String,
        /// 参数（键或值）。
        args: Vec<String>,
    },
    /// 时间表：自主配置或导入（策划书第 2 章）。
    Timetable {
        /// 动作：`list` / `show` / `new` / `import` / `apply` / `list-archive`。
        action: String,
        /// 参数。
        args: Vec<String>,
    },
    /// 课程表：手工/CSV/ClassIsland 导入。
    Schedule {
        /// 动作：`list` / `show` / `import` / `export` / `list-archive`。
        action: String,
        /// 参数。
        args: Vec<String>,
    },
    /// 录制控制。
    Record {
        /// 动作：`start` / `stop` / `status`。
        action: String,
    },
    /// 作业（作业状态机）查询与重试。
    Task {
        /// 动作：`list` / `status` / `retry` / `restore`。
        action: String,
        /// 作业 id。
        id: Option<String>,
    },
    /// 插件管理。
    Plugin {
        /// 动作：`list` / `info` / `install <目录>` / `remove <id>`（`enable`/`disable` 待插件调度接入）。
        action: String,
        /// 插件 id 或安装来源目录。
        target: Option<String>,
    },
    /// 本地插件市场（v1 仅本地）。
    Market {
        /// 动作：`list` / `search`。
        action: String,
    },
    /// 查看日志。
    Log {
        /// 动作：`tail`。
        action: String,
    },
    /// 导入课表/时间表。
    Import {
        /// 来源：`classisland` / `csv` / `template`。
        source: String,
        /// classisland: Default.json 路径；csv: csv 路径；template: csv|timetable。
        arg: Option<String>,
        /// 指定时间表名称（classisland 有多个时间表时使用）。
        #[arg(long)]
        timetable: Option<String>,
    },
    /// 运行调度守护进程：按课表自动录制、显隐悬浮窗、在休息窗口处理后处理（前台运行，Ctrl+C 退出）。
    Run {
        /// 课程表路径（默认取当前 profile 的 config）。
        schedule: Option<String>,
        /// 只计算并打印计划、不真正显示悬浮窗。
        #[arg(long)]
        dry_run: bool,
    },
    /// 清理：删除已到期的录像（推送成功 + 72 小时）。
    Clean {
        /// 只预览，不实际删除。
        #[arg(long)]
        dry_run: bool,
    },
    /// 环境自检。加 `--fix` 会**自动修复**能修的问题（建目录、生成配置）。
    Doctor {
        /// 自动修复可修复的问题。
        #[arg(long)]
        fix: bool,
    },
    /// 首次运行引导：自检 + 自动修复 + 逐项询问必要信息。
    Setup,
    /// 重新引导（清掉初始化标记后再跑一次 setup）。
    #[command(hide = true)]
    ResetSetup,
    /// 桌面悬浮窗（下课后 5 分钟显示，上课前 2 分钟关闭）。
    Overlay {
        /// 动作：`demo` 手动验证 / `plan` 打印今日时序 / `show` / `hide`。
        action: String,
        /// `demo` 的显示秒数；`plan` 的课程表路径。
        arg: Option<String>,
    },
    /// 内部诊断命令（`panic` 用于验证崩溃静默退出）。
    #[command(hide = true)]
    Debug {
        /// 动作：`panic` / `crash-info` / `paths` / `audio` / `record-test`。
        action: String,
    },
    /// 图形界面（推荐）：本机起一个小服务，用 Edge 以应用模式打开独立窗口。
    Gui {
        /// 端口；0 表示自动挑一个空闲端口。
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// 只起服务、不自动开窗口（排查用，会打印地址）。
        #[arg(long)]
        no_open: bool,
    },
    /// 交互式命令行界面（CLI 已存档，日常请用 `vca gui`）。
    Chat,
    /// profile 管理（多教师共用）。
    Profile {
        /// 动作：`list` / `show` / `new`。
        action: String,
        /// profile 名。
        name: Option<String>,
    },
}

fn main() -> Result<()> {
    // 先把控制台切到 UTF-8，否则中文在简中 Windows 上是乱码。
    // **必须在 hide_console_for_gui 之前**：一旦 FreeConsole，
    // 这个调用就无效了（而 CLI 模式下我们仍然需要它）。
    vca_platform::session::ensure_utf8_console();

    // 双击 exe 时用户的控制台里没有任何父进程，输出会一闪而过、
    // 看起来像「程序打不开」。现在有界面了，双击直接开界面。
    #[cfg(windows)]
    if std::env::args_os().len() <= 1 && vca_platform::session::launched_by_double_click() {
        let cli = Cli::parse();
        let (layout, _) = build_layout(&cli);
        // 界面模式：日志额外落一份文件，界面上才看得到守护进程在干什么
        std::env::set_var("VCA_UI", "1");
        // 隐藏控制台窗口：GUI 模式下那个黑框纯属碍眼。
        // 顺序很关键 —— 必须在 init_tracing **之前**做完，否则第一条日志
        // 会先写进一个马上要被藏掉的控制台里，用户瞥见半行字就没了。
        // 结果说明留到 tracing 就绪之后再记（见该函数的文档）。
        let console_note = hide_console_for_gui();
        init_tracing(&layout);
        tracing::info!("{console_note}");
        return run_gui(
            &layout,
            cli.profile.as_deref().unwrap_or("default"),
            0,
            false,
        );
    }

    let cli = Cli::parse();
    let (layout, path_source) = build_layout(&cli);

    // 界面模式（不带子命令或显式 gui）下日志要落文件 —— 让 init_tracing 知道
    let is_gui = matches!(cli.command, None | Some(Command::Gui { .. }));
    let console_note = if is_gui {
        std::env::set_var("VCA_UI", "1");
        // 同上：先藏窗口，后起日志
        Some(hide_console_for_gui())
    } else {
        None
    };
    init_tracing(&layout);
    if let Some(n) = console_note {
        tracing::info!("{n}");
    }

    // 载入本地凭据文件（secrets.env）。放在这里、且在任何命令之前：
    // 后面所有模块读的都是环境变量，少一处加载就会有一个功能悄悄降级。
    // 只补缺失的键，真环境变量优先级更高。
    let _ = vca_core::secrets::load_into_env(&vca_core::secrets::default_path(&layout.config_root));

    // profile 是外部输入（--profile），而它会被直接拼进 data/config 的目录路径。
    // 在这里统一把关：放一个 `..` 或 `C:\` 进去，作业目录就能写到根之外。
    // 校验点放在 main 而不是各子命令 —— 每个命令都要用它，分散校验必然漏掉某条路径。
    if let Some(p) = cli.profile.as_deref() {
        if let Err(e) = vca_core::paths::validate_profile_name(p) {
            anyhow::bail!("{e}");
        }
    }

    // 只在「不是便携模式」时提醒一次位置，避免每次启动都刷屏；
    // 用户需要知道自己的录像到底躺哪儿。
    if matches!(path_source, vca_core::paths::PathSource::UserFallback) {
        eprintln!(
            "注意：程序所在目录不可写，数据已放到 {}\n\
             如需换位置，用 --data-dir 指定，或运行 vca setup 重新选择。",
            layout.data_root.display()
        );
    }

    match cli.command {
        // 不带子命令时进图形界面：现在 GUI 是主形态，双击就该看到界面。
        // （CLI 仍然完整可用，用 `vca chat` 显式进入。）
        None => run_gui(
            &layout,
            cli.profile.as_deref().unwrap_or("default"),
            0,
            false,
        ),
        Some(Command::Gui { port, no_open }) => run_gui(
            &layout,
            cli.profile.as_deref().unwrap_or("default"),
            port,
            no_open,
        ),
        Some(Command::Chat) => repl::run(&layout, cli.profile.as_deref().unwrap_or("default")),
        Some(Command::Import {
            source,
            arg,
            timetable,
        }) => cmd::import(&layout, &source, arg.as_deref(), timetable.as_deref()),
        Some(Command::Run { schedule, dry_run }) => cmd::run(&layout, schedule.as_deref(), dry_run),
        Some(Command::Clean { dry_run }) => cmd::clean(&layout, dry_run),
        Some(Command::Doctor { fix }) => cmd::doctor(&layout, fix, cli.profile.as_deref()),
        Some(Command::Setup) => cmd::setup(&layout, cli.profile.as_deref()),
        Some(Command::ResetSetup) => cmd::reset_setup(&layout),
        Some(Command::Config { action, args }) => cmd::config(&layout, &action, &args),
        Some(Command::Timetable { action, args }) => cmd::timetable(&layout, &action, &args),
        Some(Command::Schedule { action, args }) => cmd::schedule(&layout, &action, &args),
        Some(Command::Record { action }) => cmd::record(&layout, &action),
        Some(Command::Task { action, id }) => cmd::task(&layout, &action, id.as_deref()),
        Some(Command::Plugin { action, target }) => {
            cmd::plugin(&layout, &action, target.as_deref())
        }
        Some(Command::Market { action }) => cmd::market(&layout, &action),
        Some(Command::Log { action }) => cmd::log(&layout, &action),
        Some(Command::Overlay { action, arg }) => cmd::overlay(&layout, &action, arg.as_deref()),
        Some(Command::Debug { action }) => cmd::debug(&layout, &action),
        Some(Command::Profile { action, name }) => cmd::profile(&layout, &action, name.as_deref()),
    }
}

/// 启动图形界面。
///
/// 在本机起一个只监听回环地址的小服务，再用 Edge 的 `--app=` 模式打开独立窗口
/// （没有地址栏和标签栏，观感上就是个原生程序）。细节见 `vca_gui` crate 文档。
fn run_gui(
    layout: &vca_core::paths::Layout,
    profile: &str,
    port: u16,
    no_open: bool,
) -> Result<()> {
    vca_gui::serve(vca_gui::ServeOptions {
        config_root: layout.config_root.clone(),
        data_root: layout.data_root.clone(),
        profile: profile.to_string(),
        open_browser: !no_open,
        port,
        web_dir: resolve_web_dir(),
    })
}

/// 找前端资源目录 —— **前后端分离的约定入口**。
///
/// 查找顺序（先命中先用）：
/// 1. `VCA_WEB_DIR` 环境变量 —— 显式指定，脚本/CI 用；
/// 2. 仓库根的 `web/` —— 从 `target/{debug,release}/vca.exe` 往上找到仓库根；
/// 3. exe 同级的 `web/` —— 发布形态：`vca.exe` 旁边放一个 `web/` 目录；
/// 4. 都没有 → `None`，用 `include_str!` 编进 exe 的内置版本。
///
/// # 为什么要有第 3 条
///
/// 发布时用户拿到的如果只有 exe，改界面就得重新编译、重新分发。
/// 有了「exe 同级 web/」，前端可以单独更新：**换掉那个目录即可**，
/// 后端一行不用动。这也正是前后端分离想要的部署形态。
///
/// # 为什么第 2 条要往上找
///
/// 开发时 `cargo run` 的 exe 在 `target/debug/` 下，前端文件在仓库根
/// 的 `web/`。不往上找的话开发者必须每次设环境变量，很容易忘。
fn resolve_web_dir() -> Option<std::path::PathBuf> {
    // 1) 环境变量
    if let Ok(p) = std::env::var("VCA_WEB_DIR") {
        if !p.trim().is_empty() {
            return Some(std::path::PathBuf::from(p));
        }
    }

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));

    // 3) exe 同级的 web/（发布形态）
    if let Some(dir) = &exe_dir {
        let sibling = dir.join("web");
        if sibling.join("index.html").is_file() {
            return Some(sibling);
        }
    }

    // 2) 从 exe 所在目录往上找仓库根（开发形态）
    if let Some(dir) = &exe_dir {
        let mut cur = Some(dir.as_path());
        // 最多往上 4 层：target/debug → target → 仓库根
        for _ in 0..4 {
            let Some(here) = cur else { break };
            let candidate = here.join("web");
            if candidate.join("index.html").is_file() {
                return Some(candidate);
            }
            cur = here.parent();
        }
    }

    // 4) 都没有：用编进 exe 的内置资源
    None
}

/// 隐藏控制台窗口（仅 GUI 模式调用），返回一句可记录的结果说明。
///
/// # 为什么返回说明而不是自己记日志
///
/// 调用点在 `init_tracing` **之前** —— 必须在第一条日志写出之前就把控制台
/// 藏掉，否则用户会看到黑框里闪半行字再消失。但那样一来，这个函数里的
/// `tracing::info!` 就没有订阅者，日志会被静默丢弃（实测过：日志页里
/// 找不到这一条，"到底隐藏成功没有"反而查不到了）。
///
/// 所以：这里只做事、返回描述，由调用方在 tracing 就绪之后再写。
///
/// # 为什么要有 `VCA_KEEP_CONSOLE` 这个后门
///
/// 隐藏控制台会让 `println!` 的输出无处可去。当用户报告
/// "启动就闪退、什么提示都没有"时，需要一个办法把控制台找回来。
fn hide_console_for_gui() -> String {
    // 用了这个开关的场景本身就是"我要看输出"，所以要明确说一句
    // "控制台是你要的，不是我们忘了隐藏"
    if std::env::var("VCA_KEEP_CONSOLE").is_ok() {
        eprintln!("[VCA] VCA_KEEP_CONSOLE=1，保留控制台窗口（仅用于排查问题）");
        return "控制台窗口：按 VCA_KEEP_CONSOLE=1 的要求保留".to_string();
    }

    let had = vca_platform::console::has_console();
    let hidden = vca_platform::console::hide();
    match (hidden, had) {
        (true, _) => "控制台窗口已隐藏（输出见本页日志）".to_string(),
        (false, true) => "控制台窗口隐藏失败，窗口仍在".to_string(),
        (false, false) => "启动时就没有控制台窗口".to_string(),
    }
}

/// 初始化日志。`RUST_LOG` 控制级别，默认 `info`。
///
/// # 三个去处
///
/// 1. **内存环形缓冲**（[`vca_platform::logbuf`]）—— 界面「日志」页读它。
///    这是 GUI 模式下最主要的去处，因为控制台窗口被隐藏了。
/// 2. **`<数据目录>/logs/ui.log`** —— 界面模式额外写一份。
///    守护进程跑在后台线程里，出问题时用户在界面上可能已经翻过了，
///    文件能留着事后查。
/// 3. **stdout** —— CLI 模式保留（`vca doctor`、`vca plugin list`
///    这些就是靠 stdout 干活的），GUI 模式下由于控制台已经隐藏，
///    这一路实际不可见，但没有副作用。
///
/// 三路是叠加的，任何一个都不是"替代"关系 —— 所以隐藏控制台不会丢日志。
fn init_tracing(layout: &vca_core::paths::Layout) {
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{fmt, EnvFilter};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    if std::env::var("VCA_UI").is_ok() {
        let path = layout.data_root.join("logs").join("ui.log");
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            // 文件 layer + 内存 layer 同时挂上。
            //
            // 用 fmt::layer() 而不是 fmt().finish()：
            // 后者产出的是 Subscriber（完整订阅者），不能当 Layer 再叠一个；
            // 前者才是可组合的 Layer。这两个混用会报
            // "FmtSubscriber: Layer<...> is not satisfied" —— 名字很像，
            // 但一个能叠一个不能。
            let _ = tracing_subscriber::registry()
                .with(filter)
                .with(
                    fmt::layer()
                        .with_target(false)
                        .with_ansi(false) // 文件里不需要颜色转义
                        .with_writer(std::sync::Mutex::new(f)),
                )
                .with(vca_platform::logbuf::layer())
                .try_init();
            return;
        }
    }

    // 非界面模式（CLI）：stdout + 内存缓冲。
    // 内存缓冲在这里看着多余，但 CLI 与界面可能同在一个进程里跑
    // （`vca gui` 启动的守护线程就属于这种情况），挂上它没有代价。
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(false))
        .with(vca_platform::logbuf::layer())
        .try_init();
}

/// 依据命令行与环境变量覆盖目录布局，并说明最终位置的来源。
///
/// 默认**不落 C 盘用户目录**：优先用程序所在目录（便携模式），
/// 其次才用用户之前选过的位置。完整优先级见 `vca_core::paths` 模块文档。
fn build_layout(cli: &Cli) -> (Layout, vca_core::paths::PathSource) {
    use vca_core::paths::PathSource;

    let env = |k: &str| {
        std::env::var(k)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };

    // 命令行最高：便于脚本化部署与多实例
    if cli.data_dir.is_some() || cli.config_dir.is_some() {
        let mut l = Layout::resolve().0;
        if let Some(d) = &cli.data_dir {
            l.data_root = std::path::PathBuf::from(d);
        }
        if let Some(c) = &cli.config_dir {
            l.config_root = std::path::PathBuf::from(c);
        }
        return (l, PathSource::Cli);
    }

    // 环境变量次之
    let (ed, ec) = (env("VCA_DATA_DIR"), env("VCA_CONFIG_DIR"));
    if ed.is_some() || ec.is_some() {
        let mut l = Layout::resolve().0;
        if let Some(d) = ed {
            l.data_root = std::path::PathBuf::from(d);
        }
        if let Some(c) = ec {
            l.config_root = std::path::PathBuf::from(c);
        }
        return (l, PathSource::Env);
    }

    // 其余交给 resolve：引导文件 → 便携 → 兜底
    Layout::resolve()
}

/// 打印产品名（供 banner 使用）。
#[allow(dead_code)]
fn product() -> &'static str {
    PRODUCT_NAME
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `VCA_WEB_DIR` 显式指定时必须优先 —— 脚本/CI 靠它指向临时前端目录。
    ///
    /// 这条测试会改进程级环境变量，所以只在一个测试里做、并在结尾还原，
    /// 避免和其它测试互相干扰（cargo 默认多线程跑测试）。
    #[test]
    fn env_var_wins_when_set() {
        let key = "VCA_WEB_DIR";
        let old = std::env::var(key).ok();

        std::env::set_var(key, "D:\\some\\custom\\web");
        let got = resolve_web_dir();
        assert_eq!(
            got.as_deref(),
            Some(std::path::Path::new("D:\\some\\custom\\web")),
            "设了 VCA_WEB_DIR 却不是它优先"
        );

        // 空字符串要当成"没设"，否则会得到一个空路径去 join，行为很怪
        std::env::set_var(key, "   ");
        let got_empty = resolve_web_dir();
        assert_ne!(
            got_empty.as_deref(),
            Some(std::path::Path::new("   ")),
            "空白 VCA_WEB_DIR 被当成了有效路径"
        );

        match old {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }

    /// 仓库根真的有 `web/index.html` —— 这是前后端分离的落点，
    /// 删掉/改名了前端就静默退回内置版本（表现为"改了没效果"）。
    #[test]
    fn repo_has_web_dir_at_root() {
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        // crates/vca-cli → 仓库根
        let root = here
            .parent()
            .and_then(|p| p.parent())
            .expect("找不到仓库根");
        let index = root.join("web").join("index.html");
        assert!(
            index.is_file(),
            "仓库根没有 web/index.html（{}）—— 前端资源目录丢失了",
            index.display()
        );
    }

    /// 内置兜底资源必须齐全：exe 单独被拷走时全靠它们。
    #[test]
    fn builtin_assets_are_not_empty() {
        for (name, body) in vca_gui::web::BUILTIN {
            assert!(!body.trim().is_empty(), "内置资源 {name} 是空的");
        }
    }
}
