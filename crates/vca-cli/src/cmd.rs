//! 子命令实现。
//!
//! 已实现：`doctor` / `config` / `overlay` / `run` / `clean` / `plugin` / `import` / `debug`。
//! 预留命令统一输出「预留」提示并给出对应策划书章节，避免用户误以为可用。

use anyhow::Result;

use vca_core::model::OverlaySettings;
use vca_core::paths::Layout;
use vca_platform::{doctor, overlay};

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
        println!("{}", vca_core::i18n::t("cmd.out.自动修复"));
        println!("{}", "-".repeat(62));
        let rep = vca_core::setup::repair(layout, profile);

        if rep.created_dirs.is_empty() && rep.created_files.is_empty() {
            println!(
                "{}",
                vca_core::i18n::t("cmd.out.目录与配置都已就绪无需修复")
            );
        } else {
            for d in &rep.created_dirs {
                println!(
                    "{}",
                    vca_core::i18n::tf("cmd.out.  + 新建目录  {d}", &[("d", &(d).to_string())])
                );
            }
            for f in &rep.created_files {
                println!(
                    "{}",
                    vca_core::i18n::tf("cmd.out.  + 生成文件  {f}", &[("f", &(f).to_string())])
                );
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

    println!("{}", vca_core::i18n::t("cmd.out.环境自检"));
    println!("{}", "-".repeat(62));
    let mut items = doctor::run_all();
    items.push(doctor::session_check());
    for it in &items {
        println!("{}", it.render());
    }
    println!("{}", "-".repeat(62));
    println!("{}", vca_core::i18n::t("cmd.out.说明SKIP表示该项缺失也不"));
    println!("{}", vca_core::i18n::t("cmd.out.ffmpeg缺少就没法录像；"));

    // 配置层面的检查
    let rep = vca_core::setup::repair(layout, profile);
    if !rep.needs_user.is_empty() {
        println!();
        println!("{}", vca_core::i18n::t("cmd.out.还需要你提供这些信息"));
        for (i, n) in rep.needs_user.iter().enumerate() {
            println!("  {}. {n}", i + 1);
        }
        println!();
        println!("{}", vca_core::i18n::t("cmd.out.提示运行vcasetup会逐"));
    }
    if !rep.warnings.is_empty() {
        println!();
        println!("{}", vca_core::i18n::t("cmd.out.需要注意"));
        for w in &rep.warnings {
            println!("  ! {w}");
        }
    }

    let failed = items.iter().filter(|i| !i.ok && !i.optional).count();
    if failed == 0 && rep.needs_user.is_empty() {
        println!();
        println!("{}", vca_core::i18n::t("cmd.out.一切就绪可以直接运行vcar"));
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
    println!("{}", vca_core::i18n::t("cmd.out.VibeClassAgent"));
    println!("================================================================");
    println!();
    println!(
        "{}",
        vca_core::i18n::t("cmd.out.每一步都可以直接回车跳过；跳")
    );
    println!("{}", vca_core::i18n::t("cmd.out.重新运行本向导vcasetu"));
    println!();

    // ---------- 第 1 步：自动修复 ----------
    println!("{}", vca_core::i18n::t("cmd.out.15环境检查与自动修复"));
    let rep = vca_core::setup::repair(layout, &profile);
    if rep.created_dirs.is_empty() && rep.created_files.is_empty() {
        println!("{}", vca_core::i18n::t("cmd.out.目录与配置已就绪"));
    } else {
        for f in rep.created_files.iter().take(5) {
            println!(
                "{}",
                vca_core::i18n::tf("cmd.out.      + 已生成 {f}", &[("f", &(f).to_string())])
            );
        }
        println!(
            "{}",
            vca_core::i18n::tf(
                "cmd.out.       已补齐 {p1} 个目录 / {p2} 个文件",
                &[
                    ("p1", &(rep.created_dirs.len()).to_string()),
                    ("p2", &(rep.created_files.len()).to_string())
                ]
            )
        );
    }

    // 录制能力探测：直接用 Windows 的音频 API 问，不再绕道 Python
    println!(
        "{}",
        vca_core::i18n::t("cmd.out.正在检测录屏与麦克风首次可能")
    );
    let probe = probe_recording();
    for line in probe.lines() {
        println!("      {line}");
    }
    println!();

    // ---------- 第 2 步：课表 ----------
    println!("{}", vca_core::i18n::t("cmd.out.25课表"));
    let (need_sched, _) = schedule_state(layout, &profile);
    if !need_sched {
        println!("{}", vca_core::i18n::t("cmd.out.课表已有内容"));
    } else {
        println!(
            "{}",
            vca_core::i18n::t("cmd.out.现在还没有可用的课表三种方式")
        );
        println!("{}", vca_core::i18n::t("cmd.out.a从ClassIsland导"));
        println!("{}", vca_core::i18n::t("cmd.out.b从CSV导入模板vcaim"));
        println!("        c) 稍后自己编辑 config\\schedule\\current.yaml");
        println!();
        print!("{}", vca_core::i18n::t("cmd.out.输入ClassIsland的"));
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
                    println!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out.       导入成功：{p1} 条课程",
                            &[("p1", &(imp.report.entry_count).to_string())]
                        )
                    );
                    println!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out.        时间表 {p1}",
                            &[("p1", &(tt.display()).to_string())]
                        )
                    );
                    println!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out.        课程表 {p1}",
                            &[("p1", &(sc.display()).to_string())]
                        )
                    );
                    if !imp.report.teachers_missing.is_empty() {
                        println!(
                            "{}",
                            vca_core::i18n::tf(
                                "cmd.out.        ! 这些科目没填教师：{p1}",
                                &[(
                                    "p1",
                                    &(imp
                                        .report
                                        .teachers_missing
                                        .join(vca_core::i18n::t("cmd.list_sep").as_str()))
                                    .to_string()
                                )]
                            )
                        );
                    }
                    println!();
                    println!("{}", vca_core::i18n::t("cmd.out.导入后所有课的record都"));
                    print!(
                        "{}",
                        vca_core::i18n::t("cmd.out.要现在把「全部课」都设为要录")
                    );
                    let _ = std::io::stdout().flush();
                    if read_line().trim().eq_ignore_ascii_case("y") {
                        mark_all_record(&sc)?;
                        println!("{}", vca_core::i18n::t("cmd.out.已把所有课设为recordt"));
                    }
                }
                Err(e) => println!(
                    "{}",
                    vca_core::i18n::tf("cmd.out.       导入失败：{e}", &[("e", &(e).to_string())])
                ),
            }
        }
    }
    println!();

    // ---------- 第 3 步：学期对齐 ----------
    println!(
        "{}",
        vca_core::i18n::t("cmd.out.35学期对齐决定「单周双周」")
    );
    println!(
        "{}",
        vca_core::i18n::t("cmd.out.如果课表里没有单双周这一步可")
    );
    print!("{}", vca_core::i18n::t("cmd.out.本学期第1周的周一日期如20"));
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
            print!(
                "{}",
                vca_core::i18n::t("cmd.out.该周是单周还是双周1单周2双")
            );
            let _ = std::io::stdout().flush();
            let par = if read_line().trim() == "2" {
                "even"
            } else {
                "odd"
            };
            let _ =
                vca_core::setup::patch_settings_line(&settings_path, "term.first_week_parity", par);
            // 「单/双」也是要显示给人看的字，同样不该写死在代码里
            let par_word = vca_core::i18n::t(if par == "even" {
                "cmd.word.even"
            } else {
                "cmd.word.odd"
            });
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.       已写入（第 1 周为{p1}周）",
                    &[("p1", &par_word)]
                )
            );
        } else {
            println!("{}", vca_core::i18n::t("cmd.out.日期格式不对应为YYYY-M"));
        }
    }
    println!();

    // ---------- 第 4 步：模型 API ----------
    println!("{}", vca_core::i18n::t("setup.step_model"));
    println!("      {}", vca_core::i18n::t("setup.step_model_note"));
    println!();

    // 4.1 选服务商：用内置预设，省得用户去翻各家文档
    let presets: Vec<&vca_platform::llm::ProviderPreset> = vca_platform::llm::PROVIDERS
        .iter()
        .filter(|p| !p.base_url.is_empty())
        .collect();
    println!("      {}", vca_core::i18n::t("setup.ask_provider"));
    for (i, p) in presets.iter().enumerate() {
        println!("        {:>2}) {:<20} {}", i + 1, p.name, p.note);
    }
    println!("         {}", vca_core::i18n::t("setup.provider_skip"));
    print!(
        "      {}",
        vca_core::i18n::tf("setup.provider_pick", &[("n", &presets.len().to_string())])
    );
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
        print!("{}", vca_core::i18n::t("cmd.out.APIKey回车跳过"));
        let _ = std::io::stdout().flush();
        let key = read_line().trim().to_string();
        if !key.is_empty() {
            let sp = vca_core::secrets::default_path(&layout.config_root);
            match vca_core::secrets::upsert(&sp, "VCA_LLM_API_KEY", &key) {
                Ok(()) => {
                    // 立刻注入，这样下面就能直接拿它拉模型列表
                    std::env::set_var("VCA_LLM_API_KEY", &key);
                    println!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out.       已写入 {p1}",
                            &[("p1", &(sp.display()).to_string())]
                        )
                    );
                    println!("{}", vca_core::i18n::t("cmd.out.这个文件含密钥已被gitig"));
                }
                Err(e) => {
                    println!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out.       写入失败：{e}",
                            &[("e", &(e).to_string())]
                        )
                    );
                    println!("{}", vca_core::i18n::t("cmd.out.可手动设置环境变量VCA_L"));
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
            print!("{}", vca_core::i18n::t("cmd.out.正在拉取模型列表…"));
            let _ = std::io::stdout().flush();
            match vca_platform::llm::list_models(&cfg) {
                Ok(models) if !models.is_empty() => {
                    println!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out. 找到 {p1} 个",
                            &[("p1", &(models.len()).to_string())]
                        )
                    );
                    let show = models.len().min(20);
                    for (i, m) in models.iter().take(show).enumerate() {
                        println!("        {:>2}) {m}", i + 1);
                    }
                    if models.len() > show {
                        println!(
                            "{}",
                            vca_core::i18n::tf(
                                "cmd.out.         …（共 {p1} 个，也可以直接输入模型名）",
                                &[("p1", &(models.len()).to_string())]
                            )
                        );
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
                Ok(_) => println!("{}", vca_core::i18n::t("cmd.out. 服务端没有返回模型列表")),
                Err(e) => println!(
                    "{}",
                    vca_core::i18n::tf("cmd.out. 失败：{e}", &[("e", &(e).to_string())])
                ),
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
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.      已写入配置：{p1} / {model}",
                    &[("p1", (p.name)), ("model", &(model).to_string())]
                )
            );
        }
    } else {
        println!("{}", vca_core::i18n::t("cmd.out.已跳过之后可用model配置"));
    }
    println!();

    // ---------- 第 5 步：推送 ----------
    println!("{}", vca_core::i18n::t("cmd.out.55推送渠道把文档发到微信Q"));
    println!("{}", vca_core::i18n::t("cmd.out.1企业微信机器人2通用Web"));
    print!("{}", vca_core::i18n::t("cmd.out.请选择0-3回车0"));
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
        println!(
            "{}",
            vca_core::i18n::tf("cmd.out.       已选：{pv}", &[("pv", (pv))])
        );
        println!("{}", vca_core::i18n::t("cmd.out.推送地址同样走环境变量"));
        println!("        setx VCA_PUSH_ENDPOINT \"你的Webhook地址\"");
    } else {
        println!("{}", vca_core::i18n::t("cmd.out.已跳过之后在settings"));
    }

    // ---------- 收尾 ----------
    vca_core::setup::mark_initialized(layout)?;

    println!();
    println!("================================================================");
    println!("{}", vca_core::i18n::t("cmd.out.引导完成"));
    println!("================================================================");
    println!();
    println!(
        "{}",
        vca_core::i18n::tf(
            "cmd.out.  配置目录：{p1}",
            &[("p1", &(layout.config_root.display()).to_string())]
        )
    );
    println!(
        "{}",
        vca_core::i18n::tf(
            "cmd.out.  数据目录：{p1}",
            &[("p1", &(layout.data_root.display()).to_string())]
        )
    );
    println!();
    // 这里给的是**主菜单里的序号**，不是 shell 命令。
    // 之前写的是 `vca doctor --fix` 这类外部命令 —— 用户双击进来之后根本没法执行，
    // 只能退出去再开一个 cmd 窗口，等于给了一串看不懂的提示。
    println!("{}", vca_core::i18n::t("cmd.out.接下来这样走回主菜单输"));
    println!("{}", vca_core::i18n::t("cmd.out.   3课表管理导入你的课"));
    println!("{}", vca_core::i18n::t("cmd.out.   2配置推送配好之后文"));
    println!("{}", vca_core::i18n::t("cmd.out.   4环境自检与修复再"));
    println!("{}", vca_core::i18n::t("cmd.out.   5录制测试录10秒"));
    println!("{}", vca_core::i18n::t("cmd.out.   8试运行不真录不"));
    println!("{}", vca_core::i18n::t("cmd.out.   9正式运行按课表自"));
    println!();
    println!(
        "{}",
        vca_core::i18n::t("cmd.out.双击本程序也能看到同样的菜单")
    );
    Ok(())
}

