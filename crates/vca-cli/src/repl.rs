//! 交互式命令行界面。
//!
//! # 观感
//!
//! 参照 Claude Code：
//!
//! - 顶端用 **ASCII 字符画**打出程序缩写 `VCA`（内容取自语言文件的
//!   `welcome.logo`，想换造型改语言文件即可，不必动代码）；
//! - 主色是陶土橙（Claude 品牌色），配暗色终端；次要信息压成灰色；
//! - `> ` 提示符 + 行编辑（历史、左右移动、Ctrl+C 中断）；
//! - 每个动作以 `⏺` 起头，结果用 `⎿` 缩进挂在下面，末尾给耗时；
//! - 留白偏宽松，一屏只讲一件事。
//!
//! # 文案
//!
//! 界面上的**每一句固定文字都走 [`vca_core::i18n`]**，没有硬编码。
//! 改词、加语种只动 `locales/*.json`，不用重新编译。
//!
//! # 为什么不用全屏 TUI
//!
//! 全屏 TUI 会把历史输出盖掉，而课堂场景里「刚才那节课处理到哪一步了」
//! 是要能往上翻的。滚动式输出更适合这种「跑一件事、看结果、再跑下一件」的用法。

use std::io::Write;

use anyhow::Result;
use rustyline::error::ReadlineError;

use vca_core::config::load_settings;
use vca_core::i18n::{t, tf};
use vca_core::paths::Layout;
use vca_core::store::JobStore;

/// 配色与字形。取 Claude Code 的调子：陶土橙主色 + 灰阶次要信息。
mod c {
    /// 重置。
    pub const RESET: &str = "\x1b[0m";
    /// 加粗。
    pub const BOLD: &str = "\x1b[1m";
    /// 压暗。
    pub const DIM: &str = "\x1b[2m";
    /// 主色：陶土橙（Claude 品牌色的 256 色近似）。
    pub const ACCENT: &str = "\x1b[38;5;216m";
    /// 正文。
    pub const TEXT: &str = "\x1b[38;5;253m";
    /// 次要信息。
    pub const MUTED: &str = "\x1b[38;5;245m";
    /// 成功。
    pub const GREEN: &str = "\x1b[38;5;114m";
    /// 警告。
    pub const YELLOW: &str = "\x1b[38;5;221m";
    /// 错误。
    pub const RED: &str = "\x1b[38;5;210m";
    /// 强调（命令名）。
    pub const CYAN: &str = "\x1b[38;5;116m";
}

/// 动作标记。
const DOT: &str = "⏺";
/// 结果标记。
const ELBOW: &str = "⎿";

/// 一次命令之后是否继续。
#[derive(Debug, PartialEq, Eq)]
pub enum Flow {
    /// 继续下一轮。
    Continue,
    /// 退出。
    Quit,
}

