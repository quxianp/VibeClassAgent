//! # vca-cli
//!
//! 命令行入口。对应策划书 5.6 的命令集合：
//! `config` / `timetable` / `schedule` / `record` / `task` / `plugin` / `market` /
//! `log` / `doctor` / `profile`。
//!
//! 已实现：doctor / config / overlay / run / clean / plugin / import / debug。
//! 预留：timetable / schedule / record / task / market / log / profile。

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
    // 先把控制台切到 UTF-8，否则中文在简中 Windows 上是乱码
    vca_platform::session::ensure_utf8_console();

    // 双击 exe 时用户的控制台里没有任何父进程，输出会一闪而过、
    // 看起来像「程序打不开」。现在有界面了，双击直接开界面。
    #[cfg(windows)]
    if std::env::args_os().len() <= 1 && vca_platform::session::launched_by_double_click() {
        let cli = Cli::parse();
        let (layout, _) = build_layout(&cli);
        // 界面模式：日志额外落一份文件，界面上才看得到守护进程在干什么
        std::env::set_var("VCA_UI", "1");
        init_tracing(&layout);
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
    if matches!(cli.command, None | Some(Command::Gui { .. })) {
        std::env::set_var("VCA_UI", "1");
    }
    init_tracing(&layout);

    // 载入本地凭据文件（secrets.env）。放在这里、且在任何命令之前：
    // 后面所有模块读的都是环境变量，少一处加载就会有一个功能悄悄降级。
    // 只补缺失的键，真环境变量优先级更高。
    let _ = vca_core::secrets::load_into_env(&vca_core::secrets::default_path(&layout.config_root));

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
        // 开发时把 VCA_WEB_DIR 指向 assets/web 就能改页面即时看到效果
        web_dir: std::env::var("VCA_WEB_DIR")
            .ok()
            .map(std::path::PathBuf::from),
    })
}

/// 初始化日志。`RUST_LOG` 控制级别，默认 `info`。
///
/// 界面模式（`VCA_UI=1`）下额外写一份 `<数据目录>/logs/ui.log`：
/// 守护进程跑在后台线程里，它的输出如果只走 stdout，用户在界面上
/// 就完全看不到"它到底在干什么" —— 而看不到的东西最容易让人以为坏了。
fn init_tracing(layout: &vca_core::paths::Layout) {
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
            let _ = fmt()
                .with_env_filter(filter)
                .with_target(false)
                .with_ansi(false) // 文件里不需要颜色转义
                .with_writer(std::sync::Mutex::new(f))
                .try_init();
            return;
        }
    }

    let _ = fmt().with_env_filter(filter).with_target(false).try_init();
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
