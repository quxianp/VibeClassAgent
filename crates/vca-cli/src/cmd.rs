//! 子命令实现。
//!
//! 已实现：`doctor` / `config` / `overlay` / `run` / `clean` / `plugin` / `import` / `debug`。
//! 预留命令统一输出「预留」提示并给出对应策划书章节，避免用户误以为可用。

use anyhow::Result;

use vca_core::model::OverlaySettings;
use vca_core::paths::Layout;
use vca_core::{PRODUCT_NAME, VERSION};
use vca_platform::{doctor, overlay};

/// 打印欢迎横幅与基本信息。
pub fn print_banner(layout: &Layout, profile: Option<&str>) -> Result<()> {
    println!("{PRODUCT_NAME}  v{VERSION}");
    println!();
    println!("配置目录 : {}", layout.config_root.display());
    println!("数据目录 : {}", layout.data_root.display());
    println!(
        "当前身份 : {}",
        profile.unwrap_or("(默认：Windows 登录账户)")
    );
    println!();
    println!("可用命令：");
    println!("  config     配置读写与管理");
    println!("  timetable  时间表：自主配置或导入（策划书第 2 章）");
    println!("  schedule   课程表：手工 / CSV / ClassIsland 导入");
    println!("  record     录制控制 start|stop|status");
    println!("  task       作业状态查询与重试");
    println!("  plugin     插件管理（本地）");
    println!("  market     本地插件市场");
    println!("  log        日志查看");
    println!("  doctor     环境自检（可直接运行）");
    println!("  profile    多教师 profile 管理");
    println!();
    println!("提示：运行 `vca doctor` 查看本机依赖是否齐备。");
    Ok(())
}

/// 环境自检。
///
/// `fix = true` 时先做一轮**自动修复**：创建目录、从内置模板生成配置、
/// 再指出只有用户能提供的项（API Key、课表、推送地址）。
/// 从配置构造模型客户端配置。
///
/// **凭据只从环境变量读**（`VCA_LLM_API_KEY`），配置文件里永远只有地址与模型名。
pub fn llm_config_from(s: &vca_core::config::Settings) -> vca_platform::llm::LlmConfig {
    let b = &s.llm.browser;
    vca_platform::llm::LlmConfig {
        mode: vca_platform::llm::LlmMode::parse(&s.llm.mode),
        provider: s.llm.provider.clone(),
        base_url: s.llm.base_url.clone(),
        api_key: std::env::var("VCA_LLM_API_KEY").unwrap_or_default(),
        model: s.llm.model.clone(),
        timeout_ms: 90_000,
        max_chars: 6000,
        browser: vca_platform::browser_bot::BrowserBotConfig {
            site: b.site.clone(),
            headless: b.headless,
            risk_ack: b.risk_ack,
            user_data_dir: b.user_data_dir.clone().into(),
            timeout_sec: b.timeout_sec,
            selectors_override: b.selectors.clone(),
            ..Default::default()
        },
    }
}

pub fn doctor(layout: &Layout, fix: bool, profile: Option<&str>) -> Result<()> {
    let profile = profile.unwrap_or("default");

    if fix {
        println!("自动修复");
        println!("{}", "-".repeat(62));
        let rep = vca_core::setup::repair(layout, profile);

        if rep.created_dirs.is_empty() && rep.created_files.is_empty() {
            println!("   目录与配置都已就绪，无需修复");
        } else {
            for d in &rep.created_dirs {
                println!("  + 新建目录  {d}");
            }
            for f in &rep.created_files {
                println!("  + 生成文件  {f}");
            }
        }
        if !rep.fixed.is_empty() {
            println!();
            for f in &rep.fixed {
                println!("  * {f}");
            }
        }
        println!();
        println!("  {}", rep.summary());
        println!();
    }

    println!("环境自检");
    println!("{}", "-".repeat(62));
    let mut items = doctor::run_all();
    items.push(doctor::session_check());
    for it in &items {
        println!("{}", it.render());
    }
    println!("{}", "-".repeat(62));
    println!("说明：[SKIP] 表示该项缺失也不影响主流程（有替代路径），[MISS] 才要处理。");
    println!("      ffmpeg 缺少就没法录像；本地转写缺失可以改走云端。");

    // 配置层面的检查
    let rep = vca_core::setup::repair(layout, profile);
    if !rep.needs_user.is_empty() {
        println!();
        println!("还需要你提供这些信息：");
        for (i, n) in rep.needs_user.iter().enumerate() {
            println!("  {}. {n}", i + 1);
        }
        println!();
        println!("提示：运行 `vca setup` 会逐项引导你填完。");
    }
    if !rep.warnings.is_empty() {
        println!();
        println!("需要注意：");
        for w in &rep.warnings {
            println!("  ! {w}");
        }
    }

    let failed = items.iter().filter(|i| !i.ok && !i.optional).count();
    if failed == 0 && rep.needs_user.is_empty() {
        println!();
        println!(" 一切就绪，可以直接运行 `vca run`。");
    }
    Ok(())
}

