//! 首次运行引导与**自动修复**。
//!
//! 设计目标：让「第一次打开软件」的人不必先读文档。
//!
//! 分两类处理：
//!
//! | 类别 | 例子 | 处理 |
//! |---|---|---|
//! | **程序能自己搞定的** | 目录不存在、配置文件没生成 | 直接创建（[`repair`]） |
//! | **只有用户能提供的** | 模型 API Key、课表、推送地址 | 收集成清单，由向导逐项询问 |
//!
//! 配置文件模板用 `include_str!` **编译进二进制**，
//! 所以即使有人只拷走了 `vca.exe`，也能自己生成出一套完整配置。

use std::path::Path;

use crate::config::{load_settings, ScheduleFile, Settings, TimetableFile};
use crate::paths::Layout;

/// 内置模板：主配置。
pub const SETTINGS_TEMPLATE: &str = include_str!("../../../config/settings.example.yaml");
/// 内置模板：时间表。
pub const TIMETABLE_TEMPLATE: &str = include_str!("../../../config/timetable.example.yaml");
/// 内置模板：课程表。
pub const SCHEDULE_TEMPLATE: &str = include_str!("../../../config/schedule.example.yaml");

/// 初始化标记文件名（放在配置根目录下）。
pub const MARKER: &str = ".initialized";

/// 一次修复的结果。
#[derive(Debug, Clone, Default)]
pub struct RepairReport {
    /// 新建的目录。
    pub created_dirs: Vec<String>,
    /// 新建的文件。
    pub created_files: Vec<String>,
    /// 修正过的内容。
    pub fixed: Vec<String>,
    /// 需要用户自己提供的东西（向导会逐项询问）。
    pub needs_user: Vec<String>,
    /// 无法自动处理的问题。
    pub warnings: Vec<String>,
}

impl RepairReport {
    /// 是否做过任何改动。
    pub fn changed(&self) -> bool {
        !self.created_dirs.is_empty() || !self.created_files.is_empty() || !self.fixed.is_empty()
    }

    /// 汇总成一行描述（日志用）。
    pub fn summary(&self) -> String {
        format!(
            "新建目录 {} 个 / 新建文件 {} 个 / 修正 {} 项 / 待用户提供 {} 项 / 警告 {} 条",
            self.created_dirs.len(),
            self.created_files.len(),
            self.fixed.len(),
            self.needs_user.len(),
            self.warnings.len()
        )
    }
}

/// 确保目录存在；返回是否**新建**了它。
fn ensure_dir(p: &Path, out: &mut RepairReport) -> bool {
    if p.is_dir() {
        return false;
    }
    match std::fs::create_dir_all(p) {
        Ok(()) => {
            out.created_dirs.push(p.display().to_string());
            true
        }
        Err(e) => {
            out.warnings
                .push(format!("创建目录失败 {}: {e}", p.display()));
            false
        }
    }
}

/// 写文件（仅当不存在时）；返回是否写了。
fn ensure_file(p: &Path, content: &str, out: &mut RepairReport) -> bool {
    if p.exists() {
        return false;
    }
    if let Some(d) = p.parent() {
        ensure_dir(d, out);
    }
    match std::fs::write(p, content) {
        Ok(()) => {
            out.created_files.push(p.display().to_string());
            true
        }
        Err(e) => {
            out.warnings.push(format!("写入失败 {}: {e}", p.display()));
            false
        }
    }
}

/// 是否已经完成过初始化。
pub fn is_initialized(layout: &Layout) -> bool {
    layout.config_root.join(MARKER).exists()
}

/// 标记为已初始化。
pub fn mark_initialized(layout: &Layout) -> std::io::Result<()> {
    std::fs::create_dir_all(&layout.config_root)?;
    let p = layout.config_root.join(MARKER);
    std::fs::write(
        p,
        format!(
            "# 本文件由 VibeClassAgent 生成，表示已完成首次初始化。\n# 删除它即可让程序下次启动时重新走引导。\nversion={}\n",
            env!("CARGO_PKG_VERSION")
        ),
    )
}

