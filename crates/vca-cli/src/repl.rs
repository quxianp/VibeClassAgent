//! 交互式命令行界面。
//!
//! # 观感参照 Claude Code
//!
//! - 启动时一个方框，把「我在哪、我用什么配置」一次讲清；
//! - `> ` 提示符 + 行编辑（历史、左右移动、Ctrl+C 中断）；
//! - 斜杠命令（`/status` `/process` …），输入 `/` 有补全提示；
//! - 每个动作以 `⏺` 起头，结果用 `⎿` 缩进挂在下面，末尾给耗时；
//! - 全部输出走 ANSI 颜色，深色终端下可读。
//!
//! # 为什么不用全屏 TUI
//!
//! 全屏 TUI（ratatui 那类）会把历史输出盖掉，而课堂场景里
//! 「刚才那节课处理到哪一步了」是要能往上翻的。Claude Code 那种
//! 「滚动式对话 + 行内结果」更适合这种「跑一件事、看结果、再跑下一件」的用法。

use std::io::Write;

use anyhow::Result;
use rustyline::error::ReadlineError;

use vca_core::config::load_settings;
use vca_core::paths::Layout;
use vca_core::store::JobStore;

/// ANSI 颜色。Windows 10 以后的终端都支持。
mod c {
    /// 重置。
    pub const RESET: &str = "\x1b[0m";
    /// 暗色。
    pub const DIM: &str = "\x1b[2m";
    /// 加粗。
    pub const BOLD: &str = "\x1b[1m";
    /// 品牌蓝。
    pub const BLUE: &str = "\x1b[38;5;39m";
    /// 成功绿。
    pub const GREEN: &str = "\x1b[38;5;41m";
    /// 警告黄。
    pub const YELLOW: &str = "\x1b[38;5;214m";
    /// 错误红。
    pub const RED: &str = "\x1b[38;5;203m";
    /// 次要灰。
    pub const GRAY: &str = "\x1b[38;5;245m";
    /// 强调青。
    pub const CYAN: &str = "\x1b[38;5;44m";
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
    print_welcome(layout, profile);

    let mut rl = match rustyline::DefaultEditor::new() {
        Ok(r) => r,
        Err(e) => {
            // 没有可用的终端（比如被重定向）时不要直接崩，
            // 给一条能看懂的提示并且不要卡住。
            println!("{}无法启动交互输入（{e}）。改用子命令方式：vca --help{}", c::YELLOW, c::RESET);
            return Ok(());
        }
    };

    let prompt = format!("{}>{} ", c::BLUE, c::RESET);
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
                        println!("{}{} 出错了：{e}{}", c::RED, DOT, c::RESET);
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("{}(Ctrl+C) 输入 /quit 退出，/help 看命令{}", c::GRAY, c::RESET);
            }
            Err(ReadlineError::Eof) => break,
            Err(e) => {
                println!("{}输入读取失败：{e}{}", c::RED, c::RESET);
                break;
            }
        }
    }

    println!("{}{} 已退出。{}", c::GRAY, DOT, c::RESET);
    Ok(())
}

/// 启动横幅。
fn print_welcome(layout: &Layout, profile: &str) {
    let ver = env!("CARGO_PKG_VERSION");
    println!();
    println!(
        "{}╭──────────────────────────────────────────────────────╮{}",
        c::BLUE,
        c::RESET
    );
    println!(
        "{}│{}  {}VibeClassAgent{}  {}v{ver}{}                              {}│{}",
        c::BLUE,
        c::RESET,
        c::BOLD,
        c::RESET,
        c::GRAY,
        c::RESET,
        c::BLUE,
        c::RESET
    );
    println!(
        "{}│{}  {}静默课堂录制与课后总结{}                            {}│{}",
        c::BLUE,
        c::RESET,
        c::DIM,
        c::RESET,
        c::BLUE,
        c::RESET
    );
    println!(
        "{}╰──────────────────────────────────────────────────────╯{}",
        c::BLUE,
        c::RESET
    );
    println!();

    let settings_path = layout.profile_config_dir(profile).join("settings.yaml");
    let settings = load_settings(&settings_path).ok();

    println!(
        "  {}配置{}  {}",
        c::GRAY,
        c::RESET,
        layout.profile_config_dir(profile).display()
    );
    println!(
        "  {}数据{}  {}",
        c::GRAY,
        c::RESET,
        layout.profile_data_dir(profile).display()
    );
    println!("  {}身份{}  {profile}", c::GRAY, c::RESET);

    if let Some(s) = &settings {
        let llm = if s.llm.model.trim().is_empty() {
            format!("{}未配置{}", c::YELLOW, c::RESET)
        } else {
            let mode = if s.llm.mode.trim().is_empty() {
                "paid-api"
            } else {
                s.llm.mode.as_str()
            };
            format!("{} / {}（{mode}）", s.llm.model, s.llm.provider)
        };
        println!("  {}模型{}  {llm}", c::GRAY, c::RESET);

        let stt = if vca_platform::stt::local_available(
            &vca_platform::stt::LocalSttConfig {
                model: s.transcriber.model.clone(),
                ..Default::default()
            },
        ) {
            format!(
                "本地 whisper.cpp({}) {}就绪{}",
                s.transcriber.model,
                c::GREEN,
                c::RESET
            )
        } else {
            format!(
                "{}本地模型缺失（跑 scripts/fetch-deps.py）{}",
                c::YELLOW,
                c::RESET
            )
        };
        println!("  {}转写{}  {stt}", c::GRAY, c::RESET);

        let push = if s.push.provider.trim().is_empty() {
            format!("{}未配置{}", c::YELLOW, c::RESET)
        } else {
            s.push.provider.clone()
        };
        println!("  {}推送{}  {push}", c::GRAY, c::RESET);
    } else {
        println!(
            "  {}还没生成配置文件，先跑 /doctor 让它自动补上。{}",
            c::YELLOW,
            c::RESET
        );
    }

    println!();
    println!("  {}输入 /help 查看命令，/quit 退出。{}", c::DIM, c::RESET);
    println!();
}