/// 首次运行引导向导。
///
/// 流程：自动修复  课表  学期对齐  模型 API  推送  写回配置。
/// 每一步都可以跳过（留空回车），之后随时能用 `vca setup` 再来一遍。
pub fn setup(layout: &Layout, profile: Option<&str>) -> Result<()> {
    use std::io::Write;

    let profile = profile.unwrap_or("default").to_string();
    let settings_path = layout.profile_config_dir(&profile).join("settings.yaml");

    println!();
    println!("================================================================");
    println!("  VibeClassAgent  首次使用引导");
    println!("================================================================");
    println!();
    println!("  每一步都可以直接回车跳过；跳过的东西之后随时能补。");
    println!("  重新运行本向导：vca setup");
    println!();

    // ---------- 第 1 步：自动修复 ----------
    println!("[1/5] 环境检查与自动修复");
    let rep = vca_core::setup::repair(layout, &profile);
    if rep.created_dirs.is_empty() && rep.created_files.is_empty() {
        println!("       目录与配置已就绪");
    } else {
        for f in rep.created_files.iter().take(5) {
            println!("      + 已生成 {f}");
        }
        println!(
            "       已补齐 {} 个目录 / {} 个文件",
            rep.created_dirs.len(),
            rep.created_files.len()
        );
    }

    // 录制能力探测：直接用 Windows 的音频 API 问，不再绕道 Python
    println!("      正在检测录屏与麦克风（首次可能要几秒）");
    let probe = probe_recording();
    for line in probe.lines() {
        println!("      {line}");
    }
    println!();

    // ---------- 第 2 步：课表 ----------
    println!("[2/5] 课表");
    let (need_sched, _) = schedule_state(layout, &profile);
    if !need_sched {
        println!("       课表已有内容");
    } else {
        println!("      现在还没有可用的课表。三种方式任选：");
        println!("        a) 从 ClassIsland 导入（推荐，如果有用）");
        println!("        b) 从 CSV 导入（模板：vca import template csv > 课表.csv）");
        println!("        c) 稍后自己编辑 config\\schedule\\current.yaml");
        println!();
        print!("      输入 ClassIsland 的 Default.json 路径（直接回车跳过）：");
        let _ = std::io::stdout().flush();
        let line = read_line();
        let t = line.trim().trim_matches('"');
        if !t.is_empty() {
            let p = std::path::PathBuf::from(t);
            match vca_core::import::import_classisland(&p, None) {
                Ok(imp) => {
                    let week = vca_platform::clock::now_local().date.week_key();
                    let (tt, sc) = vca_core::import::write_current(
                        &layout.config_root,
                        &imp.timetable,
                        &imp.schedule,
                    )?;
                    let _ = vca_core::import::save_archive(
                        &layout.timetable_archive_dir(),
                        "timetable",
                        &week,
                        &vca_core::config::TimetableFile {
                            timetables: vec![imp.timetable.clone()],
                        },
                    );
                    let _ = vca_core::import::save_archive(
                        &layout.schedule_archive_dir(),
                        "schedule",
                        &week,
                        &imp.schedule,
                    );
                    println!("       导入成功：{} 条课程", imp.report.entry_count);
                    println!("        时间表 {}", tt.display());
                    println!("        课程表 {}", sc.display());
                    if !imp.report.teachers_missing.is_empty() {
                        println!(
                            "        ! 这些科目没填教师：{}",
                            imp.report.teachers_missing.join("、")
                        );
                    }
                    println!();
                    println!("       导入后所有课的 record 都是 false（不会录任何课）。");
                    print!("        要现在把「全部课」都设为要录吗？[y/N] ");
                    let _ = std::io::stdout().flush();
                    if read_line().trim().eq_ignore_ascii_case("y") {
                        mark_all_record(&sc)?;
                        println!("         已把所有课设为 record: true（可再手动关掉不想录的）");
                    }
                }
                Err(e) => println!("       导入失败：{e}"),
            }
        }
    }
    println!();

    // ---------- 第 3 步：学期对齐 ----------
    println!("[3/5] 学期对齐（决定「单周/双周」怎么算）");
    println!("      如果课表里没有单双周，这一步可以跳过。");
    print!("      本学期第 1 周的周一日期（如 2026-09-01，回车跳过）：");
    let _ = std::io::stdout().flush();
    let first = read_line().trim().to_string();
    if !first.is_empty() {
        if vca_core::time::LocalDateTime::parse(&format!("{first} 00:00"))
            .filter(|d| d.date.year > 2000 && d.date.year < 2100)
            .is_some()
        {
            let _ = vca_core::setup::patch_settings_line(
                &settings_path,
                "term.first_monday",
                &format!("\"{first}\""),
            );
            print!("      该周是单周还是双周？[1=单周 / 2=双周，回车=单周] ");
            let _ = std::io::stdout().flush();
            let par = if read_line().trim() == "2" {
                "even"
            } else {
                "odd"
            };
            let _ =
                vca_core::setup::patch_settings_line(&settings_path, "term.first_week_parity", par);
            println!(
                "       已写入（第 1 周为{}周）",
                if par == "even" { "双" } else { "单" }
            );
        } else {
            println!("       日期格式不对（应为 YYYY-MM-DD），已跳过");
        }
    }
    println!();

    // ---------- 第 4 步：模型 API ----------
    println!("[4/5] 模型 API（把转写文字提炼成课堂要点）");
    println!("      语音转写是本地完成的、不需要 Key；这里配的是「要点提取」用的接口。");
    println!();

    // 4.1 选服务商：用内置预设，省得用户去翻各家文档
    let presets: Vec<&vca_platform::llm::ProviderPreset> = vca_platform::llm::PROVIDERS
        .iter()
        .filter(|p| !p.base_url.is_empty())
        .collect();
    println!("      选择服务商：");
    for (i, p) in presets.iter().enumerate() {
        println!("        {:>2}) {:<20} {}", i + 1, p.name, p.note);
    }
    println!("         0) 跳过（之后可用 /model 配置）");
    print!("      请选择 [0-{}，回车=0]：", presets.len());
    let _ = std::io::stdout().flush();
    let idx: usize = read_line().trim().parse().unwrap_or(0);

    if idx >= 1 && idx <= presets.len() {
        let p = presets[idx - 1];
        let _ = vca_core::setup::patch_settings_line(
            &settings_path,
            "llm.provider",
            &format!("\"{}\"", p.id),
        );
        let _ = vca_core::setup::patch_settings_line(
            &settings_path,
            "llm.base_url",
            &format!("\"{}\"", p.base_url),
        );

        // 4.2 API Key：写进本地凭据文件，不动系统环境变量
        println!();
        print!("      API Key（回车跳过）：");
        let _ = std::io::stdout().flush();
        let key = read_line().trim().to_string();
        if !key.is_empty() {
            let sp = vca_core::secrets::default_path(&layout.config_root);
            match vca_core::secrets::upsert(&sp, "VCA_LLM_API_KEY", &key) {
                Ok(()) => {
                    // 立刻注入，这样下面就能直接拿它拉模型列表
                    std::env::set_var("VCA_LLM_API_KEY", &key);
                    println!("       已写入 {}", sp.display());
                    println!("       这个文件含密钥，已被 .gitignore 排除，别拷给别人。");
                }
                Err(e) => {
                    println!("       写入失败：{e}");
                    println!("       可手动设置环境变量 VCA_LLM_API_KEY。");
                }
            }
        }

        // 4.3 自动拉模型列表让用户挑 —— 模型名最容易填错，
        //     而填错的代价是「课后处理时才发现调用失败」，那时课已经录完了。
        println!();
        let mut chosen: Option<String> = None;
        if vca_core::secrets::is_set("VCA_LLM_API_KEY") {
            let cfg = vca_platform::llm::LlmConfig {
                provider: p.id.to_string(),
                base_url: p.base_url.to_string(),
                api_key: std::env::var("VCA_LLM_API_KEY").unwrap_or_default(),
                ..Default::default()
            };
            print!("      正在拉取模型列表…");
            let _ = std::io::stdout().flush();
            match vca_platform::llm::list_models(&cfg) {
                Ok(models) if !models.is_empty() => {
                    println!(" 找到 {} 个", models.len());
                    let show = models.len().min(20);
                    for (i, m) in models.iter().take(show).enumerate() {
                        println!("        {:>2}) {m}", i + 1);
                    }
                    if models.len() > show {
                        println!("         …（共 {} 个，也可以直接输入模型名）", models.len());
                    }
                    print!("      选择模型 [1-{show}，回车跳过]：");
                    let _ = std::io::stdout().flush();
                    let s = read_line().trim().to_string();
                    match s.parse::<usize>() {
                        Ok(n) if n >= 1 && n <= show => chosen = Some(models[n - 1].clone()),
                        _ if !s.is_empty() => chosen = Some(s),
                        _ => {}
                    }
                }
                Ok(_) => println!(" 服务端没有返回模型列表"),
                Err(e) => println!(" 失败：{e}"),
            }
        }

        // 拉不到列表就退回手填
        let model = match chosen {
            Some(m) => m,
            None => {
                let hint = p.paid_models.first().copied().unwrap_or("模型名");
                print!("      模型名（如 {hint}）：");
                let _ = std::io::stdout().flush();
                read_line().trim().to_string()
            }
        };
        if !model.is_empty() {
            let _ = vca_core::setup::patch_settings_line(
                &settings_path,
                "llm.model",
                &format!("\"{model}\""),
            );
            println!("      已写入配置：{} / {model}", p.name);
        }
    } else {
        println!("      已跳过，之后可用 /model 配置。");
    }
    println!();

    // ---------- 第 5 步：推送 ----------
    println!("[5/5] 推送渠道（把文档发到微信 / QQ）");
    println!("      1) 企业微信机器人   2) 通用 Webhook   3) Server 酱   0) 先不配");
    print!("      请选择 [0-3，回车=0]：");
    let _ = std::io::stdout().flush();
    let provider = match read_line().trim() {
        "1" => Some("wecom"),
        "2" => Some("webhook"),
        "3" => Some("serverchan"),
        _ => None,
    };
    if let Some(pv) = provider {
        let _ = vca_core::setup::patch_settings_line(
            &settings_path,
            "push.provider",
            &format!("\"{pv}\""),
        );
        println!("       已选：{pv}");
        println!("      推送地址同样走环境变量：");
        println!("        setx VCA_PUSH_ENDPOINT \"你的Webhook地址\"");
    } else {
        println!("      已跳过（之后在 settings.yaml 的 push.provider 里配）");
    }

    // ---------- 收尾 ----------
    vca_core::setup::mark_initialized(layout)?;

    println!();
    println!("================================================================");
    println!("  引导完成");
    println!("================================================================");
    println!();
    println!("  配置目录：{}", layout.config_root.display());
    println!("  数据目录：{}", layout.data_root.display());
    println!();
    println!("  接下来建议按顺序做：");
    println!("    1) vca doctor --fix      再看一遍还有没有缺的");
    println!("    2) vca debug record-test 录 10 秒，确认录屏录音正常");
    println!("    3) vca overlay plan      看看今天的悬浮窗时刻对不对");
    println!("    4) vca run --dry-run     试跑一遍（不真正录制/推送）");
    println!("    5) vca run               正式运行");
    println!();
    println!("  双击本程序也能看到同样的菜单（不用记命令）。");
    Ok(())
}