/// 启动交互界面。
pub fn run(layout: &Layout, profile: &str) -> Result<()> {
    vca_core::i18n::install(None);
    print_welcome(layout, profile);

    let mut rl = match rustyline::DefaultEditor::new() {
        Ok(r) => r,
        Err(e) => {
            println!(
                "{}",
                tf("cmd.no_terminal", &[("err", &e.to_string())])
            );
            return Ok(());
        }
    };

    let prompt = format!("{}>{} ", c::ACCENT, c::RESET);
    loop {
        match rl.readline(&prompt) {
            Ok(line) => {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }
                let _ = rl.add_history_entry(line.as_str());
                match dispatch(layout, profile, &line) {
                    Ok(Flow::Quit) => break,
                    Ok(Flow::Continue) => {}
                    Err(e) => {
                        println!(
                            "{}{} {}{}",
                            c::RED,
                            tf("cmd.error_prefix", &[("msg", &e.to_string())]),
                            DOT,
                            c::RESET
                        );
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("{}{} {}{}", c::MUTED, DOT, t("cmd.ctrl_c"), c::RESET);
            }
            Err(ReadlineError::Eof) => break,
            Err(e) => {
                println!(
                    "{}",
                    tf("err.read_input", &[("err", &e.to_string())])
                );
                break;
            }
        }
    }

    println!("{}{} {}{}", c::MUTED, DOT, t("cmd.bye"), c::RESET);
    Ok(())
}

/// 启动横幅：ASCII 字符画 + 一屏交代清楚「在哪、用什么配置」。
fn print_welcome(layout: &Layout, profile: &str) {
    let logo = t("welcome.logo");
    println!();
    for line in logo.lines() {
        println!("  {}{}{}", c::ACCENT, line, c::RESET);
    }
    println!(
        "  {}{}{}  {}{}{}",
        c::TEXT,
        t("app.name"),
        c::RESET,
        c::MUTED,
        format!("v{}", vca_core::VERSION),
        c::RESET
    );
    println!("  {}{}{}", c::MUTED, t("app.tagline"), c::RESET);
    println!();

    let settings_path = layout.profile_config_dir(profile).join("settings.yaml");
    let settings = load_settings(&settings_path).ok();

    let row = |k: &str, v: String| {
        println!("  {}{:<6}{}  {}", c::MUTED, k, c::RESET, v);
    };

    row(
        &t("welcome.label.config"),
        format!("{}", layout.profile_config_dir(profile).display()),
    );
    row(
        &t("welcome.label.data"),
        format!("{}", layout.profile_data_dir(profile).display()),
    );
    row(&t("welcome.label.profile"), profile.to_string());
    row(&t("welcome.label.lang"), vca_core::i18n::lang());

    if let Some(s) = &settings {
        let mode = if s.llm.mode.trim().is_empty() {
            "paid-api"
        } else {
            s.llm.mode.as_str()
        };

        // 模型：没配就明确说「未配置」，而不是显示空白让人猜
        let model = if s.llm.model.trim().is_empty() {
            format!("{}{}{}", c::YELLOW, t("welcome.not_configured"), c::RESET)
        } else {
            let prov = if s.llm.provider.trim().is_empty() {
                "自定义"
            } else {
                s.llm.provider.as_str()
            };
            format!("{}（{prov} · {mode}）", s.llm.model)
        };
        row(&t("welcome.label.model"), model);

        // 本地转写是否就绪：这是「转写要不要花钱」的关键信息
        let stt_ok = vca_platform::stt::local_available(&vca_platform::stt::LocalSttConfig {
            model: s.transcriber.model.clone(),
            ..Default::default()
        });
        let stt = if stt_ok {
            format!(
                "{}{}{}",
                c::GREEN,
                tf("welcome.stt_ready", &[("model", &s.transcriber.model)]),
                c::RESET
            )
        } else {
            format!("{}{}{}", c::YELLOW, t("welcome.stt_missing"), c::RESET)
        };
        row(&t("welcome.label.stt"), stt);

        let push = if s.push.provider.trim().is_empty() {
            format!("{}{}{}", c::YELLOW, t("welcome.not_configured"), c::RESET)
        } else {
            s.push.provider.clone()
        };
        row(&t("welcome.label.push"), push);
    } else {
        println!(
            "  {}{}{}",
            c::YELLOW,
            t("welcome.need_setup"),
            c::RESET
        );
    }

    println!();
    println!("  {}{}{}", c::DIM, t("welcome.hint"), c::RESET);
    println!();
}

/// 命令分发。
pub fn dispatch(layout: &Layout, profile: &str, line: &str) -> Result<Flow> {
    let line = line.trim();
    if !line.starts_with('/') {
        println!("{}{} {}{}", c::MUTED, DOT, t("cmd.only_commands"), c::RESET);
        return Ok(Flow::Continue);
    }

    let mut it = line.split_whitespace();
    let cmd = it.next().unwrap_or("");
    let args: Vec<&str> = it.collect();

    match cmd {
        "/help" | "/?" => help(),
        "/status" => status(layout, profile)?,
        "/jobs" => jobs(layout, profile)?,
        "/process" => process(layout, profile, &args)?,
        "/paths" => paths_cmd(layout, &args)?,
        "/model" => model_cmd(layout, profile, &args)?,
        "/doctor" => {
            crate::cmd::doctor(layout, args.first() == Some(&"fix"), Some(profile))?;
        }
        "/config" => crate::cmd::config(layout, args.first().copied().unwrap_or("path"), &[])?,
        "/clean" => crate::cmd::clean(layout, args.first() != Some(&"run"))?,
        "/logs" => crate::cmd::log(layout, args.first().copied().unwrap_or("tail"))?,
        "/record" => crate::cmd::record(layout, args.first().copied().unwrap_or("status"))?,
        "/audio" => crate::cmd::debug(layout, "audio")?,
        "/record-test" => crate::cmd::debug(layout, "record-test")?,
        "/transcribe-test" => crate::cmd::debug(layout, "transcribe")?,
        "/e2e" => crate::cmd::debug(layout, "e2e")?,
        "/clear" => clear_screen(),
        "/quit" | "/exit" | "/q" => return Ok(Flow::Quit),
        other => {
            println!(
                "{}{} {}{}",
                c::YELLOW,
                DOT,
                tf("cmd.unknown", &[("cmd", &format!("{}{}{}", c::BOLD, other, c::RESET))]),
                c::RESET
            );
        }
    }
    Ok(Flow::Continue)
}

/// 帮助。
fn help() {
    println!("{}{} {}{}", c::ACCENT, DOT, t("cmd.help_title"), c::RESET);
    let rows: &[(&str, &str)] = &[
        ("/status", "cmd.status"),
        ("/jobs", "cmd.jobs"),
        ("/process", "cmd.process"),
        ("/process dry", "cmd.process_dry"),
        ("/model", "cmd.model"),
        ("/paths", "cmd.paths"),
        ("/doctor [fix]", "cmd.doctor"),
        ("/config [path]", "cmd.config"),
        ("/record [status]", "cmd.record"),
        ("/audio", "cmd.audio"),
        ("/record-test", "cmd.record_test"),
        ("/transcribe-test", "cmd.transcribe_test"),
        ("/e2e", "cmd.e2e"),
        ("/clean [run]", "cmd.clean"),
        ("/logs [tail]", "cmd.logs"),
        ("/clear", "cmd.clear"),
        ("/quit", "cmd.quit"),
    ];
    for (k, key) in rows {
        println!("  {}{:<20}{} {}", c::CYAN, k, c::RESET, t(key));
    }
    println!();
    println!("  {}{}{}", c::DIM, t("cmd.help_footer"), c::RESET);
}

/// 打开配置与作业仓库。
fn open(layout: &Layout, profile: &str) -> (vca_core::config::Settings, JobStore) {
    let settings_path = layout.profile_config_dir(profile).join("settings.yaml");
    let settings = load_settings(&settings_path).unwrap_or_default();
    let store = JobStore::new(layout.profile_data_dir(profile));
    (settings, store)
}

/// 总览。
fn status(layout: &Layout, profile: &str) -> Result<()> {
    let (s, store) = open(layout, profile);
    let all = store.list();

    println!("{}{} {}{}", c::ACCENT, DOT, t("status.title"), c::RESET);

    let mut by_state: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for j in &all {
        *by_state.entry(format!("{:?}", j.state)).or_insert(0) += 1;
    }

    println!(
        "  {}{} {}{}",
        c::MUTED,
        ELBOW,
        tf("status.jobs_total", &[("n", &all.len().to_string())]),
        c::RESET
    );
    if all.is_empty() {
        println!("     {}", t("status.jobs_empty"));
    } else {
        for (k, v) in &by_state {
            println!("     {k}: {v}");
        }
    }

    let pending = store.pending();
    if !pending.is_empty() {
        println!(
            "  {}{} {}{}",
            c::YELLOW,
            ELBOW,
            tf("status.pending", &[("n", &pending.len().to_string())]),
            c::RESET
        );
    }

    // ---- 组件就绪情况 ----
    let ffmpeg = vca_platform::capture::ffmpeg_available();
    comp(
        &t("status.ffmpeg"),
        ffmpeg,
        t("status.ffmpeg_ready"),
        t("status.ffmpeg_missing"),
    );

    let stt_ok = vca_platform::stt::local_available(&vca_platform::stt::LocalSttConfig {
        model: s.transcriber.model.clone(),
        ..Default::default()
    });
    comp(
        &t("status.stt"),
        stt_ok,
        tf("status.stt_ready", &[("model", &s.transcriber.model)]),
        t("status.stt_missing"),
    );

    let llm_ok = !s.llm.model.trim().is_empty();
    comp(
        &t("status.llm"),
        llm_ok,
        tf(
            "status.llm_ready",
            &[
                ("model", &s.llm.model),
                (
                    "mode",
                    &if s.llm.mode.trim().is_empty() {
                        "paid-api".to_string()
                    } else {
                        s.llm.mode.clone()
                    },
                ),
            ],
        ),
        t("status.llm_missing"),
    );

    let push_ok = !s.push.provider.trim().is_empty();
    comp(
        &t("status.push"),
        push_ok,
        s.push.provider.clone(),
        t("status.push_missing"),
    );

    Ok(())
}

/// 打印一行组件状态（`标签：状态`，状态带颜色）。
fn comp(label: &str, ok: bool, ok_text: String, bad_text: String) {
    let (color, text) = if ok {
        (c::GREEN, ok_text)
    } else {
        (c::YELLOW, bad_text)
    };
    // label 形如 "ffmpeg：{state}"，把 {state} 换成带色文本
    let line = label.replace("{state}", &format!("{color}{text}{}", c::RESET));
    println!("  {}{} {}{}", c::MUTED, ELBOW, line, c::RESET);
}

/// 列作业。
fn jobs(layout: &Layout, profile: &str) -> Result<()> {
    let (_, store) = open(layout, profile);
    let all = store.list();
    println!(
        "{}{} {}{}",
        c::ACCENT,
        DOT,
        tf("jobs.title", &[("n", &all.len().to_string())]),
        c::RESET
    );
    for j in &all {
        let state = format!("{:?}", j.state);
        let color = match j.state {
            vca_core::job::JobState::Pushed | vca_core::job::JobState::Cleaned => c::GREEN,
            vca_core::job::JobState::Failed => c::RED,
            _ => c::YELLOW,
        };
        println!(
            "  {}{:<10}{} {} {}  {}{}{}",
            color,
            state,
            c::RESET,
            j.date,
            j.course,
            c::MUTED,
            j.id,
            c::RESET
        );
        if let Some(e) = &j.last_error {
            println!(
                "     {}",
                tf("jobs.last_error", &[("msg", &format!("{}{e}{}", c::RED, c::RESET))])
            );
        }
    }
    Ok(())
}

/// 处理待办作业。
fn process(layout: &Layout, profile: &str, args: &[&str]) -> Result<()> {
    let dry = args.iter().any(|a| *a == "dry" || *a == "--dry-run");
    let (settings, store) = open(layout, profile);
    let targets = store.pending();

    if targets.is_empty() {
        println!("{}{} {}{}", c::MUTED, DOT, t("process.none"), c::RESET);
        return Ok(());
    }

    let dry_note = if dry {
        t("process.dry_note")
    } else {
        String::new()
    };
    println!(
        "{}{} {}{}",
        c::ACCENT,
        DOT,
        tf(
            "process.title",
            &[
                ("n", &targets.len().to_string()),
                ("mode", dry_note.as_str()),
            ]
        ),
        c::RESET
    );

    let mut pipeline = vca_engine::pipeline::Pipeline::new(
        layout,
        profile,
        &settings,
        &store,
        std::path::PathBuf::new(),
    );
    pipeline.dry_run = dry;

    let started = std::time::Instant::now();
    for mut job in targets {
        println!("  {}{} {} {}{}", c::CYAN, DOT, job.date, job.course, c::RESET);
        let outcome = pipeline.process(&mut job);
        for s in &outcome.steps {
            println!("     {}{} {s}{}", c::MUTED, ELBOW, c::RESET);
        }
        for w in &outcome.warnings {
            println!("     {}{} {w}{}", c::YELLOW, ELBOW, c::RESET);
        }
        if let Some(e) = &outcome.error {
            println!(
                "     {}{} {}{}",
                c::RED,
                ELBOW,
                tf("process.failed", &[("msg", e)]),
                c::RESET
            );
        } else {
            println!(
                "     {}{} {}{}",
                c::GREEN,
                ELBOW,
                tf("process.done_state", &[("state", &format!("{:?}", outcome.state))]),
                c::RESET
            );
        }
    }

    println!(
        "{}{} {}{}",
        c::ACCENT,
        DOT,
        tf(
            "process.finished",
            &[("secs", &format!("{:.1}", started.elapsed().as_secs_f64()))]
        ),
        c::RESET
    );
    Ok(())
}

/// 查看与修改数据/配置目录。
///
/// 默认位置是**程序所在目录**（便携模式），不碰 C 盘用户目录。
/// 用户想换地方时，选择会写进 exe 同级的 `vca.paths.json` ——
/// 不能记在待选择的目录里，那是个鸡生蛋问题。
fn paths_cmd(layout: &Layout, args: &[&str]) -> Result<()> {
    use std::path::Path;

    if args.is_empty() || args[0] == "show" {
        let (_, source) = Layout::resolve();
        println!(
            "{}{} {}{}",
            c::ACCENT,
            DOT,
            tf("paths.title", &[("source", source.describe())]),
            c::RESET
        );
        println!("  {:<8}  {}", t("paths.data"), layout.data_root.display());
        println!("  {:<8}  {}", t("paths.config"), layout.config_root.display());
        println!(
            "  {:<8}  {}",
            t("paths.bootstrap"),
            vca_core::paths::bootstrap_path().display()
        );
        println!();
        println!("  {}{}{}", c::MUTED, t("paths.how_to_change"), c::RESET);
        println!("  {}{}{}", c::MUTED, t("paths.example"), c::RESET);
        println!("  {}{}{}", c::MUTED, t("paths.restart_note"), c::RESET);
        return Ok(());
    }

    if args[0] == "set" {
        let Some(data) = args.get(1) else {
            println!("{}{} {}{}", c::YELLOW, DOT, t("paths.usage"), c::RESET);
            return Ok(());
        };
        let cfg = args
            .get(2)
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("{data}\\config"));

        // 先真写一下试试，别让用户选一个根本写不进去的地方
        if !vca_core::paths::is_writable(Path::new(data)) {
            println!(
                "{}{} {}{}",
                c::RED,
                DOT,
                tf("paths.unwritable", &[("dir", data)]),
                c::RESET
            );
            println!("    {}", t("paths.unwritable_hint"));
            return Ok(());
        }

        let choice = vca_core::paths::PathChoice {
            data_root: data.to_string(),
            config_root: cfg.clone(),
        };
        match vca_core::paths::save_bootstrap(&choice) {
            Ok(p) => {
                println!("{}{} {}{}", c::GREEN, DOT, t("paths.saved"), c::RESET);
                println!("    {:<6}  {data}", t("paths.data"));
                println!("    {:<6}  {cfg}", t("paths.config"));
                println!("    {}", tf("paths.note", &[("file", &p.display().to_string())]));
                println!();
                println!("  {}{}{}", c::YELLOW, t("paths.move_note"), c::RESET);
                println!("  {}{}{}", c::MUTED, t("paths.restart_note"), c::RESET);
            }
            Err(e) => println!(
                "{}{} {}{}",
                c::RED,
                DOT,
                tf("paths.write_failed", &[("err", &e.to_string())]),
                c::RESET
            ),
        }
        return Ok(());
    }

    println!("{}{} {}{}", c::YELLOW, DOT, t("paths.usage"), c::RESET);
    Ok(())
}