/// 命令分发。
pub fn dispatch(layout: &Layout, profile: &str, line: &str) -> Result<Flow> {
    let line = line.trim();
    if !line.starts_with('/') {
        // 非斜杠输入：不做自然语言假装，直接指路
        println!(
            "{}{} 这里只认命令。输入 /help 看全部，或 /status 看当前状态。{}",
            c::GRAY,
            DOT,
            c::RESET
        );
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
        "/clear" => clear_screen(),
        "/quit" | "/exit" | "/q" => return Ok(Flow::Quit),
        other => {
            println!(
                "{}{} 未知命令 {}{}{}，输入 /help 看全部。",
                c::YELLOW,
                DOT,
                c::BOLD,
                other,
                c::RESET
            );
        }
    }
    Ok(Flow::Continue)
}

/// 帮助。
fn help() {
    println!("{}{} 命令{}", c::BLUE, DOT, c::RESET);
    let rows: &[(&str, &str)] = &[
        ("/status", "总览：课程、作业、配置、组件就绪情况"),
        ("/jobs", "列出所有作业及其状态"),
        ("/process", "处理待办作业（转写 → 提取 → 文档 → 推送）"),
        ("/process dry", "演练：不真正推送"),
        ("/doctor [fix]", "环境自检；加 fix 自动建目录补配置"),
        ("/config [path]", "查看配置目录 / 生成配置模板"),
        ("/record [status]", "录制状态；record start / stop 手动控制"),
        ("/audio", "探测 WASAPI 音频端点（系统声 / 麦克风）"),
        ("/record-test", "录 10 秒冒烟测试（含截图与合并）"),
        ("/transcribe-test", "对上次录制跑一次转写冒烟测试"),
        ("/clean [run]", "预览到期清理；加 run 真正删除"),
        ("/logs [tail]", "查看日志"),
        ("/clear", "清屏"),
        ("/quit", "退出（同 Ctrl+D）"),
    ];
    for (k, v) in rows {
        println!("  {}{:<20}{} {}", c::CYAN, k, c::RESET, v);
    }
    println!();
    println!(
        "  {}提示：直接输 `vca <子命令>` 也能用同样的功能，适合写进计划任务。{}",
        c::DIM,
        c::RESET
    );
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

    println!("{}{} 状态{}", c::BLUE, DOT, c::RESET);

    let mut by_state: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for j in &all {
        *by_state.entry(format!("{:?}", j.state)).or_insert(0) += 1;
    }

    println!(
        "  {}{} 作业共 {} 个{}",
        c::GRAY,
        ELBOW,
        all.len(),
        c::RESET
    );
    if all.is_empty() {
        println!("     （还没有作业。录制会在上课时段自动开始）");
    } else {
        for (k, v) in &by_state {
            println!("     {k}：{v}");
        }
    }

    let pending = store.pending();
    if !pending.is_empty() {
        println!(
            "  {}{} 待处理 {} 个 —— 用 /process 开始{}",
            c::YELLOW,
            ELBOW,
            pending.len(),
            c::RESET
        );
    }

    // 组件就绪情况
    let ffmpeg = vca_platform::capture::ffmpeg_available();
    println!(
        "  {}{} ffmpeg：{}{}{}",
        c::GRAY,
        ELBOW,
        if ffmpeg { c::GREEN } else { c::YELLOW },
        if ffmpeg { "就绪" } else { "缺失（跑 scripts/fetch-deps.py）" },
        c::RESET
    );

    let stt_ok = vca_platform::stt::local_available(&vca_platform::stt::LocalSttConfig {
        model: s.transcriber.model.clone(),
        ..Default::default()
    });
    println!(
        "  {}{} 本地转写：{}{}{}",
        c::GRAY,
        ELBOW,
        if stt_ok { c::GREEN } else { c::YELLOW },
        if stt_ok {
            format!("就绪（{}）", s.transcriber.model)
        } else {
            "模型缺失，将走云端（若已配置）".to_string()
        },
        c::RESET
    );

    let llm_ok = !s.llm.model.trim().is_empty();
    println!(
        "  {}{} 要点提取：{}{}{}",
        c::GRAY,
        ELBOW,
        if llm_ok { c::GREEN } else { c::YELLOW },
        if llm_ok {
            format!("{}（{}）", s.llm.model, s.llm.mode)
        } else {
            "未配置，将降级为原文摘要".to_string()
        },
        c::RESET
    );

    let push_ok = !s.push.provider.trim().is_empty();
    println!(
        "  {}{} 推送渠道：{}{}{}",
        c::GRAY,
        ELBOW,
        if push_ok { c::GREEN } else { c::YELLOW },
        if push_ok {
            s.push.provider.clone()
        } else {
            "未配置（dry-run）".to_string()
        },
        c::RESET
    );

    Ok(())
}