/// 取消初始化标记（用于「重新引导」）。
pub fn unmark_initialized(layout: &Layout) {
    let _ = std::fs::remove_file(layout.config_root.join(MARKER));
}

/// **核心：自动修复**。幂等，可反复调用。
///
/// 只做「不需要询问用户」的事；需要用户输入的项目收进 `needs_user`。
pub fn repair(layout: &Layout, profile: &str) -> RepairReport {
    let mut rep = RepairReport::default();

    // ---- 1) 目录骨架 ----
    for d in [
        layout.config_root.clone(),
        layout.profile_config_dir(profile),
        layout.config_root.join("timetable").join("archive"),
        layout.config_root.join("schedule").join("archive"),
        layout.data_root.clone(),
        layout.profile_data_dir(profile),
        layout.profile_data_dir(profile).join("jobs"),
        layout.profile_data_dir(profile).join("logs"),
        layout.profile_data_dir(profile).join("docs"),
    ] {
        ensure_dir(&d, &mut rep);
    }

    // ---- 2) 主配置 ----
    let settings_path = layout.profile_config_dir(profile).join("settings.yaml");
    if ensure_file(&settings_path, SETTINGS_TEMPLATE, &mut rep) {
        rep.fixed
            .push("已从内置模板生成 settings.yaml（请按需修改）".into());
    }

    // ---- 3) 时间表 ----
    let tt_path = layout.config_root.join("timetable").join("current.yaml");
    ensure_file(&tt_path, TIMETABLE_TEMPLATE, &mut rep);

    // ---- 4) 课程表 ----
    let sc_path = layout.config_root.join("schedule").join("current.yaml");
    ensure_file(&sc_path, SCHEDULE_TEMPLATE, &mut rep);

    // ---- 5) 检查配置内容，指出需要人工处理的部分 ----
    check_settings(&settings_path, &mut rep);
    check_schedule(&sc_path, &mut rep);
    check_timetable(&tt_path, &mut rep);

    // ---- 6) 数据目录可写性 ----
    let probe = layout.profile_data_dir(profile).join(".write_probe");
    match std::fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
        }
        Err(e) => rep.warnings.push(format!(
            "数据目录不可写（{}）：{e}",
            layout.data_root.display()
        )),
    }

    rep
}

/// 检查主配置，收集需要用户补的项。
fn check_settings(path: &Path, rep: &mut RepairReport) {
    let Ok(cfg) = load_settings(path) else {
        rep.warnings.push(format!(
            "settings.yaml 解析失败（{}），请检查 YAML 语法",
            path.display()
        ));
        return;
    };

    // 模型 API：Key 走环境变量，所以只检查 base_url / model 是否还是占位符
    let is_placeholder = |s: &str| {
        let t = s.trim();
        t.is_empty() || t.contains('<') || t.contains('>') || t.contains("你的")
    };
    if is_placeholder(&cfg.llm.base_url) || is_placeholder(&cfg.llm.model) {
        rep.needs_user
            .push("模型 API：base_url 与 model（用于从转写文字里提取要点）".into());
    }
    if std::env::var("VCA_LLM_API_KEY")
        .unwrap_or_default()
        .trim()
        .is_empty()
    {
        rep.needs_user
            .push("模型 API Key：设置环境变量 VCA_LLM_API_KEY".into());
    }

    // 推送
    if cfg.push.provider.trim().is_empty() {
        rep.needs_user
            .push("推送渠道：settings.yaml 的 push.provider（可先留空，之后再配）".into());
    } else if std::env::var("VCA_PUSH_ENDPOINT")
        .unwrap_or_default()
        .trim()
        .is_empty()
    {
        rep.needs_user
            .push("推送地址：设置环境变量 VCA_PUSH_ENDPOINT".into());
    }

    // 学期对齐
    if cfg
        .term
        .first_monday
        .as_deref()
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        rep.needs_user
            .push("学期对齐：settings.yaml 的 term.first_monday（用于单双周，可跳过）".into());
    }
}

