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
    init_tracing();

    // 双击 exe 时用户的控制台里没有任何父进程，输出会一闪而过、
    // 看起来像「程序打不开」。这里检测这种情况并改走交互菜单。
    #[cfg(windows)]
    if std::env::args_os().len() <= 1 && vca_platform::session::launched_by_double_click() {
        return interactive_menu();
    }

    let cli = Cli::parse();
    let layout = build_layout(&cli);

    match cli.command {
        None => cmd::print_banner(&layout, cli.profile.as_deref()),
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

/// 双击启动时的交互菜单。
///
/// 目的：让不懂命令行的老师也能用双击 exe 不再是一闪而过，
/// 而是一个能看懂的中文菜单。
#[cfg(windows)]
fn interactive_menu() -> Result<()> {
    use std::io::Write;

    loop {
        println!();
        println!("================================================================");
        println!("  VibeClassAgent  静默课堂录制与课后总结系统");
        println!("================================================================");
        println!();
        println!("  1  首次引导向导（**第一次使用请先跑这个**）");
        println!("  2  环境自检 + 自动修复");
        println!("  3  显示悬浮窗（看看效果，8 秒后消失）");
        println!("  4  录制测试（录 10 秒，验证录屏录音是否正常）");
        println!("  5  从 ClassIsland 导入课表");
        println!("  6  预览今天的悬浮窗时序");
        println!("  7  试运行守护进程（不真正录制 / 不真正推送）");
        println!("  8  正式运行守护进程");
        println!("  9  预览录像清理（不会真的删除）");
        println!("  d  打开文档目录");
        println!("  0  退出");
        println!();
        print!("请输入序号后回车：");

        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            return Ok(());
        }
        let choice = line.trim();
        println!();

        let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("vca"));
        let run = |args: &[&str]| -> Result<()> {
            let st = std::process::Command::new(&exe).args(args).status();
            match st {
                Ok(s) if s.success() => {}
                Ok(s) => println!("\n[命令结束，退出码 {:?}]", s.code()),
                Err(e) => println!("\n[执行失败：{e}]"),
            }
            Ok(())
        };

        let _ = match choice {
            "1" => run(&["setup"]),
            "2" => run(&["doctor", "--fix"]),
            "3" => run(&["overlay", "demo", "8"]),
            "4" => run(&["debug", "record-test"]),
            "5" => run(&["import", "classisland", ""]),
            "6" => run(&["overlay", "plan"]),
            "7" => run(&["run", "--dry-run"]),
            "8" => run(&["run"]),
            "9" => run(&["clean", "--dry-run"]),
            "d" | "D" => {
                let d = std::path::PathBuf::from("docs");
                let d = if d.is_dir() {
                    d
                } else {
                    std::path::PathBuf::from("../docs")
                };
                println!("文档目录：{}", d.display());
                let _ = std::process::Command::new("explorer").arg(&d).spawn();
                Ok(())
            }
            "0" | "q" | "quit" | "exit" => {
                println!("再见。");
                return Ok(());
            }
            "" => continue,
            other => {
                println!("无效的序号：{other}");
                Ok(())
            }
        };
    }
}

/// 初始化日志。`RUST_LOG` 控制级别，默认 `info`。
fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = fmt().with_env_filter(filter).with_target(false).try_init();
}

/// 依据命令行覆盖项构造目录布局。
fn build_layout(cli: &Cli) -> Layout {
    let mut layout = Layout::default_windows();
    if let Some(d) = &cli.data_dir {
        layout.data_root = std::path::PathBuf::from(d);
    }
    if let Some(c) = &cli.config_dir {
        layout.config_root = std::path::PathBuf::from(c);
    }
    layout
}

/// 打印产品名（供 banner 使用）。
#[allow(dead_code)]
fn product() -> &'static str {
    PRODUCT_NAME
}