/// 清掉初始化标记，让下次启动重新走引导。
pub fn reset_setup(layout: &Layout) -> Result<()> {
    vca_core::setup::unmark_initialized(layout);
    println!("已清除初始化标记，下次运行 `vca setup` 会重新引导。");
    println!("配置目录：{}", layout.config_root.display());
    Ok(())
}

/// 读一行输入（去掉换行）。
fn read_line() -> String {
    let mut s = String::new();
    let _ = std::io::stdin().read_line(&mut s);
    s.trim_end_matches(['\r', '\n']).to_string()
}

/// 探测录制能力。
///
/// 这里**直接问 Windows**：音频端点用 WASAPI 枚举，录屏引擎查 ffmpeg，
/// 本地转写查 whisper.cpp 是否就位。
///
/// 旧版本是拉起一个 Python 脚本来问的，那个设计不仅要求机器上有可用的 Python，
/// 还会在路径不对时吐出一句 `MISS 无法启动 Python（os error 267）`——
/// 用户完全无从下手。现在同一件事只有 Rust 一处实现，也就没得错。
fn probe_recording() -> String {
    let mut lines: Vec<String> = Vec::new();

    // 音频：WASAPI 默认端点。这是「能不能录到系统播放的声音」的判据。
    match vca_platform::audio::probe_endpoints() {
        Ok(list) if list.is_empty() => {
            lines.push("MISS 没有可用的音频端点（会录成无声视频）".into())
        }
        Ok(list) => {
            for ep in list {
                let role = if ep.role == "render" {
                    "系统声音（回环）"
                } else {
                    "麦克风"
                };
                lines.push(format!("OK   {role}：{}", ep.format));
            }
        }
        Err(e) => lines.push(format!("MISS 音频探测失败：{e}")),
    }

    // 屏幕：ffmpeg 在不在。真正能不能抓到画面，要录一次才算数。
    match vca_platform::capture::ffmpeg_path() {
        Some(p) => lines.push(format!("OK   录屏引擎：{}", p.display())),
        None => lines.push("MISS 找不到 ffmpeg，无法录屏（跑 scripts/fetch-deps.py）".into()),
    }

    // 本地转写：缺了还能走云端，所以是 SKIP 而不是 MISS
    if vca_platform::stt::local_available(&Default::default()) {
        lines.push("OK   本地转写：whisper.cpp 就绪".into());
    } else {
        lines.push("SKIP 本地转写未就绪（可改用云端，或跑 scripts/fetch-deps.py）".into());
    }

    lines.join("\n")
}