/// 检查课程表：有没有至少一节要录的课。
fn check_schedule(path: &Path, rep: &mut RepairReport) {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return,
    };
    let Ok(f) = serde_yaml::from_str::<ScheduleFile>(&text) else {
        rep.warnings
            .push(format!("课程表解析失败（{}）", path.display()));
        return;
    };

    let total = f.week_template.entries.len()
        + f.weekend_template
            .as_ref()
            .map(|w| w.entries.len())
            .unwrap_or(0);
    if total == 0 {
        rep.needs_user.push(
            "课表：目前是空的，请用 `vca import classisland` 或 `vca import csv` 导入".into(),
        );
        return;
    }
    let to_record = f.week_template.entries.iter().filter(|e| e.record).count()
        + f.weekend_template
            .as_ref()
            .map(|w| w.entries.iter().filter(|e| e.record).count())
            .unwrap_or(0);
    if to_record == 0 {
        rep.needs_user.push(
            "录制开关：课表里所有课的 record 都是 false（系统不会录任何课），\
             请把要录的课改成 record: true"
                .into(),
        );
    }
}

/// 检查时间表：有没有足够的休息段来推导处理窗口。
fn check_timetable(path: &Path, rep: &mut RepairReport) {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return,
    };
    let Ok(f) = serde_yaml::from_str::<TimetableFile>(&text) else {
        rep.warnings
            .push(format!("时间表解析失败（{}）", path.display()));
        return;
    };
    let Some(tt) = f
        .timetables
        .iter()
        .find(|t| t.is_active)
        .or(f.timetables.first())
    else {
        rep.needs_user.push("时间表：文件里没有任何时间表".into());
        return;
    };
    let wins = crate::windows::derive_windows(tt, crate::windows::WindowSpec::default());
    if wins.is_empty() {
        rep.needs_user.push(
            "处理窗口：时间表里没有 >= 15 分钟的休息段，系统无法自动安排课后处理。\
             请在时间表里补上「午餐午休」等长休息段"
                .into(),
        );
    }
}

/// 读取一个可用的（已存在的）配置文件，供向导展示当前值。
pub fn peek_settings(layout: &Layout, profile: &str) -> Option<Settings> {
    let p = layout.profile_config_dir(profile).join("settings.yaml");
    load_settings(&p).ok()
}

/// 退出码/状态：是否「完全就绪」（没有任何 needs_user）。
pub fn ready(rep: &RepairReport) -> bool {
    rep.needs_user.is_empty() && rep.warnings.is_empty()
}

/// 供向导使用：把 settings.yaml 里的某个字段替换掉（简单文本替换，保留注释）。
///
/// 之所以不做 YAML 反序列化再序列化：那会**丢掉全部注释**，
/// 而示例配置里的注释正是给用户看的说明。
///
/// **必须按完整路径逐级定位，不能只看最后一段的 key 名。**
/// 旧写法是拿 `llm.model` 的末段 `model` 去匹配**第一个** `model:` 行，
/// 于是改掉了 transcriber 段的 `model: tiny`（那是 whisper 的模型名），
/// 而 llm 段真正要改的 `model: "<模型名>"` 一个字符都没动 ——
/// 用户看到的现象就是「配好的模型名，重启一遍没了」。
/// 同理 `llm.provider` 会误伤 push 段的 `provider:`，
/// 把推送渠道写成模型服务商的名字（`push.provider: deepseek`），于是推送永远发不出去。
///
/// 路径里的字段在模板中不存在时**追加到该段末尾**，而不是静默返回 false：
/// 模板里没有 `provider:` 行很正常，静默失败等于「配置压根没保存」。
pub fn patch_settings_line(path: &Path, key_path: &str, new_value: &str) -> std::io::Result<bool> {
    let text = std::fs::read_to_string(path)?;
    let mut lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
    let parts: Vec<&str> = key_path.split('.').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return Ok(false);
    }
    let leaf = *parts.last().unwrap_or(&"");

    match locate(&lines, &parts) {
        Some(li) => {
            let line = lines[li].clone();
            let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
            // 保留行尾注释（模板里的说明全靠它）
            let comment = line
                .find('#')
                .map(|i| line[i..].to_string())
                .unwrap_or_default();
            lines[li] = if comment.is_empty() {
                format!("{indent}{leaf}: {new_value}")
            } else {
                format!("{indent}{leaf}: {new_value}  {comment}")
            };
        }
        None => {
            // 父段都不存在（顶层字段名写错了，或模板缺这一整段）：
            // 宁可不写，也不要往文件里塞一个孤儿字段 —— 那会做出一个
            // 结构畸形的配置，下次解析直接失败。
            if parts.len() > 1 && locate(&lines, &parts[..parts.len() - 1]).is_none() {
                return Ok(false);
            }
            let (at, indent) = insertion_point(&lines, &parts);
            lines.insert(at, format!("{indent}{leaf}: {new_value}"));
        }
    }

    let mut s = lines.join("\n");
    s.push('\n');
    std::fs::write(path, s)?;
    Ok(true)
}