/// 清掉初始化标记，让下次启动重新走引导。
pub fn reset_setup(layout: &Layout) -> Result<()> {
    vca_core::setup::unmark_initialized(layout);
    println!(
        "{}",
        vca_core::i18n::t("cmd.out.已清除初始化标记下次运行vc")
    );
    println!(
        "{}",
        vca_core::i18n::tf(
            "cmd.out.配置目录：{p1}",
            &[("p1", &(layout.config_root.display()).to_string())]
        )
    );
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
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.配置目录 : {p1}",
                    &[("p1", &(layout.config_root.display()).to_string())]
                )
            );
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.数据目录 : {p1}",
                    &[("p1", &(layout.data_root.display()).to_string())]
                )
            );
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
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.插件目录：{p1}",
                    &[("p1", &(dir.display()).to_string())]
                )
            );
            println!(
                "{}",
                vca_core::i18n::tf("cmd.out.已加载 {n} 个插件：", &[("n", &(n).to_string())])
            );
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
                    println!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out.插件 id   : {p1}",
                            &[("p1", &(h.manifest.plugin.id).to_string())]
                        )
                    );
                    println!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out.名称      : {p1}",
                            &[("p1", &(h.manifest.plugin.name).to_string())]
                        )
                    );
                    println!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out.版本      : {p1}",
                            &[("p1", &(h.manifest.plugin.version).to_string())]
                        )
                    );
                    println!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out.接口版本  : {p1}",
                            &[("p1", &(h.manifest.plugin.api_version).to_string())]
                        )
                    );
                    println!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out.运行模式  : {p1}",
                            &[("p1", &(h.manifest.runtime.r#type).to_string())]
                        )
                    );
                    println!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out.入口      : {p1}",
                            &[("p1", &(h.manifest.runtime.entry).to_string())]
                        )
                    );
                    println!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out.目录      : {p1}",
                            &[("p1", &(h.dir.display()).to_string())]
                        )
                    );
                    if let Some(r) = &h.manifest.plugin.risk_note {
                        println!(
                            "{}",
                            vca_core::i18n::tf(
                                "cmd.out.风险提示  : {r}",
                                &[("r", &(r).to_string())]
                            )
                        );
                    }
                    Ok(())
                }
                None => {
                    println!(
                        "{}",
                        vca_core::i18n::tf("cmd.out.未找到插件：{id}", &[("id", (id))])
                    );
                    Ok(())
                }
            }
        }
        "install" => {
            let src = target.unwrap_or("");
            if src.is_empty() {
                eprintln!("{}", vca_core::i18n::t("cmd.out.用法vcapluginins"));
                return Ok(());
            }
            let market = vca_plugin_host::market::LocalMarket::new();
            let source =
                vca_plugin_host::market::MarketSource::LocalDir(std::path::PathBuf::from(src));
            match market.install(&source, &dir, false) {
                Ok(out) => {
                    println!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out.插件已安装：{p1}",
                            &[("p1", &(out.plugin_id).to_string())]
                        )
                    );
                    println!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out.安装位置  ：{p1}",
                            &[("p1", &(out.installed_to.display()).to_string())]
                        )
                    );
                    // 权限与风险必须显式展示给用户确认（策划书 4.2）
                    if !out.network.is_empty() {
                        println!(
                            "{}",
                            vca_core::i18n::tf(
                                "cmd.out.网络权限  : {p1}",
                                &[("p1", &(out.network.join(", ")).to_string())]
                            )
                        );
                    }
                    if !out.filesystem.is_empty() {
                        println!(
                            "{}",
                            vca_core::i18n::tf(
                                "cmd.out.文件权限  : {p1}",
                                &[("p1", &(out.filesystem.join(", ")).to_string())]
                            )
                        );
                    }
                    if !out.env.is_empty() {
                        println!(
                            "{}",
                            vca_core::i18n::tf(
                                "cmd.out.需要的环境变量: {p1}",
                                &[("p1", &(out.env.join(", ")).to_string())]
                            )
                        );
                    }
                    if let Some(risk) = &out.risk_note {
                        println!();
                        println!(
                            "{}",
                            vca_core::i18n::tf(
                                "cmd.out. 风险提示：{risk}",
                                &[("risk", &(risk).to_string())]
                            )
                        );
                        println!(
                            "{}",
                            vca_core::i18n::t("cmd.out.启用前请自行评估；本项目不对")
                        );
                    }
                    println!();
                    println!(
                        "{}",
                        vca_core::i18n::t("cmd.out.提示本程序不会自动执行插件需")
                    );
                    Ok(())
                }
                Err(e) => {
                    eprintln!(
                        "{}",
                        vca_core::i18n::tf("cmd.out.安装失败：{e}", &[("e", &(e).to_string())])
                    );
                    Ok(())
                }
            }
        }
        "remove" | "uninstall" => {
            let id = target.unwrap_or("");
            if id.is_empty() {
                eprintln!("{}", vca_core::i18n::t("cmd.out.用法vcapluginrem"));
                return Ok(());
            }
            let market = vca_plugin_host::market::LocalMarket::new();
            match market.uninstall(id, &dir) {
                Ok(p) => {
                    println!(
                        "{}",
                        vca_core::i18n::tf("cmd.out.已卸载：{id}", &[("id", (id))])
                    );
                    println!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out.删除目录：{p1}",
                            &[("p1", &(p.display()).to_string())]
                        )
                    );
                    Ok(())
                }
                Err(e) => {
                    eprintln!(
                        "{}",
                        vca_core::i18n::tf("cmd.out.卸载失败：{e}", &[("e", &(e).to_string())])
                    );
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
    println!(
        "{}",
        vca_core::i18n::tf(
            "cmd.out.[预留] {cmd} {action}",
            &[("cmd", (cmd)), ("action", (action))]
        )
    );
    println!(
        "{}",
        vca_core::i18n::t("cmd.out.该子命令尚未实现；当前功能已")
    );
    println!(
        "{}",
        vca_core::i18n::tf(
            "cmd.out.  设计依据：{reference}",
            &[("reference", (reference))]
        )
    );
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
            println!(
                "{}",
                vca_core::i18n::t("cmd.out.正在创建悬浮窗右上角置顶不抢")
            );
            let (win, handle) = match overlay::spawn(style) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out.创建悬浮窗失败：{e}",
                            &[("e", &(e).to_string())]
                        )
                    );
                    eprintln!(
                        "{}",
                        vca_core::i18n::t("cmd.out.提示无图形会话如远程服务会话")
                    );
                    return Ok(());
                }
            };
            println!("窗口句柄 = 0x{:X}", win.raw());
            win.show();
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.已显示，保持 {secs} 秒后自动关闭...",
                    &[("secs", &(secs).to_string())]
                )
            );
            std::thread::sleep(std::time::Duration::from_secs(secs));
            win.close();
            let _ = handle.join();
            println!("{}", vca_core::i18n::t("cmd.out.已关闭"));
            Ok(())
        }
        "plan" => {
            let path = arg
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("config/schedule.example.yaml"));
            let file = match vca_core::config::load_schedule_file(&path) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out.读取课程表失败：{e}",
                            &[("e", &(e).to_string())]
                        )
                    );
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

            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.日期：{today}（星期{p1}）",
                    &[
                        ("today", &(today).to_string()),
                        ("p1", &(today.weekday() + 1).to_string())
                    ]
                )
            );
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.悬浮窗文本：{p1}",
                    &[("p1", &(settings.text).to_string())]
                )
            );
            println!("{}", vca_core::i18n::tf("cmd.out.规则：下课后 {p1} 分钟显示，上课前 {p2} 分钟关闭；无后续课程时兜底 {p3} 分钟", &[("p1", &(settings.show_delay_minutes).to_string()), ("p2", &(settings.hide_before_minutes).to_string()), ("p3", &(settings.fallback_minutes).to_string())]));
            println!("{}", "-".repeat(62));
            if lessons.is_empty() {
                println!("{}", vca_core::i18n::t("cmd.out.当天没有课程"));
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
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.[{action}] 需要宿主进程持有窗口句柄；请用 `vca overlay demo` 验证。",
                    &[("action", (action))]
                )
            );
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
/// 密钥打码：只留长度与首尾几位，用来确认「填没填、填对没填对」。
fn mask_secret(s: &str) -> String {
    let t = s.trim();
    if t.is_empty() {
        return "（未设置）".to_string();
    }
    let n = t.chars().count();
    if n <= 8 {
        return format!("已设置（{n} 字符）");
    }
    let head: String = t.chars().take(4).collect();
    let tail: String = t.chars().skip(n - 4).collect();
    format!("{head}…{tail}（{n} 字符）")
}