/// 课表是否还是空的。
fn schedule_state(layout: &Layout, _profile: &str) -> (bool, usize) {
    let p = layout.config_root.join("schedule").join("current.yaml");
    // 复用 core 的加载器，避免在 CLI 里再引一份 serde_yaml
    let Ok(f) = vca_core::config::load_schedule_file(&p) else {
        return (true, 0);
    };
    let n = f.week_template.entries.len()
        + f.weekend_template
            .as_ref()
            .map(|w| w.entries.len())
            .unwrap_or(0);
    (n == 0, n)
}

/// 把课表里所有课都设为 `record: true`。
fn mark_all_record(schedule_path: &std::path::Path) -> Result<()> {
    let text = std::fs::read_to_string(schedule_path)?;
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        if line.contains("record:") {
            out.push_str(&line.replace("record: false", "record: true"));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    std::fs::write(schedule_path, out)?;
    Ok(())
}

/// 配置命令（部分预留：仅 `path` 已实现）。
pub fn config(layout: &Layout, action: &str, _args: &[String]) -> Result<()> {
    match action {
        "path" => {
            println!("配置目录 : {}", layout.config_root.display());
            println!("数据目录 : {}", layout.data_root.display());
            Ok(())
        }
        _ => not_implemented("config", action, "策划书第 9 章 用户配置项"),
    }
}

/// 时间表命令（预留；导入请用 `import`）。
pub fn timetable(_layout: &Layout, action: &str, _args: &[String]) -> Result<()> {
    not_implemented("timetable", action, "策划书第 2 章 时间表配置与导入")
}

/// 课程表命令（预留；导入请用 `import`）。
pub fn schedule(_layout: &Layout, action: &str, _args: &[String]) -> Result<()> {
    not_implemented("schedule", action, "策划书 9.3 课表字段 / 附录 B CSV 对接")
}

/// 录制命令（预留；录制由 `run` 的守护进程自动完成）。
pub fn record(_layout: &Layout, action: &str) -> Result<()> {
    not_implemented("record", action, "策划书 3.2.1 静默录制")
}

/// 作业命令（预留；作业状态在数据目录 jobs/ 下可查）。
pub fn task(_layout: &Layout, action: &str, _id: Option<&str>) -> Result<()> {
    not_implemented("task", action, "策划书 6.1 关键流程时序")
}

/// 解析插件目录。
///
/// 按「部署形态  数据形态  开发形态」依次探测，取第一个**真实存在且含清单**的目录：
/// 1. 可执行体同级 `plugins/`（把 vca.exe 与 plugins 一起打包的部署方式）
/// 2. 配置根目录同级 `plugins/`（`<ProgramData>/<产品名>/` 布局）
/// 3. 当前工作目录 `plugins/`（从源码仓库直接运行）
///
/// 都不存在时返回「配置根目录同级」并创建它，避免把插件装到意料之外的地方。
fn resolve_plugins_dir(layout: &Layout) -> std::path::PathBuf {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            candidates.push(d.join("plugins"));
        }
    }
    let config_sibling = layout
        .config_root
        .parent()
        .map(|p| p.join("plugins"))
        .unwrap_or_else(|| std::path::PathBuf::from("plugins"));
    candidates.push(config_sibling.clone());
    candidates.push(std::path::PathBuf::from("plugins"));

    let has_manifest = |d: &std::path::Path| -> bool {
        std::fs::read_dir(d)
            .map(|rd| {
                rd.flatten()
                    .any(|e| e.path().join("manifest.toml").exists())
            })
            .unwrap_or(false)
    };

    // 第一轮：优先选「已经装了插件」的目录
    for c in &candidates {
        if c.is_dir() && has_manifest(c) {
            return c.clone();
        }
    }
    // 第二轮：退而求其次，选一个已存在的空目录
    // （发行版里 `plugins/` 就是空的，但它正是用户安装插件的地方）
    for c in &candidates {
        if c.is_dir() {
            return c.clone();
        }
    }
    // 都不存在：用配置同级目录并创建
    let _ = std::fs::create_dir_all(&config_sibling);
    config_sibling
}