/// 按 `a.b.c` 逐级定位行号：每一级都必须落在上一级的缩进块里。
///
/// 顶层字段要求顶格（缩进 0），否则会在别人的子块里找到同名 key。
fn locate(lines: &[String], parts: &[&str]) -> Option<usize> {
    let mut start = 0usize;
    let mut parent_indent: Option<usize> = None;
    let mut hit: Option<usize> = None;

    for part in parts {
        let needle = format!("{part}:");
        let mut j = start;
        let mut found: Option<(usize, usize)> = None;
        while j < lines.len() {
            let line = &lines[j];
            let trimmed = line.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                j += 1;
                continue;
            }
            let indent = line.len() - trimmed.len();
            match parent_indent {
                Some(base) if indent <= base => break, // 已经走出父段
                None if indent != 0 => {
                    j += 1;
                    continue; // 顶层字段必须顶格
                }
                _ => {}
            }
            if trimmed.starts_with(&needle) {
                found = Some((j, indent));
                break;
            }
            j += 1;
        }
        let (li, ind) = found?;
        hit = Some(li);
        parent_indent = Some(ind);
        start = li + 1;
    }
    hit
}

/// 字段不存在时该插到哪：父段内最后一个子行的下一行（即父段末尾）。
fn insertion_point(lines: &[String], parts: &[&str]) -> (usize, String) {
    let parents: Vec<&str> = parts[..parts.len() - 1].to_vec();
    let (from, parent_indent) = if parents.is_empty() {
        (0usize, 0usize)
    } else {
        match locate(lines, &parents) {
            Some(li) => {
                let trimmed = lines[li].trim_start();
                (li + 1, lines[li].len() - trimmed.len())
            }
            None => (0usize, 0usize),
        }
    };

    let mut at = lines.len();
    for (j, line) in lines.iter().enumerate().skip(from) {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        let indent = line.len() - trimmed.len();
        if indent <= parent_indent {
            at = j;
            break;
        }
    }
    (at, " ".repeat(parent_indent + 2))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> Layout {
        let base = std::env::temp_dir().join(format!("vca_setup_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        Layout::new(base.join("data"), base.join("config"))
    }

    /// 造一个「模板已就位」的 profile，返回 (布局, settings.yaml 路径)。
    fn tmp_with_settings(tag: &str) -> (Layout, std::path::PathBuf) {
        let l = tmp(tag);
        std::fs::create_dir_all(l.profile_config_dir("default")).unwrap();
        let p = l.profile_config_dir("default").join("settings.yaml");
        std::fs::write(&p, SETTINGS_TEMPLATE).unwrap();
        (l, p)
    }

    #[test]
    fn patch_hits_the_right_section() {
        // 回归：模板里 transcriber 段有 `model: tiny`。
        // 旧实现只拿末段 key 名去匹配第一个 `model:` 行，于是把 **whisper 的
        // 模型名**改成了 LLM 模型名，而 llm 段该改的那行纹丝不动 ——
        // 用户看到的现象就是「配好的模型名，重启一遍没了」。
        let (l, p) = tmp_with_settings("patch_section");

        assert!(patch_settings_line(&p, "llm.model", "\"deepseek-chat\"").unwrap());
        let s: crate::config::Settings = load_settings(&p).unwrap();
        assert_eq!(s.llm.model, "deepseek-chat");
        assert_eq!(s.transcriber.model, "tiny", "不能误伤 whisper 的模型名");
        let _ = std::fs::remove_dir_all(&l.data_root);
    }

    #[test]
    fn patch_does_not_touch_push_provider() {
        // 回归：`llm.provider` 曾经改到 push 段的 `provider:` 上，
        // 把推送渠道写成模型服务商的名字 —— 于是推送永远发不出去，
        // 而界面上还显示「未配置推送」，用户完全找不到问题在哪。
        let (l, p) = tmp_with_settings("patch_push");

        assert!(patch_settings_line(&p, "llm.provider", "\"deepseek\"").unwrap());
        let s: crate::config::Settings = load_settings(&p).unwrap();
        assert_eq!(s.llm.provider, "deepseek");
        assert_ne!(
            s.push.provider, "deepseek",
            "push.provider 不该被模型服务商的名字污染"
        );
        let _ = std::fs::remove_dir_all(&l.data_root);
    }

    #[test]
    fn patch_appends_missing_key_into_its_section() {
        // 模板的 llm 段里没有 provider 行；这时应当**追加进 llm 段**，
        // 而不是静默返回 false（那等于「配置压根没保存」）。
        let (l, p) = tmp_with_settings("patch_append");

        assert!(patch_settings_line(&p, "llm.provider", "\"moonshot\"").unwrap());
        let s: crate::config::Settings = load_settings(&p).unwrap();
        assert_eq!(s.llm.provider, "moonshot");
        assert_ne!(s.push.provider, "moonshot", "别落到 push 段去");
        let _ = std::fs::remove_dir_all(&l.data_root);
    }

    #[test]
    fn repair_creates_everything_on_empty_layout() {
        let l = tmp("empty");
        let rep = repair(&l, "default");
        assert!(rep.created_dirs.len() >= 5, "应创建目录骨架");
        assert!(
            rep.created_files
                .iter()
                .any(|f| f.ends_with("settings.yaml")),
            "应生成 settings.yaml：{:?}",
            rep.created_files
        );
        assert!(
            rep.created_files
                .iter()
                .any(|f| f.ends_with("current.yaml")),
            "应生成 current.yaml"
        );
        // 数据目录必须可写（repair 会做一次写探针）
        assert!(
            !rep.warnings.iter().any(|w| w.contains("不可写")),
            "数据目录应可写：{:?}",
            rep.warnings
        );
        let _ = std::fs::remove_dir_all(&l.data_root);
    }

    #[test]
    fn repair_flags_empty_schedule() {
        let l = tmp("nosched");
        let _ = repair(&l, "default");
        // 人为写一份空课表
        let p = l.config_root.join("schedule").join("current.yaml");
        std::fs::write(
            &p,
            "teachers: []\nweek_template:\n  cycle: every\n  entries: []\noverrides: []\n",
        )
        .unwrap();
        let rep = repair(&l, "default");
        assert!(
            rep.needs_user.iter().any(|s| s.contains("课表")),
            "空课表应提示导入：{:?}",
            rep.needs_user
        );
        let _ = std::fs::remove_dir_all(&l.data_root);
    }

    #[test]
    fn repair_is_idempotent() {
        let l = tmp("idem");
        let a = repair(&l, "default");
        let b = repair(&l, "default");
        assert!(!a.created_files.is_empty());
        assert!(b.created_files.is_empty(), "第二次不应再创建文件");
        let _ = std::fs::remove_dir_all(&l.data_root);
    }

    #[test]
    fn repair_detects_all_record_false() {
        let l = tmp("rec");
        let _ = repair(&l, "default");
        // 人为写一份「有课但全都 record: false」的课表
        let p = l.config_root.join("schedule").join("current.yaml");
        std::fs::write(
            &p,
            "teachers:\n- { id: t1, name: A, profile: t1 }\n\
             week_template:\n  cycle: every\n  entries:\n\
             \x20 - { day: Mon, start: \"08:00\", end: \"08:45\", course: X, teacherId: t1, record: false }\n\
             overrides: []\n",
        )
        .unwrap();
        let rep = repair(&l, "default");
        assert!(
            rep.needs_user.iter().any(|s| s.contains("record")),
            "应提示录制开关未打开：{:?}",
            rep.needs_user
        );
        let _ = std::fs::remove_dir_all(&l.data_root);
    }

    #[test]
    fn repair_accepts_record_true_schedule() {
        let l = tmp("rec2");
        let _ = repair(&l, "default");
        let p = l.config_root.join("schedule").join("current.yaml");
        std::fs::write(
            &p,
            "teachers:\n- { id: t1, name: A, profile: t1 }\n\
             week_template:\n  cycle: every\n  entries:\n\
             \x20 - { day: Mon, start: \"08:00\", end: \"08:45\", course: X, teacherId: t1, record: true }\n\
             overrides: []\n",
        )
        .unwrap();
        let rep = repair(&l, "default");
        assert!(
            !rep.needs_user.iter().any(|s| s.contains("record")),
            "已勾选录制时不应再提示：{:?}",
            rep.needs_user
        );
        let _ = std::fs::remove_dir_all(&l.data_root);
    }

    #[test]
    fn marker_roundtrip() {
        let l = tmp("mark");
        assert!(!is_initialized(&l));
        mark_initialized(&l).unwrap();
        assert!(is_initialized(&l));
        unmark_initialized(&l);
        assert!(!is_initialized(&l));
        let _ = std::fs::remove_dir_all(&l.data_root);
    }

    #[test]
    fn templates_are_valid_yaml() {
        // 内置模板必须能被解析，否则生成出来的配置会让程序起不来
        let _: Settings = serde_yaml::from_str(SETTINGS_TEMPLATE).expect("settings 模板应可解析");
        let _: TimetableFile =
            serde_yaml::from_str(TIMETABLE_TEMPLATE).expect("timetable 模板应可解析");
        let _: ScheduleFile =
            serde_yaml::from_str(SCHEDULE_TEMPLATE).expect("schedule 模板应可解析");
    }

    #[test]
    fn patch_settings_replaces_value_and_keeps_comment() {
        let l = tmp("patch");
        let _ = repair(&l, "default");
        let p = l.profile_config_dir("default").join("settings.yaml");
        let ok = patch_settings_line(&p, "term.first_monday", "\"2026-09-01\"").unwrap();
        assert!(ok, "应找到 term.first_monday 并替换");
        let s = std::fs::read_to_string(&p).unwrap();
        assert!(s.contains("2026-09-01"));
        // 替换后仍能解析
        let _: Settings = serde_yaml::from_str(&s).expect("替换后仍应是合法 YAML");
        let _ = std::fs::remove_dir_all(&l.data_root);
    }

    #[test]
    fn patch_settings_appends_but_never_silently_drops() {
        // 旧语义是「找不到 key 就返回 false」，调用方一路 `let _ =` 忽略掉 ——
        // 结果就是用户以为配好了、其实一个字段都没写进去。
        // 现在的语义：**段在就把字段追加进段里**；连段都不存在才返回 false。
        let l = tmp("patch2");
        let _ = repair(&l, "default");
        let p = l.profile_config_dir("default").join("settings.yaml");

        assert!(
            !patch_settings_line(&p, "no.such.key", "1").unwrap(),
            "连顶层段都没有，就别往文件里塞孤儿字段"
        );
        assert!(
            patch_settings_line(&p, "llm.extra_field", "\"x\"").unwrap(),
            "段存在、字段缺失时必须真的写进去"
        );

        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("extra_field: \"x\""), "缺字段应被追加");
        assert!(!text.contains("no.such.key"), "不存在的那条不该落盘");
        let s: Settings = serde_yaml::from_str(&text).expect("追加后仍是合法 YAML");
        assert!(!s.llm.model.is_empty(), "原有字段不受影响");
        let _ = std::fs::remove_dir_all(&l.data_root);
    }

    #[test]
    fn summary_is_readable() {
        let mut r = RepairReport::default();
        r.created_dirs.push("a".into());
        r.needs_user.push("b".into());
        let s = r.summary();
        assert!(s.contains("新建目录 1"));
        assert!(s.contains("待用户提供 1"));
        assert!(r.changed());
    }
}