/// 直接发一条测试消息，验证推送渠道到底通不通。
///
/// 为什么需要它：推送链路上任何一环不对 —— 地址、token、群号、机器人不在线、
/// NapCat 没开 HTTP 服务 —— 症状全是同一个「课后没收到消息」。
/// 等真上完一节课才发现发不出去，代价太大。这个命令让配置完就能立刻验证，
/// 而且报的是**原始错误**（HTTP 状态码 + 服务端返回体），可以直接拿去搜。
pub fn push_test(layout: &Layout, profile: &str) -> Result<()> {
    use vca_platform::push::{
        make_pusher, send_with_retry, Provider as PushProvider, PushConfig, PushDoc,
    };

    let settings_path = layout.profile_config_dir(profile).join("settings.yaml");
    let s = vca_core::config::load_settings(&settings_path).unwrap_or_default();

    println!("{}", "-".repeat(62));
    println!("{}", vca_core::i18n::t("push.test_title"));
    println!("{}", "-".repeat(62));

    let id = s.push.provider.trim().to_string();
    if id.is_empty() {
        println!("{}", vca_core::i18n::t("push.test_no_provider"));
        return Ok(());
    }
    let Some(provider) = PushProvider::parse(&id) else {
        println!("{}", vca_core::i18n::tf("push.test_unknown", &[("p", &id)]));
        println!("   可用渠道：wecom / qq / onebot / serverchan / webhook / wechat-personal");
        return Ok(());
    };

    // 与 pipeline.rs 完全一致的取值逻辑：配置优先，空则回落到环境变量。
    // 两处必须一致 —— 否则会出现「测试通过、真推送失败」这种最难查的偏差。
    let endpoint = if s.push.endpoint.trim().is_empty() {
        std::env::var("VCA_PUSH_ENDPOINT").unwrap_or_default()
    } else {
        s.push.endpoint.clone()
    };
    let token = std::env::var("VCA_PUSH_TOKEN").unwrap_or_default();

    println!("   provider          = {id}");
    println!(
        "   endpoint          = {}",
        if endpoint.is_empty() {
            "（空）"
        } else {
            &endpoint
        }
    );
    println!(
        "   target            = {}（{}）",
        s.push.target.clone().unwrap_or_default(),
        s.push.target_type
    );
    println!("   VCA_PUSH_TOKEN    = {}", mask_secret(&token));
    if provider == PushProvider::QqOfficial {
        println!(
            "   VCA_QQ_APP_ID     = {}",
            mask_secret(&std::env::var("VCA_QQ_APP_ID").unwrap_or_default())
        );
        println!(
            "   VCA_QQ_APP_SECRET = {}",
            mask_secret(&std::env::var("VCA_QQ_APP_SECRET").unwrap_or_default())
        );
    }
    println!();

    let cfg = PushConfig {
        provider,
        endpoint,
        token,
        target: s.push.target.clone().unwrap_or_default(),
        target_type: s.push.target_type.clone(),
        app_id: vca_platform::push::credentials_for(provider).0,
        app_secret: vca_platform::push::credentials_for(provider).1,
        max_retries: 1,
        timeout_ms: 30_000,
        image_card: s.push.image_card,
    };
    let pusher = make_pusher(&cfg);
    let doc = PushDoc {
        title: "VibeClassAgent 推送测试".to_string(),
        summary: "这是一条测试消息。收到它就说明推送链路是通的。".to_string(),
        docx: None,
        pdf: None,
        target: s.push.target.clone(),
        preview_url: None,
    };

    println!("{}", vca_core::i18n::t("push.test_sending"));
    let out = send_with_retry(pusher.as_ref(), &doc, 1);
    println!();
    if out.success {
        println!(
            "{}",
            vca_core::i18n::tf(
                "push.test_ok",
                &[("id", out.message_id.as_deref().unwrap_or("-"))]
            )
        );
    } else {
        println!("{}", vca_core::i18n::t("push.test_fail"));
        println!(
            "   {}",
            out.error.unwrap_or_else(|| "（没有更多信息）".to_string())
        );
    }
    println!();
    println!("{}", vca_core::i18n::t("push.test_notes"));
    Ok(())
}