/// 插件命令（`list` / `info` / `install` / `remove` 已实现）。
pub fn plugin(layout: &Layout, action: &str, target: Option<&str>) -> Result<()> {
    let dir = resolve_plugins_dir(layout);
    match action {
        "list" | "discover" => {
            let mut host = vca_plugin_host::host::PluginHost::new().with_plugins_dir(&dir);
            let n = host.discover()?;
            println!("插件目录：{}", dir.display());
            println!("已加载 {n} 个插件：");
            println!("{:<28} {:<12} {:<10} 名称", "ID", "类型", "版本");
            for h in host.list() {
                println!(
                    "{:<28} {:<12} {:<10} {}",
                    h.manifest.plugin.id,
                    format!("{:?}", h.manifest.plugin.kind).to_lowercase(),
                    h.manifest.plugin.version,
                    h.manifest.plugin.name
                );
            }
            Ok(())
        }
        "info" => {
            let id = target.unwrap_or("");
            let mut host = vca_plugin_host::host::PluginHost::new().with_plugins_dir(&dir);
            host.discover()?;
            match host.list().iter().find(|h| h.manifest.plugin.id == id) {
                Some(h) => {
                    println!("插件 id   : {}", h.manifest.plugin.id);
                    println!("名称      : {}", h.manifest.plugin.name);
                    println!("版本      : {}", h.manifest.plugin.version);
                    println!("接口版本  : {}", h.manifest.plugin.api_version);
                    println!("运行模式  : {}", h.manifest.runtime.r#type);
                    println!("入口      : {}", h.manifest.runtime.entry);
                    println!("目录      : {}", h.dir.display());
                    if let Some(r) = &h.manifest.plugin.risk_note {
                        println!("风险提示  : {r}");
                    }
                    Ok(())
                }
                None => {
                    println!("未找到插件：{id}");
                    Ok(())
                }
            }
        }
        "install" => {
            let src = target.unwrap_or("");
            if src.is_empty() {
                eprintln!("用法：vca plugin install <插件目录>");
                return Ok(());
            }
            let market = vca_plugin_host::market::LocalMarket::new();
            let source =
                vca_plugin_host::market::MarketSource::LocalDir(std::path::PathBuf::from(src));
            match market.install(&source, &dir, false) {
                Ok(out) => {
                    println!("插件已安装：{}", out.plugin_id);
                    println!("安装位置  ：{}", out.installed_to.display());
                    // 权限与风险必须显式展示给用户确认（策划书 4.2）
                    if !out.network.is_empty() {
                        println!("网络权限  : {}", out.network.join(", "));
                    }
                    if !out.filesystem.is_empty() {
                        println!("文件权限  : {}", out.filesystem.join(", "));
                    }
                    if !out.env.is_empty() {
                        println!("需要的环境变量: {}", out.env.join(", "));
                    }
                    if let Some(risk) = &out.risk_note {
                        println!();
                        println!(" 风险提示：{risk}");
                        println!("  启用前请自行评估；本项目不对第三方协议的后果负责。");
                    }
                    println!();
                    println!("提示：本程序**不会自动执行**插件，需由后续版本的插件调度接入。");
                    Ok(())
                }
                Err(e) => {
                    eprintln!("安装失败：{e}");
                    Ok(())
                }
            }
        }
        "remove" | "uninstall" => {
            let id = target.unwrap_or("");
            if id.is_empty() {
                eprintln!("用法：vca plugin remove <插件id>");
                return Ok(());
            }
            let market = vca_plugin_host::market::LocalMarket::new();
            match market.uninstall(id, &dir) {
                Ok(p) => {
                    println!("已卸载：{id}");
                    println!("删除目录：{}", p.display());
                    Ok(())
                }
                Err(e) => {
                    eprintln!("卸载失败：{e}");
                    Ok(())
                }
            }
        }
        "enable" | "disable" | "doctor" => not_implemented(
            "plugin",
            action,
            "策划书 4.2 插件机制（启用/停用待插件调度接入后开放）",
        ),
        _ => not_implemented("plugin", action, "策划书 4.2 插件机制"),
    }
}

/// 市场命令（预留；v1 仅支持本地目录安装）。
pub fn market(_layout: &Layout, action: &str) -> Result<()> {
    not_implemented("market", action, "策划书 4.2 本地插件市场")
}

/// 日志命令（预留；日志见数据目录 logs/）。
pub fn log(_layout: &Layout, action: &str) -> Result<()> {
    not_implemented("log", action, "策划书 8.3 日志保留 30 天")
}

/// profile 命令（预留；用 --profile 切换）。
pub fn profile(_layout: &Layout, action: &str, _name: Option<&str>) -> Result<()> {
    not_implemented("profile", action, "策划书 4.6 多教师共用设计")
}

/// 统一的「预留」提示。
fn not_implemented(cmd: &str, action: &str, reference: &str) -> Result<()> {
    println!("[预留] {cmd} {action}");
    println!("  该子命令尚未实现；当前功能已可通过 run / import / overlay / clean 完成。");
    println!("  设计依据：{reference}");
    Ok(())
}

/// 悬浮窗命令。
///
/// - `demo`  ：立即显示悬浮窗，保持 N 秒后关闭（用于人工验证外观与位置）；
/// - `plan`  ：读取课程表 YAML，打印「今日各次课结束后悬浮窗的显示/关闭时刻」；
/// - `show` / `hide`：当前进程内直接控制（调试用）。
pub fn overlay(_layout: &Layout, action: &str, arg: Option<&str>) -> Result<()> {
    match action {
        "demo" => {
            let secs: u64 = arg.and_then(|s| s.trim().parse().ok()).unwrap_or(8);
            let style = style_from(&OverlaySettings::default());
            println!("正在创建悬浮窗（右上角，置顶，不抢焦点）...");
            let (win, handle) = match overlay::spawn(style) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("创建悬浮窗失败：{e}");
                    eprintln!("提示：无图形会话（如远程/服务会话）时无法创建窗口。");
                    return Ok(());
                }
            };
            println!("窗口句柄 = 0x{:X}", win.raw());
            win.show();
            println!("已显示，保持 {secs} 秒后自动关闭...");
            std::thread::sleep(std::time::Duration::from_secs(secs));
            win.close();
            let _ = handle.join();
            println!("已关闭。");
            Ok(())
        }
        "plan" => {
            let path = arg
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("config/schedule.example.yaml"));
            let file = match vca_core::config::load_schedule_file(&path) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("读取课程表失败：{e}");
                    return Ok(());
                }
            };
            let settings = OverlaySettings::default();
            let today = vca_platform::clock::now_local().date;
            let plan = file.plan_for_weekday(today.weekday());
            let lessons = vca_core::schedule::plan_for_date(
                today,
                vca_core::schedule::WeekParity::Every,
                &plan,
                &file.overrides,
                true,
            );
            let windows = vca_core::schedule::overlay_windows(&lessons, settings.timing());

            println!("日期：{today}（星期{}）", today.weekday() + 1);
            println!("悬浮窗文本：{}", settings.text);
            println!(
                "规则：下课后 {} 分钟显示，上课前 {} 分钟关闭；无后续课程时兜底 {} 分钟",
                settings.show_delay_minutes,
                settings.hide_before_minutes,
                settings.fallback_minutes
            );
            println!("{}", "-".repeat(62));
            if lessons.is_empty() {
                println!("当天没有课程。");
                return Ok(());
            }
            println!(
                "{:<6} {:<14} {:<8} {:<10} {:<10}",
                "序号", "课程", "时段", "显示", "关闭"
            );
            for (i, l) in lessons.iter().enumerate() {
                let w = windows.iter().find(|w| w.show_at >= l.end);
                let (sh, hd) = match w {
                    Some(w) => (
                        format!("{:02}:{:02}", w.show_at.hour, w.show_at.minute),
                        format!("{:02}:{:02}", w.hide_at.hour, w.hide_at.minute),
                    ),
                    None => ("（跳过）".to_string(), "间隔过短".to_string()),
                };
                println!(
                    "{:<6} {:<14} {:02}:{:02}-{:02}:{:02}  {:<10} {:<10}",
                    i + 1,
                    l.course,
                    l.start.hour,
                    l.start.minute,
                    l.end.hour,
                    l.end.minute,
                    sh,
                    hd
                );
            }
            Ok(())
        }
        "show" | "hide" => {
            println!("[{action}] 需要宿主进程持有窗口句柄；请用 `vca overlay demo` 验证。");
            Ok(())
        }
        _ => not_implemented("overlay", action, "策划书 1.4 悬浮窗时序"),
    }
}