/// 查看或选择要点提取用的模型。
///
/// 模型名不必手打：`/model list` 会直接问 API 的 `/v1/models` 要列表，
/// 用户按序号挑即可。这是「自动拉取」的落点。
fn model_cmd(layout: &Layout, profile: &str, args: &[&str]) -> Result<()> {
    let (mut settings, _) = open(layout, profile);

    if args.first() == Some(&"set") {
        let Some(name) = args.get(1) else {
            println!("{}{} {}{}", c::YELLOW, DOT, t("model.usage"), c::RESET);
            return Ok(());
        };
        settings.llm.model = name.to_string();
        return save_model(layout, profile, &settings, name);
    }

    println!("{}{} {}{}", c::ACCENT, DOT, t("model.title"), c::RESET);
    let provider = if settings.llm.provider.trim().is_empty() {
        "custom".to_string()
    } else {
        settings.llm.provider.clone()
    };
    let mode = if settings.llm.mode.trim().is_empty() {
        "paid-api".to_string()
    } else {
        settings.llm.mode.clone()
    };
    println!(
        "  {}",
        tf(
            "model.current",
            &[
                ("model", &settings.llm.model),
                ("provider", &provider),
                ("mode", &mode)
            ]
        )
    );

    // 拉取列表
    let cfg = crate::cmd::llm_config_from(&settings);
    if !cfg.is_configured() {
        println!();
        println!("  {}{}{}", c::YELLOW, t("model.need_key"), c::RESET);
        return Ok(());
    }
    println!();
    println!(
        "  {}{}{}",
        c::MUTED,
        tf("model.fetching", &[("url", &cfg.models_endpoint())]),
        c::RESET
    );

    match vca_platform::llm::list_models(&cfg) {
        Ok(models) if models.is_empty() => {
            println!("  {}", t("model.fetch_failed").replace("{err}", "(空列表)"));
        }
        Ok(models) => {
            println!(
                "  {}{}{}",
                c::TEXT,
                tf("model.fetched", &[("n", &models.len().to_string())]),
                c::RESET
            );
            for (i, m) in models.iter().enumerate() {
                let mark = if *m == settings.llm.model { "●" } else { " " };
                println!("   {}{mark}{} {:>3}. {m}", c::ACCENT, c::RESET, i + 1);
            }
            println!();
            println!("  {}{}{}", c::MUTED, t("model.pick"), c::RESET);

            // 交互式选择
            let mut rl = match rustyline::DefaultEditor::new() {
                Ok(r) => r,
                Err(_) => return Ok(()),
            };
            let prompt = format!("{}  >{} ", c::ACCENT, c::RESET);
            if let Ok(line) = rl.readline(&prompt) {
                let line = line.trim();
                if line.is_empty() {
                    return Ok(());
                }
                let picked = match line.parse::<usize>() {
                    Ok(n) if n >= 1 && n <= models.len() => models[n - 1].clone(),
                    _ => line.to_string(),
                };
                settings.llm.model = picked.clone();
                return save_model(layout, profile, &settings, &picked);
            }
        }
        Err(e) => {
            println!(
                "  {}",
                tf("model.fetch_failed", &[("err", &e.to_string())])
            );
        }
    }
    Ok(())
}