pub fn debug(layout: &Layout, action: &str) -> Result<()> {
    let log_dir = layout.profile_data_dir("default").join("logs");
    let handler = vca_platform::crash::CrashHandler::new(&log_dir);

    match action {
        "push-test" => push_test(layout, "default"),
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
            println!("{}", vca_core::i18n::t("cmd.out.WASAPI音频端点"));
            println!("{}", "-".repeat(62));
            match vca_platform::audio::probe_endpoints() {
                Ok(list) if list.is_empty() => {
                    println!(
                        "{}",
                        vca_core::i18n::t("cmd.out.未发现默认音频端点系统里可能")
                    );
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
                        println!(
                            "{}",
                            vca_core::i18n::tf(
                                "cmd.out.  格式 : {p1}",
                                &[("p1", &(ep.format).to_string())]
                            )
                        );
                        println!(
                            "{}",
                            vca_core::i18n::tf(
                                "cmd.out.  端点 : {p1}",
                                &[("p1", &(ep.id).to_string())]
                            )
                        );
                    }
                    Ok(())
                }
                Err(e) => {
                    println!(
                        "{}",
                        vca_core::i18n::tf("cmd.out.探测失败：{e}", &[("e", &(e).to_string())])
                    );
                    Ok(())
                }
            }
        }
        "transcribe" => {
            // 转写冒烟测试：对指定媒体文件跑一次本地/云端转写并打印结果。
            // 用法：vca debug transcribe <文件> [work_dir]
            let media = layout.data_root.join("_record_test").join("test.mp4");
            let media = if media.is_file() {
                media
            } else {
                println!("{}", vca_core::i18n::t("cmd.out.没有可转写的文件先跑vcad"));
                return Ok(());
            };
            let work = layout.data_root.join("_record_test").join("_work_stt");

            println!("{}", vca_core::i18n::t("cmd.out.转写冒烟测试"));
            println!("{}", "-".repeat(62));
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.输入 : {p1}",
                    &[("p1", &(media.display()).to_string())]
                )
            );
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.本地 : {p1}",
                    &[(
                        "p1",
                        (if vca_platform::stt::local_available(&Default::default()) {
                            "可用"
                        } else {
                            "不可用（缺 whisper-cli.exe 或模型，可运行 scripts/fetch-deps.py）"
                        })
                    )]
                )
            );
            println!();

            let cfg = vca_platform::stt::SttConfig::new();
            let t0 = std::time::Instant::now();
            match vca_platform::stt::transcribe(&cfg, &media, &work) {
                Ok(t) => {
                    println!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out.引擎     : {p1}",
                            &[("p1", &(t.engine).to_string())]
                        )
                    );
                    println!("耗时     : {:.1}s（含转码）", t.seconds);
                    println!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out.分段     : {p1} 段",
                            &[("p1", &(t.segments.len()).to_string())]
                        )
                    );
                    println!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out.文本长度 : {p1} 字",
                            &[("p1", &(t.text.chars().count()).to_string())]
                        )
                    );
                    println!();
                    println!("{}", vca_core::i18n::t("cmd.out.---转写内容前500字--"));
                    let preview: String = t.text.chars().take(500).collect();
                    println!("{preview}");
                    if t.text.chars().count() > 500 {
                        println!("{}", vca_core::i18n::t("cmd.out.已截断"));
                    }
                }
                Err(e) => {
                    println!(
                        "{}",
                        vca_core::i18n::tf("cmd.out.转写失败：{e}", &[("e", &(e).to_string())])
                    );
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

            println!("{}", vca_core::i18n::t("cmd.out.端到端冒烟测试"));
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
                    println!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out.   录制初始化失败：{e}",
                            &[("e", &(e).to_string())]
                        )
                    );
                    return Ok(());
                }
            };
            if let Err(e) = sess.start() {
                println!();
                println!(
                    "{}",
                    vca_core::i18n::tf("cmd.out.   录制启动失败：{e}", &[("e", &(e).to_string())])
                );
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_secs(secs as u64));
            let _ = sess.stop();
            let outcome = match sess.finalize() {
                Ok(o) => o,
                Err(e) => {
                    println!();
                    println!(
                        "{}",
                        vca_core::i18n::tf("cmd.out.   收尾失败：{e}", &[("e", &(e).to_string())])
                    );
                    return Ok(());
                }
            };
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.   → 视频 {p1} / 音频 {p2} 路 / 截图 {p3} 张",
                    &[
                        (
                            "p1",
                            (if outcome.video.is_some() {
                                "有"
                            } else {
                                "无"
                            })
                        ),
                        ("p2", &(outcome.audio.len()).to_string()),
                        ("p3", &(outcome.screenshots.len()).to_string())
                    ]
                )
            );
            for n in &outcome.notes {
                println!(
                    "{}",
                    vca_core::i18n::tf("cmd.out.     备注：{n}", &[("n", &(n).to_string())])
                );
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

            println!("{}", vca_core::i18n::t("cmd.out.②课后流水线演练不真正推送"));
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
                println!(
                    "{}",
                    vca_core::i18n::tf("cmd.out.   ✗ 失败：{e}", &[("e", &(e).to_string())])
                );
            }

            // ---- 4) 产物清单 ----
            println!("{}", vca_core::i18n::t("cmd.out.③产物"));
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

            println!("{}", vca_core::i18n::t("cmd.out.录制冒烟测试"));
            println!("{}", "-".repeat(62));
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.输出目录 : {p1}",
                    &[("p1", &(out_dir.display()).to_string())]
                )
            );
            println!("{}", vca_core::i18n::t("cmd.out.时长10秒8fps720p每"));
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
                    println!(
                        "{}",
                        vca_core::i18n::tf("cmd.out.初始化失败：{e}", &[("e", &(e).to_string())])
                    );
                    return Ok(());
                }
            };
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.录制引擎 : {p1}",
                    &[("p1", &(sess.engine_label()).to_string())]
                )
            );
            println!();

            print!("{}", vca_core::i18n::t("cmd.out.正在录制"));
            use std::io::Write;
            let _ = std::io::stdout().flush();
            if let Err(e) = sess.start() {
                println!();
                println!(
                    "{}",
                    vca_core::i18n::tf("cmd.out.启动失败：{e}", &[("e", &(e).to_string())])
                );
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
                            println!(
                                "{}",
                                vca_core::i18n::tf(
                                    "cmd.out. 收尾产物：{p1} ({p2} KB)",
                                    &[
                                        ("p1", &(v.display()).to_string()),
                                        ("p2", &(kb).to_string())
                                    ]
                                )
                            );
                        }
                        None => println!(
                            "{}",
                            vca_core::i18n::t("cmd.out. 没有产出最终 mp4（见下方备注）")
                        ),
                    }
                    println!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out. 音频轨  ：{p1} 路",
                            &[("p1", &(o.audio.len()).to_string())]
                        )
                    );
                    for a in &o.audio {
                        println!("            {}", a.display());
                    }
                    println!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out. 截图    ：{p1} 张",
                            &[("p1", &(o.screenshots.len()).to_string())]
                        )
                    );
                    if let Some(raw) = &o.raw_video {
                        println!(
                            "{}",
                            vca_core::i18n::tf(
                                "cmd.out. 中间视频：{p1}",
                                &[("p1", &(raw.display()).to_string())]
                            )
                        );
                    }
                    for n in &o.notes {
                        println!(
                            "{}",
                            vca_core::i18n::tf(
                                "cmd.out. 备注    ：{n}",
                                &[("n", &(n).to_string())]
                            )
                        );
                    }
                }
                Err(e) => println!(
                    "{}",
                    vca_core::i18n::tf("cmd.out. 收尾失败：{e}", &[("e", &(e).to_string())])
                ),
            }
            Ok(())
        }
        "crash-info" => {
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.崩溃日志 : {p1}",
                    &[("p1", &(handler.log_path().display()).to_string())]
                )
            );
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.已记录次数: {p1}",
                    &[("p1", &(handler.crash_count()).to_string())]
                )
            );
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.退出码    : {p1}",
                    &[("p1", &(vca_platform::crash::CRASH_EXIT_CODE).to_string())]
                )
            );
            Ok(())
        }
        "paths" => {
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.配置目录 : {p1}",
                    &[("p1", &(layout.config_root.display()).to_string())]
                )
            );
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.数据目录 : {p1}",
                    &[("p1", &(layout.data_root.display()).to_string())]
                )
            );
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.日志目录 : {p1}",
                    &[("p1", &(log_dir.display()).to_string())]
                )
            );
            Ok(())
        }
        _ => {
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.[debug] 未知动作: {action}",
                    &[("action", (action))]
                )
            );
            Ok(())
        }
    }
}