/// 由配置构造窗口样式。
fn style_from(s: &OverlaySettings) -> overlay::OverlayStyle {
    let bg = vca_core::model::parse_hex_color(&s.bg_color).unwrap_or((0x1F, 0x6F, 0xEB));
    let fg = vca_core::model::parse_hex_color(&s.text_color).unwrap_or((0xFF, 0xFF, 0xFF));
    overlay::OverlayStyle {
        text: s.text.clone(),
        width: s.width,
        height: s.height,
        margin_top: s.margin_top,
        margin_right: s.margin_right,
        bg_rgb: ((bg.0 as u32) | ((bg.1 as u32) << 8) | ((bg.2 as u32) << 16)),
        text_rgb: ((fg.0 as u32) | ((fg.1 as u32) << 8) | ((fg.2 as u32) << 16)),
        font_size: s.font_size,
        corner_radius: 14,
        click_through: false,
        font_face: s.font_face.clone(),
        liquid_glass: s.liquid_glass,
        glass_alpha: s.glass_alpha,
        use_system_accent: s.use_system_accent,
    }
}

/// 内部诊断命令。
///
/// `panic` 用于**验证崩溃静默退出**：安装崩溃处理器后主动 panic，
/// 期望行为是「写入 crash.log 并以退出码 70 静默结束，不打印、不弹窗」。
pub fn debug(layout: &Layout, action: &str) -> Result<()> {
    let log_dir = layout.profile_data_dir("default").join("logs");
    let handler = vca_platform::crash::CrashHandler::new(&log_dir);

    match action {
        "panic" => {
            handler.install();
            // 主动制造一次 panic，用于端到端验证静默退出。
            // 说明：这里必须真的 panic，故使用 panic! 而非 Result::unwrap。
            #[allow(clippy::panic)]
            {
                panic!("故意触发的崩溃（用于验证静默退出）");
            }
            #[allow(unreachable_code)]
            Ok(())
        }
        "audio" => {
            // 音频端点自检：验证 WASAPI 能否枚举到「系统回环」端。
            // 这是「能不能录到一体机自己播放的声音」的判据。
            println!("WASAPI 音频端点");
            println!("{}", "-".repeat(62));
            match vca_platform::audio::probe_endpoints() {
                Ok(list) if list.is_empty() => {
                    println!("未发现默认音频端点（系统里可能没有可用的播放/录制设备）");
                    Ok(())
                }
                Ok(list) => {
                    for ep in list {
                        let label = if ep.role == "render" {
                            "播放端（系统声音回环，loopback）"
                        } else {
                            "录制端（麦克风）"
                        };
                        println!("{label}");
                        println!("  格式 : {}", ep.format);
                        println!("  端点 : {}", ep.id);
                    }
                    Ok(())
                }
                Err(e) => {
                    println!("探测失败：{e}");
                    Ok(())
                }
            }
        }
        "transcribe" => {
            // 转写冒烟测试：对指定媒体文件跑一次本地/云端转写并打印结果。
            // 用法：vca debug transcribe <文件> [work_dir]
            let media = layout
                .data_root
                .join("_record_test")
                .join("test.mp4");
            let media = if media.is_file() {
                media
            } else {
                println!("没有可转写的文件。先跑 `vca debug record-test` 录一段。");
                return Ok(());
            };
            let work = layout.data_root.join("_record_test").join("_work_stt");

            println!("转写冒烟测试");
            println!("{}", "-".repeat(62));
            println!("输入 : {}", media.display());
            println!(
                "本地 : {}",
                if vca_platform::stt::local_available(&Default::default()) {
                    "可用"
                } else {
                    "不可用（缺 whisper-cli.exe 或模型，可运行 scripts/fetch-deps.py）"
                }
            );
            println!();

            let cfg = vca_platform::stt::SttConfig::new();
            let t0 = std::time::Instant::now();
            match vca_platform::stt::transcribe(&cfg, &media, &work) {
                Ok(t) => {
                    println!("引擎     : {}", t.engine);
                    println!("耗时     : {:.1}s（含转码）", t.seconds);
                    println!("分段     : {} 段", t.segments.len());
                    println!("文本长度 : {} 字", t.text.chars().count());
                    println!();
                    println!("--- 转写内容（前 500 字）---");
                    let preview: String = t.text.chars().take(500).collect();
                    println!("{preview}");
                    if t.text.chars().count() > 500 {
                        println!("...（已截断）");
                    }
                }
                Err(e) => {
                    println!("转写失败：{e}");
                }
            }
            println!();
            println!("总耗时 : {:.1}s", t0.elapsed().as_secs_f64());
            Ok(())
        }
        "e2e" => {
            // 端到端冒烟：录一小段 → 跑完整课后流水线。
            // 这是「所有零件装在一起还能转」的唯一证明。
            use vca_core::job::{Job, JobState};
            use vca_core::store::JobStore;
            use vca_engine::pipeline::Pipeline;

            let profile = "default";
            let secs: u32 = 15;

            println!("端到端冒烟测试");
            println!("{}", "-".repeat(62));

            // 注意：LocalDateTime 的 Display 带时间部分，直接拿去做目录名会
            // 因为冒号（Windows 非法字符）报 os error 267。只取日期段。
            let now = vca_platform::clock::now_local().to_string();
            let today: String = now
                .split_whitespace()
                .next()
                .unwrap_or("1970-01-01")
                .chars()
                .take(10)
                .collect();
            let compact = today.replace('-', "");

            // ---- 1) 录制 ----
            let raw_dir = layout.raw_dir(profile, &compact);
            std::fs::create_dir_all(&raw_dir)?;
            let video = raw_dir.join("e2e.mp4");
            let shots = layout.screenshots_dir(profile, &compact);
            let _ = std::fs::remove_file(&video);
            let _ = std::fs::remove_dir_all(&shots);

            let params = vca_platform::capture::CaptureParams {
                fps: 8,
                height: 720,
                crf: 30,
                screenshot_interval_secs: 5,
                ..Default::default()
            };
            print!("① 录制 {secs} 秒");
            use std::io::Write;
            let _ = std::io::stdout().flush();
            let mut sess = match vca_platform::capture::CaptureSession::new(params, &video) {
                Ok(s) => s.with_shot_dir(shots.clone()),
                Err(e) => {
                    println!();
                    println!("   录制初始化失败：{e}");
                    return Ok(());
                }
            };
            if let Err(e) = sess.start() {
                println!();
                println!("   录制启动失败：{e}");
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_secs(secs as u64));
            let _ = sess.stop();
            let outcome = match sess.finalize() {
                Ok(o) => o,
                Err(e) => {
                    println!();
                    println!("   收尾失败：{e}");
                    return Ok(());
                }
            };
            println!(
                "   → 视频 {} / 音频 {} 路 / 截图 {} 张",
                if outcome.video.is_some() { "有" } else { "无" },
                outcome.audio.len(),
                outcome.screenshots.len()
            );
            for n in &outcome.notes {
                println!("     备注：{n}");
            }

            // ---- 2) 建作业 ----
            let store = JobStore::new(layout.profile_data_dir(profile));
            let mut job = Job {
                id: format!("e2e_{compact}_端到端测试"),
                profile: profile.to_string(),
                state: JobState::Recorded,
                push_succeeded_at: None,
                expire_at: None,
                last_error: None,
                course: "端到端测试".to_string(),
                teacher_id: String::new(),
                date: today.clone(),
                start: "08:00".to_string(),
                end: "08:12".to_string(),
                video_path: outcome
                    .video
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string()),
                audio_path: outcome
                    .audio
                    .first()
                    .map(|p| p.to_string_lossy().to_string()),
                screenshots: outcome
                    .screenshots
                    .iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect(),
                docx_path: None,
                pdf_path: None,
            };
            store.save(&job)?;

            // ---- 3) 流水线 ----
            let settings_path = layout.profile_config_dir(profile).join("settings.yaml");
            let settings = vca_core::config::load_settings(&settings_path).unwrap_or_default();

            println!("② 课后流水线（演练，不真正推送）");
            let mut pipe = Pipeline::new(layout, profile, &settings, &store, "");
            pipe.dry_run = true;
            let res = pipe.process(&mut job);
            for s in &res.steps {
                println!("   ⎿ {s}");
            }
            for w in &res.warnings {
                println!("   ⚠ {w}");
            }
            if let Some(e) = &res.error {
                println!("   ✗ 失败：{e}");
            }

            // ---- 4) 产物清单 ----
            println!("③ 产物");
            let dir = store.dir_of(&job.id);
            if let Ok(rd) = std::fs::read_dir(&dir) {
                let mut items: Vec<_> = rd.flatten().map(|e| e.path()).collect();
                items.sort();
                for p in items {
                    let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                    println!(
                        "   {size:>9} B  {}",
                        p.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default()
                    );
                }
            }
            println!("{}", "-".repeat(62));
            println!("结束状态：{:?}", job.state);
            Ok(())
        }
        "record-test" => {
            // 录制冒烟测试：录 10 秒并报告产出，用来验证「录屏 + 系统声音 + 麦克风」链路。
            use vca_platform::capture::{CaptureParams, CaptureSession};

            let out_dir = layout.data_root.join("_record_test");
            std::fs::create_dir_all(&out_dir)?;
            let out = out_dir.join("test.mp4");
            let shots = out_dir.join("shots");
            let _ = std::fs::remove_file(&out);

            println!("录制冒烟测试");
            println!("{}", "-".repeat(62));
            println!("输出目录 : {}", out_dir.display());
            println!("时长     : 10 秒（8 fps / 720p / 每 5 秒一张截图）");
            println!();

            let params = CaptureParams {
                fps: 8,
                height: 720,
                crf: 30,
                screenshot_interval_secs: 5,
                ..Default::default()
            };

            let mut sess = match CaptureSession::new(params, &out) {
                Ok(s) => s.with_shot_dir(shots.clone()),
                Err(e) => {
                    println!("初始化失败：{e}");
                    return Ok(());
                }
            };
            println!("录制引擎 : {}", sess.engine_label());
            println!();

            print!("正在录制");
            use std::io::Write;
            let _ = std::io::stdout().flush();
            if let Err(e) = sess.start() {
                println!();
                println!("启动失败：{e}");
                return Ok(());
            }
            for _ in 0..10 {
                std::thread::sleep(std::time::Duration::from_secs(1));
                print!(".");
                let _ = std::io::stdout().flush();
            }
            println!();
            let _ = sess.stop();

            println!();
            println!("{}", "-".repeat(62));
            match sess.finalize() {
                Ok(o) => {
                    match &o.video {
                        Some(v) => {
                            let kb = std::fs::metadata(v).map(|m| m.len() / 1024).unwrap_or(0);
                            println!(" 收尾产物：{} ({} KB)", v.display(), kb);
                        }
                        None => println!(" 没有产出最终 mp4（见下方备注）"),
                    }
                    println!(" 音频轨  ：{} 路", o.audio.len());
                    for a in &o.audio {
                        println!("            {}", a.display());
                    }
                    println!(" 截图    ：{} 张", o.screenshots.len());
                    if let Some(raw) = &o.raw_video {
                        println!(" 中间视频：{}", raw.display());
                    }
                    for n in &o.notes {
                        println!(" 备注    ：{n}");
                    }
                }
                Err(e) => println!(" 收尾失败：{e}"),
            }
            Ok(())
        }
        "crash-info" => {
            println!("崩溃日志 : {}", handler.log_path().display());
            println!("已记录次数: {}", handler.crash_count());
            println!("退出码    : {}", vca_platform::crash::CRASH_EXIT_CODE);
            Ok(())
        }
        "paths" => {
            println!("配置目录 : {}", layout.config_root.display());
            println!("数据目录 : {}", layout.data_root.display());
            println!("日志目录 : {}", log_dir.display());
            Ok(())
        }
        _ => {
            println!("[debug] 未知动作: {action}");
            Ok(())
        }
    }
}