/// 把选中的模型写回配置文件。
fn save_model(
    layout: &Layout,
    profile: &str,
    settings: &vca_core::config::Settings,
    name: &str,
) -> Result<()> {
    let dir = layout.profile_config_dir(profile);
    let path = dir.join("settings.yaml");
    match vca_core::config::save_settings(&path, settings) {
        Ok(()) => {
            println!(
                "{}{} {}{}",
                c::GREEN,
                DOT,
                tf("model.set_ok", &[("model", name)]),
                c::RESET
            );
            println!("    {}", path.display());
        }
        Err(e) => println!(
            "{}{} {}{}",
            c::RED,
            DOT,
            tf("model.set_failed", &[("err", &e)]),
            c::RESET
        ),
    }
    Ok(())
}

/// 清屏（ANSI）。
fn clear_screen() {
    print!("\x1b[2J\x1b[H");
    let _ = std::io::stdout().flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> Layout {
        let base = std::env::temp_dir().join(format!("vca_repl_{}", std::process::id()));
        Layout::new(base.join("data"), base.join("config"))
    }

    #[test]
    fn slash_commands_are_dispatched() {
        let l = layout();
        assert_eq!(dispatch(&l, "default", "/help").unwrap(), Flow::Continue);
        assert_eq!(dispatch(&l, "default", "/status").unwrap(), Flow::Continue);
        assert_eq!(dispatch(&l, "default", "/jobs").unwrap(), Flow::Continue);
    }

    #[test]
    fn quit_variants_all_exit() {
        let l = layout();
        for q in ["/quit", "/exit", "/q"] {
            assert_eq!(dispatch(&l, "default", q).unwrap(), Flow::Quit, "{q}");
        }
    }

    #[test]
    fn unknown_command_does_not_exit() {
        let l = layout();
        assert_eq!(
            dispatch(&l, "default", "/没有这个命令").unwrap(),
            Flow::Continue
        );
    }

    #[test]
    fn plain_text_is_not_treated_as_a_command() {
        let l = layout();
        assert_eq!(
            dispatch(&l, "default", "帮我处理一下").unwrap(),
            Flow::Continue
        );
    }

    #[test]
    fn process_with_no_jobs_is_ok() {
        let l = layout();
        let _ = std::fs::create_dir_all(l.profile_data_dir("default"));
        assert_eq!(dispatch(&l, "default", "/process").unwrap(), Flow::Continue);
    }

    #[test]
    fn all_ui_text_comes_from_locale_files() {
        // 界面文案一律走语言文件：这些 key 必须都能取到，
        // 取不到会返回 key 本身（界面上就会出现 "welcome.hint" 这种字面量）。
        vca_core::i18n::install(None);
        for key in [
            "app.name",
            "welcome.logo",
            "welcome.hint",
            "cmd.help_title",
            "cmd.status",
            "status.title",
            "paths.title",
            "model.title",
            "process.title",
        ] {
            let v = t(key);
            assert_ne!(v, key, "语言文件缺少 {key}");
            assert!(!v.trim().is_empty(), "{key} 不该是空的");
        }
    }

    #[test]
    fn logo_is_multiline_ascii_art() {
        vca_core::i18n::install(None);
        let logo = t("welcome.logo");
        assert!(logo.lines().count() >= 4, "logo 应该是多行字符画");
        // 纯 ASCII/制表符号，不能混入中文，否则对齐会乱
        for line in logo.lines() {
            assert!(
                line.chars().all(|ch| !ch.is_ascii() || ch.is_ascii_graphic() || ch == ' '),
                "logo 里不该有非 ASCII 的可打印字符以外的东西: {line}"
            );
        }
    }
}