/// 清理已到期录像。
pub fn clean(layout: &Layout, dry_run: bool) -> Result<()> {
    let root = layout.data_root.clone();
    let now = vca_platform::clock::now_local();
    println!(
        "{}",
        vca_core::i18n::tf(
            "cmd.out.清理扫描：{p1}（现在 {now}）",
            &[
                ("p1", &(root.display()).to_string()),
                ("now", &(now).to_string())
            ]
        )
    );
    if !root.exists() {
        println!("{}", vca_core::i18n::t("cmd.out.数据目录不存在无需清理"));
        return Ok(());
    }
    let rep = vca_core::cleanup::sweep(&root, now, dry_run);
    println!("{}", "-".repeat(62));
    println!(
        "{}",
        vca_core::i18n::tf(
            "cmd.out.扫描到计划 : {p1}",
            &[("p1", &(rep.scanned).to_string())]
        )
    );
    println!(
        "{}",
        vca_core::i18n::tf(
            "cmd.out.已删除     : {p1}",
            &[("p1", &(rep.deleted.len()).to_string())]
        )
    );
    for d in &rep.deleted {
        println!("   - {d}");
    }
    println!(
        "{}",
        vca_core::i18n::tf(
            "cmd.out.跳过(未到期): {p1}",
            &[("p1", &(rep.skipped.len()).to_string())]
        )
    );
    println!(
        "{}",
        vca_core::i18n::tf(
            "cmd.out.删除失败   : {p1}",
            &[("p1", &(rep.failed.len()).to_string())]
        )
    );
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
// 问一句、读一行（空白输入返回空串，调用方自行决定是否算跳过）。
fn ask(key: &str) -> Result<String> {
    use std::io::Write as _;
    print!("      {}", vca_core::i18n::t(key));
    let _ = std::io::stdout().flush();
    Ok(read_line().trim().to_string())
}

/// 交互式配置推送渠道。
///
/// 为什么把它做成一等功能：推送是「课后把文档送到老师手上」的最后一环 ——
/// 配不通的话，前面录得再认真也没人看得到。而它偏偏又是配置项最多、
/// 各家差异最大的一环：企业微信只要一个 URL，QQ 官方要 appid+secret+群号，
/// OneBot 还要本地服务地址。让用户对着示例文件逐个字段猜，
/// 实际结果就是「这软件好像没有推送功能」。
pub fn push_setup(layout: &Layout, profile: Option<&str>) -> Result<()> {
    let profile = profile.unwrap_or("default");
    let settings_path = layout.profile_config_dir(profile).join("settings.yaml");
    if !settings_path.exists() {
        let _ = vca_core::setup::repair(layout, profile);
    }
    let secrets_path = vca_core::secrets::default_path(&layout.config_root);

    println!("{}", "-".repeat(62));
    println!("{}", vca_core::i18n::t("push.setup_title"));
    println!("{}", "-".repeat(62));

    let cur = vca_core::config::load_settings(&settings_path).unwrap_or_default();
    if !cur.push.provider.trim().is_empty() {
        println!(
            "{}",
            vca_core::i18n::tf("push.current", &[("p", &cur.push.provider)])
        );
    }
    println!();

    // 顺序即推荐度：OneBot 最灵活（自建 NapCat），企业微信最省事
    let channels: &[(&str, &str, &str)] = &[
        (
            "onebot",
            "OneBot 11（NapCat / Lagrange）",
            "本地自建 QQ 机器人，功能全、可发群",
        ),
        ("wecom", "企业微信机器人", "只要一个 Webhook 地址，最省事"),
        ("qq", "QQ 官方机器人", "需开放平台审核，凭据较多"),
        ("serverchan", "Server 酱", "推到微信，只要一个 SendKey"),
        ("webhook", "通用 Webhook", "发到你自己的服务"),
        (
            "wechat-personal",
            "个人微信（第三方协议）",
            "有账号风险，请自行评估",
        ),
    ];
    for (i, (_, name, note)) in channels.iter().enumerate() {
        println!("   {:>2}) {:<30} {}", i + 1, name, note);
    }
    println!("    {:>2}) {}", 0, vca_core::i18n::t("push.skip"));
    println!();
    let idx: usize = ask(&vca_core::i18n::tf(
        "push.pick",
        &[("n", &channels.len().to_string())],
    ))?
    .parse()
    .unwrap_or(0);

    if idx == 0 || idx > channels.len() {
        println!("{}", vca_core::i18n::t("push.skipped"));
        return Ok(());
    }
    let (id, name, _) = channels[idx - 1];

    // 写配置：全部走 patch_settings_line，它按 llm./push. 这样的完整路径定位，
    // 不会误伤别的段（曾经它只看末段 key 名，把 llm.provider 写到 push 段上）。
    let set = |k: &str, v: &str| {
        let _ = vca_core::setup::patch_settings_line(&settings_path, k, v);
    };
    let mut saved_secret = false;
    let mut save = |key: &str, val: &str| -> Result<()> {
        if !val.is_empty() {
            vca_core::secrets::upsert(&secrets_path, key, val)?;
            saved_secret = true;
        }
        Ok(())
    };

    match id {
        "wecom" => {
            let url = ask("push.ask_wecom_url")?;
            if url.is_empty() {
                println!("{}", vca_core::i18n::t("push.skipped"));
                return Ok(());
            }
            set("push.provider", "\"wecom\"");
            set("push.endpoint", &format!("\"{url}\""));
        }
        "qq" => {
            let group = ask("push.ask_group")?;
            let appid = ask("push.ask_appid")?;
            let secret = ask("push.ask_secret")?;
            set("push.provider", "\"qq\"");
            set("push.target", &format!("\"{group}\""));
            set("push.target_type", "\"group\"");
            save("VCA_QQ_APP_ID", &appid)?;
            save("VCA_QQ_APP_SECRET", &secret)?;
        }
        "onebot" => {
            let ep = ask("push.ask_endpoint")?;
            let ep = if ep.is_empty() {
                "http://127.0.0.1:3000".to_string()
            } else {
                ep
            };
            let group = ask("push.ask_group")?;
            let token = ask("push.ask_token")?;
            set("push.provider", "\"onebot\"");
            set("push.endpoint", &format!("\"{ep}\""));
            set("push.target", &format!("\"{group}\""));
            set("push.target_type", "\"group\"");
            save("VCA_PUSH_TOKEN", &token)?;
        }
        "serverchan" => {
            let key = ask("push.ask_sendkey")?;
            if key.is_empty() {
                println!("{}", vca_core::i18n::t("push.skipped"));
                return Ok(());
            }
            set("push.provider", "\"serverchan\"");
            save("VCA_PUSH_TOKEN", &key)?;
        }
        "webhook" => {
            let url = ask("push.ask_webhook")?;
            if url.is_empty() {
                println!("{}", vca_core::i18n::t("push.skipped"));
                return Ok(());
            }
            let token = ask("push.ask_token")?;
            set("push.provider", "\"webhook\"");
            set("push.endpoint", &format!("\"{url}\""));
            save("VCA_PUSH_TOKEN", &token)?;
        }
        "wechat-personal" => {
            let ep = ask("push.ask_endpoint")?;
            let target = ask("push.ask_user")?;
            let token = ask("push.ask_token")?;
            set("push.provider", "\"wechat-personal\"");
            set("push.endpoint", &format!("\"{ep}\""));
            set("push.target", &format!("\"{target}\""));
            set("push.target_type", "\"private\"");
            save("VCA_PUSH_TOKEN", &token)?;
        }
        _ => {}
    }

    println!();
    println!("{}", vca_core::i18n::tf("push.done", &[("name", name)]));
    // 立刻回读一遍：确认真的写进去了（这曾经是个静默失败的地方）
    let after = vca_core::config::load_settings(&settings_path).unwrap_or_default();
    if after.push.provider.trim() != id {
        println!("{}", vca_core::i18n::t("push.write_failed"));
        return Ok(());
    }
    println!("   push.provider = {}", after.push.provider);
    if let Some(t) = &after.push.target {
        if !t.is_empty() {
            println!("   push.target   = {t}（{}）", after.push.target_type);
        }
    }
    if !after.push.endpoint.is_empty() {
        println!("   push.endpoint = {}", after.push.endpoint);
    }
    if saved_secret {
        println!(
            "{}",
            vca_core::i18n::tf(
                "push.secret_saved",
                &[("p", &secrets_path.display().to_string())]
            )
        );
    }
    println!();
    println!("{}", vca_core::i18n::t("push.verify_hint"));
    Ok(())
}

/// 课表管理：查看 / 导入 / 取模板。
///
/// 课表是「该录哪节课」的唯一依据，但它同时是**用户数据**而不是配置 ——
/// 每学期都要换、临时还要调。让它只能靠手改 yaml 是说不过去的。
pub fn schedule_menu(layout: &Layout, profile: Option<&str>) -> Result<()> {
    let profile = profile.unwrap_or("default");
    println!("{}", "-".repeat(62));
    println!("{}", vca_core::i18n::t("sched.title"));
    println!("{}", "-".repeat(62));

    // 现状：文件在哪、有几条、今天几节
    let sc_path = layout.config_root.join("schedule").join("current.yaml");
    let (ok, n) = schedule_state(layout, profile);
    println!(
        "{}",
        vca_core::i18n::tf("sched.where", &[("p", &sc_path.display().to_string())])
    );
    if ok {
        println!(
            "{}",
            vca_core::i18n::tf("sched.count", &[("n", &n.to_string())])
        );
    } else {
        println!("{}", vca_core::i18n::t("sched.empty"));
    }
    println!();

    println!("   1) {}", vca_core::i18n::t("sched.m_import_classisland"));
    println!("   2) {}", vca_core::i18n::t("sched.m_import_csv"));
    println!("   3) {}", vca_core::i18n::t("sched.m_template"));
    println!("   4) {}", vca_core::i18n::t("sched.m_open"));
    println!("   5) {}", vca_core::i18n::t("sched.m_edit"));
    println!("   0) {}", vca_core::i18n::t("sched.m_back"));
    println!();
    let pick: usize = ask("sched.pick")?.parse().unwrap_or(0);

    match pick {
        1 => {
            let p = ask("sched.ask_classisland")?;
            if p.is_empty() {
                return Ok(());
            }
            import(layout, "classisland", Some(&p), None)?;
        }
        2 => {
            let p = ask("sched.ask_csv")?;
            if p.is_empty() {
                return Ok(());
            }
            import(layout, "csv", Some(&p), None)?;
        }
        3 => {
            println!();
            print!("{}", vca_core::import::csv_template());
            println!();
            println!("{}", vca_core::i18n::t("sched.template_hint"));
        }
        4 => {
            let dir = layout.config_root.join("schedule");
            println!(
                "{}",
                vca_core::i18n::tf("sched.opened", &[("p", &dir.display().to_string())])
            );
            let _ = std::process::Command::new("explorer").arg(&dir).spawn();
        }
        5 => {
            println!(
                "{}",
                vca_core::i18n::tf("sched.edit_hint", &[("p", &sc_path.display().to_string())])
            );
        }
        _ => {}
    }
    Ok(())
}

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
                    eprintln!(
                        "{}",
                        vca_core::i18n::tf(
                            "cmd.out.未知模板类型: {other}（支持 csv / timetable）",
                            &[("other", (other))]
                        )
                    );
                }
            }
            Ok(())
        }
        "classisland" => {
            let path = arg
                .map(std::path::PathBuf::from)
                .ok_or_else(|| anyhow::anyhow!("请提供 ClassIsland 的 Default.json 路径"))?;
            println!("{}", vca_core::i18n::t("cmd.out.正在读取只读不会改动Clas"));
            println!("  {}", path.display());

            let imp = match vca_core::import::import_classisland(&path, timetable_hint) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!(
                        "{}",
                        vca_core::i18n::tf("cmd.out.导入失败: {e}", &[("e", &(e).to_string())])
                    );
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
            println!("{}", vca_core::i18n::t("cmd.out.已写入"));
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.  时间表 : {p1}",
                    &[("p1", &(tt.display()).to_string())]
                )
            );
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.  课程表 : {p1}",
                    &[("p1", &(sc.display()).to_string())]
                )
            );
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.  存档   : {p1} / {p2}",
                    &[
                        ("p1", &(tt_arch.display()).to_string()),
                        ("p2", &(sc_arch.display()).to_string())
                    ]
                )
            );
            println!();
            println!(
                "{}",
                vca_core::i18n::t("cmd.out.下一步编辑课程表把要录的课的")
            );
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
                    eprintln!(
                        "{}",
                        vca_core::i18n::tf("cmd.out.解析失败: {e}", &[("e", &(e).to_string())])
                    );
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
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.已写入存档：{p1}",
                    &[("p1", &(sc.display()).to_string())]
                )
            );
            println!("{}", vca_core::i18n::t("cmd.out.提示CSV导入只更新课程表；"));
            Ok(())
        }
        other => {
            eprintln!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.未知来源: {other}（支持 classisland / csv / template）",
                    &[("other", (other))]
                )
            );
            Ok(())
        }
    }
}