/// 列作业。
fn jobs(layout: &Layout, profile: &str) -> Result<()> {
    let (_, store) = open(layout, profile);
    let all = store.list();
    println!("{}{} 作业列表（{} 个）{}", c::BLUE, DOT, all.len(), c::RESET);
    if all.is_empty() {
        return Ok(());
    }
    for j in &all {
        let state = format!("{:?}", j.state);
        let color = match j.state {
            vca_core::job::JobState::Pushed | vca_core::job::JobState::Cleaned => c::GREEN,
            vca_core::job::JobState::Failed => c::RED,
            _ => c::YELLOW,
        };
        println!(
            "  {}{:<10}{} {} {}  {}{}{}",
            color, state, c::RESET, j.date, j.course, c::GRAY, j.id, c::RESET
        );
        if let Some(e) = &j.last_error {
            println!("     {}上次错误：{e}{}", c::RED, c::RESET);
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
        println!("{}{} 没有待处理的作业。{}", c::GRAY, DOT, c::RESET);
        return Ok(());
    }

    println!(
        "{}{} 开始处理 {} 个作业{}{}",
        c::BLUE,
        DOT,
        targets.len(),
        if dry { "（演练，不推送）" } else { "" },
        c::RESET
    );

    let mut pipeline = vca_engine::pipeline::Pipeline::new(
        layout,
        profile,
        &settings,
        &store,
        path_or_empty(layout),
    );
    pipeline.dry_run = dry;

    let started = std::time::Instant::now();
    for mut job in targets {
        let label = format!("{} {}", job.date, job.course);
        println!("  {}{} {label}{}", c::CYAN, DOT, c::RESET);
        let outcome = pipeline.process(&mut job);
        for s in &outcome.steps {
            println!("     {}{} {s}{}", c::GRAY, ELBOW, c::RESET);
        }
        for w in &outcome.warnings {
            println!("     {}{} {w}{}", c::YELLOW, ELBOW, c::RESET);
        }
        if let Some(e) = &outcome.error {
            println!("     {}{} 失败：{e}{}", c::RED, ELBOW, c::RESET);
        } else {
            println!(
                "     {}{} 完成，状态 {}{:?}{}",
                c::GREEN,
                ELBOW,
                c::BOLD,
                outcome.state,
                c::RESET
            );
        }
    }

    println!(
        "{}{} 全部结束，用时 {:.1}s{}",
        c::BLUE,
        DOT,
        started.elapsed().as_secs_f64(),
        c::RESET
    );
    Ok(())
}

/// `python/` 目录（现在只作为可选插件的载体，缺失不影响主流程）。
fn path_or_empty(_layout: &Layout) -> std::path::PathBuf {
    std::path::PathBuf::new()
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
        // 查询类命令不该报错
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
        // 不做自然语言假装：普通输入只是提示，不能退出也不能崩
        assert_eq!(dispatch(&l, "default", "帮我处理一下").unwrap(), Flow::Continue);
    }

    #[test]
    fn process_with_no_jobs_is_ok() {
        let l = layout();
        let _ = std::fs::create_dir_all(l.profile_data_dir("default"));
        assert_eq!(
            dispatch(&l, "default", "/process").unwrap(),
            Flow::Continue
        );
    }

    #[test]
    fn arguments_are_parsed() {
        let l = layout();
        // /clean 默认演练（dry-run），传 run 才真删 —— 这里只验证不报错
        assert_eq!(dispatch(&l, "default", "/clean").unwrap(), Flow::Continue);
        assert_eq!(dispatch(&l, "default", "/clean run").unwrap(), Flow::Continue);
    }
}