/// 清理已到期录像。
pub fn clean(layout: &Layout, dry_run: bool) -> Result<()> {
    let root = layout.data_root.clone();
    let now = vca_platform::clock::now_local();
    println!("清理扫描：{}（现在 {now}）", root.display());
    if !root.exists() {
        println!("数据目录不存在，无需清理。");
        return Ok(());
    }
    let rep = vca_core::cleanup::sweep(&root, now, dry_run);
    println!("{}", "-".repeat(62));
    println!("扫描到计划 : {}", rep.scanned);
    println!("已删除     : {}", rep.deleted.len());
    for d in &rep.deleted {
        println!("   - {d}");
    }
    println!("跳过(未到期): {}", rep.skipped.len());
    println!("删除失败   : {}", rep.failed.len());
    for (f, e) in &rep.failed {
        println!("   ! {f}: {e}");
    }
    if dry_run {
        println!("\n[dry-run] 未实际删除任何文件。");
    }
    Ok(())
}

/// 导入课表 / 时间表。
///
/// - `classisland <Default.json>`：从 ClassIsland 导入（**只读**，不改动其文件）；
/// - `csv <file.csv>`：从 CSV 导入；
/// - `template csv|timetable`：输出模板到标准输出。
pub fn import(
    layout: &Layout,
    source: &str,
    arg: Option<&str>,
    timetable_hint: Option<&str>,
) -> Result<()> {
    match source {
        "template" => {
            match arg.unwrap_or("csv") {
                "csv" => print!("{}", vca_core::import::csv_template()),
                "timetable" => print!("{}", vca_core::import::timetable_template()),
                other => {
                    eprintln!("未知模板类型: {other}（支持 csv / timetable）");
                }
            }
            Ok(())
        }
        "classisland" => {
            let path = arg
                .map(std::path::PathBuf::from)
                .ok_or_else(|| anyhow::anyhow!("请提供 ClassIsland 的 Default.json 路径"))?;
            println!("正在读取（只读，不会改动 ClassIsland 任何文件）:");
            println!("  {}", path.display());

            let imp = match vca_core::import::import_classisland(&path, timetable_hint) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("导入失败: {e}");
                    return Ok(());
                }
            };

            // 写入运行时配置 + 版本化存档（按周，不覆盖历史）
            let week = vca_platform::clock::now_local().date.week_key();
            let (tt, sc) = vca_core::import::write_current(
                &layout.config_root,
                &imp.timetable,
                &imp.schedule,
            )?;
            let tt_arch = vca_core::import::save_archive(
                &layout.timetable_archive_dir(),
                "timetable",
                &week,
                &vca_core::config::TimetableFile {
                    timetables: vec![imp.timetable.clone()],
                },
            )?;
            let sc_arch = vca_core::import::save_archive(
                &layout.schedule_archive_dir(),
                "schedule",
                &week,
                &imp.schedule,
            )?;

            print_report(&imp.report);
            println!();
            println!("已写入：");
            println!("  时间表 : {}", tt.display());
            println!("  课程表 : {}", sc.display());
            println!("  存档   : {} / {}", tt_arch.display(), sc_arch.display());
            println!();
            println!("下一步：编辑课程表，把要录的课的 record 改为 true，然后运行 `vca run`。");
            Ok(())
        }
        "csv" => {
            let path = arg
                .map(std::path::PathBuf::from)
                .ok_or_else(|| anyhow::anyhow!("请提供 CSV 文件路径"))?;
            let text = std::fs::read_to_string(&path)?;
            let rows = match vca_core::import::parse_csv(&text) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("解析失败: {e}");
                    return Ok(());
                }
            };
            let imp = vca_core::import::csv_to_schedule(&rows, &path.display().to_string());
            let week = vca_platform::clock::now_local().date.week_key();
            let sc = vca_core::import::save_archive(
                &layout.schedule_archive_dir(),
                "schedule",
                &week,
                &imp.schedule,
            )?;
            print_report(&imp.report);
            println!();
            println!("已写入存档：{}", sc.display());
            println!("提示：CSV 导入只更新课程表；如需时间表请用 `vca import template timetable` 自行填写。");
            Ok(())
        }
        other => {
            eprintln!("未知来源: {other}（支持 classisland / csv / template）");
            Ok(())
        }
    }
}