fn print_report(r: &vca_core::import::ImportReport) {
    println!("{}", "-".repeat(62));
    println!("{}", vca_core::i18n::t("cmd.out.导入报告"));
    println!(
        "{}",
        vca_core::i18n::tf(
            "cmd.out.  来源      : {p1}",
            &[("p1", &(r.source).to_string())]
        )
    );
    if !r.timetable_name.is_empty() {
        println!(
            "{}",
            vca_core::i18n::tf(
                "cmd.out.  时间表    : {p1}",
                &[("p1", &(r.timetable_name).to_string())]
            )
        );
    }
    println!(
        "{}",
        vca_core::i18n::tf(
            "cmd.out.  课程表份数: {p1}",
            &[("p1", &(r.class_plan_count).to_string())]
        )
    );
    println!(
        "{}",
        vca_core::i18n::tf(
            "cmd.out.  课程条目  : {p1}",
            &[("p1", &(r.entry_count).to_string())]
        )
    );
    if r.teachers_missing.is_empty() {
        println!("{}", vca_core::i18n::t("cmd.out.教师全部已填写"));
    } else {
        println!(
            "{}",
            vca_core::i18n::tf(
                "cmd.out.  待补全教师: {p1}",
                &[(
                    "p1",
                    &(r.teachers_missing
                        .join(vca_core::i18n::t("cmd.list_sep").as_str()))
                    .to_string()
                )]
            )
        );
        println!("{}", vca_core::i18n::t("cmd.out.已归入unassigned可"));
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
            println!(
                "{}",
                vca_core::i18n::tf(
                    "cmd.out.已载入课程表: {p1}",
                    &[("p1", &(src.display()).to_string())]
                )
            );
        }
    }

    let cfg = DaemonConfig {
        profile: std::env::var("VCA_PROFILE").unwrap_or_else(|_| "default".to_string()),
        dry_run,
        ..Default::default()
    };

    let mut d = Daemon::load(layout.clone(), cfg)?;
    println!("{}", vca_core::i18n::t("cmd.out.守护进程启动CtrlC退出"));
    println!(
        "{}",
        vca_core::i18n::tf(
            "cmd.out.配置目录 : {p1}",
            &[("p1", &(layout.config_root.display()).to_string())]
        )
    );
    println!(
        "{}",
        vca_core::i18n::tf(
            "cmd.out.数据目录 : {p1}",
            &[("p1", &(layout.data_root.display()).to_string())]
        )
    );
    println!(
        "{}",
        vca_core::i18n::tf(
            "cmd.out.Python   : {p1}",
            &[("p1", &(d.python_dir.display()).to_string())]
        )
    );
    if dry_run {
        println!("{}", vca_core::i18n::t("cmd.out.模式dry-run不真正录制"));
    }
    println!("{}", "-".repeat(62));
    d.run()
}