fn print_report(r: &vca_core::import::ImportReport) {
    println!("{}", "-".repeat(62));
    println!("导入报告");
    println!("  来源      : {}", r.source);
    if !r.timetable_name.is_empty() {
        println!("  时间表    : {}", r.timetable_name);
    }
    println!("  课程表份数: {}", r.class_plan_count);
    println!("  课程条目  : {}", r.entry_count);
    if r.teachers_missing.is_empty() {
        println!("  教师      : 全部已填写");
    } else {
        println!("  待补全教师: {}", r.teachers_missing.join("、"));
        println!("              （已归入 unassigned，可在课程表里改成实际教师 id）");
    }
    for w in &r.warnings {
        println!("  ! {w}");
    }
}

/// 运行调度守护进程（使用 vca-engine）。
pub fn run(layout: &Layout, schedule: Option<&str>, dry_run: bool) -> Result<()> {
    use vca_engine::{Daemon, DaemonConfig};

    // 若显式给出课程表，先同步到运行时位置
    if let Some(p) = schedule {
        let src = std::path::PathBuf::from(p);
        if src.exists() {
            let dst_dir = layout.config_root.join("schedule");
            std::fs::create_dir_all(&dst_dir)?;
            let _ = std::fs::copy(&src, dst_dir.join("current.yaml"));
            println!("已载入课程表: {}", src.display());
        }
    }

    let cfg = DaemonConfig {
        profile: std::env::var("VCA_PROFILE").unwrap_or_else(|_| "default".to_string()),
        dry_run,
        ..Default::default()
    };

    let mut d = Daemon::load(layout.clone(), cfg)?;
    println!("守护进程启动（Ctrl+C 退出）");
    println!("配置目录 : {}", layout.config_root.display());
    println!("数据目录 : {}", layout.data_root.display());
    println!("Python   : {}", d.python_dir.display());
    if dry_run {
        println!("模式     : dry-run（不真正录制、不真正推送）");
    }
    println!("{}", "-".repeat(62));
    d.run()
}
