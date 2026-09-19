//! 配置模型与加载。
//!
//! 设计要点（策划书 9、8.4）：
//! - 主配置 `settings.yaml` 可入库，**不含任何密钥**；
//! - 密钥一律走环境变量或 `secrets.env`，且 `secrets.env` 被 .gitignore 排除；
//! - 多教师共用：每个 profile 一套配置，互相隔离。
//!
//! 已实现：`load_settings` / `load_schedule_file` / `load_timetable_file` 负责读取，
//! `Settings::validate` 负责校验，`apply_performance_profile` 负责应用性能预设。

use serde::{Deserialize, Serialize};

use crate::model::{ClassEntry, ClassPlan, Override, Teacher, Timetable};

/// 顶层配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    /// 运行身份（多教师共用的数据隔离键）。
    #[serde(default)]
    pub profile: String,
    /// 录制参数。
    #[serde(default)]
    pub record: RecordSettings,
    /// 转写参数（默认纯本地，零费用）。
    #[serde(default)]
    pub transcriber: TranscriberSettings,
    /// 模型 API 配置（仅存非敏感字段；Key 走环境变量）。
    #[serde(default)]
    pub llm: LlmSettings,
    /// 推送配置。
    #[serde(default)]
    pub push: PushSettings,
    /// 文档导出配置。
    #[serde(default)]
    pub doc: DocSettings,
    /// 清理策略。
    #[serde(default)]
    pub cleanup: CleanupSettings,
    /// 性能预设。
    #[serde(default)]
    pub performance: PerformanceSettings,
    /// 界面选项。
    #[serde(default)]
    pub ui: UiSettings,
    /// 悬浮窗设置。
    #[serde(default)]
    pub overlay: crate::model::OverlaySettings,
    /// 处理窗口。
    #[serde(default)]
    pub processing_windows: Vec<crate::model::ProcessingWindow>,
    /// 学期对齐（用于单双周）。
    #[serde(default)]
    pub term: TermConfig,
    /// 插件路由：把某个处理步骤交给插件执行。
    #[serde(default)]
    pub plugins: PluginRouting,
}

/// 录制参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordSettings {
    /// 帧率。极简默认 8。
    pub fps: u32,
    /// 分辨率高度。极简默认 720。
    pub height: u32,
    /// 恒定质量因子。极简默认 30。
    pub crf: u32,
    /// 截图间隔（秒）。极简默认 180。
    pub screenshot_interval_secs: u32,
    /// 音频来源：`system` / `mic` / `both`。
    pub audio_source: String,
    /// 显示器索引。
    pub monitor: u32,
}

impl Default for RecordSettings {
    fn default() -> Self {
        Self {
            fps: 8,
            height: 720,
            crf: 30,
            screenshot_interval_secs: 180,
            audio_source: "both".to_string(),
            monitor: 0,
        }
    }
}

/// 转写参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriberSettings {
    /// 引擎：`local` / `cloud` / `auto`（本地可用则本地）。默认 local。
    pub engine: String,
    /// 本地模型规格：`tiny` / `base` / `small`。
    pub model: String,
    /// 语言；`auto` 表示自动检测。
    pub language: String,
    /// 线程数。0 表示自动（核数一半，上限 4）。
    pub threads: u32,
    /// 本地转写时长上限（秒）。超过就不本地跑，避免把上课用的机器占死。0 表示不限。
    #[serde(default = "default_local_max_seconds")]
    pub max_seconds: u64,
    /// 云端转写的 base url（留空用 OpenAI 官方地址）。
    #[serde(default)]
    pub cloud_base_url: String,
    /// 云端转写的模型名。
    #[serde(default = "default_cloud_stt_model")]
    pub cloud_model: String,
}

fn default_local_max_seconds() -> u64 {
    2 * 60 * 60
}

fn default_cloud_stt_model() -> String {
    "whisper-1".to_string()
}

impl Default for TranscriberSettings {
    fn default() -> Self {
        Self {
            engine: "local".to_string(),
            model: "tiny".to_string(),
            language: "zh".to_string(),
            threads: 0,
            max_seconds: default_local_max_seconds(),
            cloud_base_url: String::new(),
            cloud_model: default_cloud_stt_model(),
        }
    }
}

/// 模型 API 配置（**不含密钥**）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlmSettings {
    /// 厂商预设 id（`deepseek` / `zhipu` / `dashscope` / …）。
    /// 填了它且 base_url 为空时，自动套用预设里的地址。
    #[serde(default)]
    pub provider: String,
    /// 提取模式：`paid-api`（默认） / `free-api` / `browser-bot`。
    #[serde(default = "default_llm_mode")]
    pub mode: String,
    /// OpenAI 兼容的 base URL。
    pub base_url: String,
    /// 模型名。
    pub model: String,
    /// 提示词模板路径。
    #[serde(default)]
    pub prompt_template: Option<String>,
    /// 浏览器模式配置（仅 mode = browser-bot 时使用）。
    #[serde(default)]
    pub browser: BrowserSettings,
    /// DeepSeek 高峰时段是否积压请求、等闲时再发（**默认开**）。
    ///
    /// DeepSeek 的高峰是 UTC 01:00–04:00 与 06:00–10:00、周一至周五，
    /// 换算成北京时间就是工作日的 09:00–12:00 与 14:00–18:00（其余时间半价）。
    /// 开着它，撞上高峰的处理任务会被推迟到闲时再跑 —— 代价是文档晚一点到手。
    #[serde(default = "default_true")]
    pub defer_on_peak: bool,
    /// 中国法定节假日（`YYYY-MM-DD`，**本地日期**），这些天全天按闲时算。
    ///
    /// 程序不内置节假日日历：它每年都在变，写死了迟早过期，
    /// 过期后还会静默按错误规则跑。需要的用户在配置里补几行即可；
    /// 不填就是「只认周末」。
    #[serde(default)]
    pub peak_holidays: Vec<String>,
}

fn default_llm_mode() -> String {
    "paid-api".to_string()
}

/// 浏览器自动化（网页版聊天机器人）配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserSettings {
    /// 站点 id：`kimi` / `deepseek` / `tongyi` / `doubao` / `chatgpt` / `custom`。
    #[serde(default = "default_browser_site")]
    pub site: String,
    /// 是否用无界面模式（首次登录需要手动跑一次登录命令）。
    #[serde(default = "default_true")]
    pub headless: bool,
    /// 风险确认：必须显式改成 true 才允许启用浏览器模式。
    #[serde(default)]
    pub risk_ack: bool,
    /// 浏览器数据目录（保存登录态）。留空则用工具目录下的 browser-profile。
    #[serde(default)]
    pub user_data_dir: String,
    /// 单次问答超时（秒）。
    #[serde(default = "default_browser_timeout")]
    pub timeout_sec: u64,
    /// 选择器覆盖，形如 `input=textarea;send=.btn`。
    #[serde(default)]
    pub selectors: String,
}

fn default_browser_site() -> String {
    "kimi".to_string()
}

fn default_browser_timeout() -> u64 {
    180
}

fn default_true() -> bool {
    true
}

impl Default for BrowserSettings {
    fn default() -> Self {
        Self {
            site: default_browser_site(),
            headless: true,
            risk_ack: false,
            user_data_dir: String::new(),
            timeout_sec: default_browser_timeout(),
            selectors: String::new(),
        }
    }
}

/// 推送配置（**不含密钥**）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PushSettings {
    /// 渠道 id：`wecom` / `qq`(官方) / `onebot`(第三方) / `wechat-personal` / `webhook`。
    pub provider: String,
    /// 目标：QQ 号 / 群号 / openid。
    #[serde(default)]
    pub target: Option<String>,
    /// 目标类型：`private`（默认）或 `group`。
    #[serde(default = "default_target_type")]
    pub target_type: String,
    /// 推送失败重试次数。
    #[serde(default = "default_retry")]
    pub max_retries: u32,
    /// 渠道地址（Webhook URL / OneBot 基址）。敏感时留空并走环境变量。
    #[serde(default)]
    pub endpoint: String,
}

fn default_target_type() -> String {
    "private".to_string()
}

fn default_retry() -> u32 {
    3
}

/// 文档导出配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocSettings {
    /// 导出格式：`md` / `docx` / `pdf`。`md` 总会生成。
    pub formats: Vec<String>,
    /// 每节课最多嵌入截图数（去重之后的上限）。
    pub max_screenshots: u32,
    /// 是否把完整转写作为附录写进文档。
    #[serde(default = "default_true")]
    pub include_transcript: bool,
}

impl Default for DocSettings {
    fn default() -> Self {
        Self {
            formats: vec!["md".to_string(), "docx".to_string(), "pdf".to_string()],
            max_screenshots: 6,
            include_transcript: true,
        }
    }
}

/// 清理策略。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupSettings {
    /// 保留小时数。策划书定为 72。
    pub retention_hours: u32,
    /// 倒计时起点，固定 `push-success`（推送成功时刻）。
    pub start_from: String,
}

impl Default for CleanupSettings {
    fn default() -> Self {
        Self {
            retention_hours: 72,
            start_from: "push-success".to_string(),
        }
    }
}

/// 性能预设。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceSettings {
    /// 预设名：`minimal`（默认）或 `standard`。
    pub profile: String,
}

impl Default for PerformanceSettings {
    fn default() -> Self {
        Self {
            profile: "minimal".to_string(),
        }
    }
}

/// 界面选项。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiSettings {
    /// 是否显示小任务栏图标。
    pub tray_icon: bool,
    /// 是否显示录制中角标（默认关）。
    pub tray_badge: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            tray_icon: true,
            tray_badge: false,
        }
    }
}

// ---------------------------------------------------------------------------
// 配置文件结构（与 config/*.example.yaml 一一对应）
// ---------------------------------------------------------------------------

/// 时间表文件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimetableFile {
    /// 时间表列表。
    #[serde(default)]
    pub timetables: Vec<Timetable>,
}

/// 周末模板。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeekendTemplate {
    /// `new` 新建 / `inherit` 沿用历史。
    #[serde(default)]
    pub source: String,
    /// `source = inherit` 时指定历史周次或版本。
    #[serde(default)]
    pub inherit_from: Option<String>,
    /// 条目。
    #[serde(default)]
    pub entries: Vec<ClassEntry>,
}

/// 课程表文件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleFile {
    /// 教师名单。
    #[serde(default)]
    pub teachers: Vec<Teacher>,
    /// 工作日模板。
    pub week_template: ClassPlan,
    /// 周末模板。
    #[serde(default)]
    pub weekend_template: Option<WeekendTemplate>,
    /// 临时停课 / 调课。
    #[serde(default)]
    pub overrides: Vec<Override>,
}

impl ScheduleFile {
    /// 取指定日期适用的课程表：周六/周日用周末模板，其余用工作日模板。
    pub fn plan_for_weekday(&self, weekday: u32) -> ClassPlan {
        if weekday >= 5 {
            if let Some(w) = &self.weekend_template {
                return ClassPlan {
                    cycle: self.week_template.cycle.clone(),
                    entries: w.entries.clone(),
                };
            }
        }
        self.week_template.clone()
    }
}

/// 加载错误。
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// 读取失败。
    #[error("读取文件失败 {path}: {source}")]
    Io {
        /// 路径。
        path: String,
        /// 底层错误。
        source: std::io::Error,
    },
    /// 解析失败。
    #[error("解析 YAML 失败 {path}: {source}")]
    Yaml {
        /// 路径。
        path: String,
        /// 底层错误。
        source: serde_yaml::Error,
    },
}

fn read_to_string(path: &std::path::Path) -> Result<String, LoadError> {
    std::fs::read_to_string(path).map_err(|e| LoadError::Io {
        path: path.display().to_string(),
        source: e,
    })
}

/// 保存主配置为 YAML。
///
/// 返回 `Result<(), String>` 而不是自定义错误类型：调用方都是 CLI，
/// 只需要把原因原样展示给用户，没必要再包一层。
pub fn save_settings(path: &std::path::Path, s: &Settings) -> Result<(), String> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).map_err(|e| format!("创建配置目录失败: {e}"))?;
    }
    let text = serde_yaml::to_string(s).map_err(|e| format!("序列化配置失败: {e}"))?;
    std::fs::write(path, text).map_err(|e| format!("写入配置失败: {e}"))?;
    Ok(())
}

/// 从 YAML 文件加载主配置。
pub fn load_settings(path: &std::path::Path) -> Result<Settings, LoadError> {
    let text = read_to_string(path)?;
    serde_yaml::from_str(&text).map_err(|e| LoadError::Yaml {
        path: path.display().to_string(),
        source: e,
    })
}

/// 从 YAML 文件加载时间表。
pub fn load_timetable_file(path: &std::path::Path) -> Result<TimetableFile, LoadError> {
    let text = read_to_string(path)?;
    serde_yaml::from_str(&text).map_err(|e| LoadError::Yaml {
        path: path.display().to_string(),
        source: e,
    })
}

/// 从 YAML 文件加载课程表。
pub fn load_schedule_file(path: &std::path::Path) -> Result<ScheduleFile, LoadError> {
    let text = read_to_string(path)?;
    serde_yaml::from_str(&text).map_err(|e| LoadError::Yaml {
        path: path.display().to_string(),
        source: e,
    })
}

/// 以 YAML 序列化（用于存档）。
pub fn to_yaml<T: Serialize>(value: &T) -> Result<String, serde_yaml::Error> {
    serde_yaml::to_string(value)
}

impl Settings {
    /// 从 JSON 字符串解析（供测试与插件传参）。
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// 校验配置，返回问题列表（空表示通过）。
    pub fn validate(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.record.fps == 0 || self.record.fps > 60 {
            v.push(format!("record.fps 越界: {}", self.record.fps));
        }
        if self.cleanup.retention_hours == 0 {
            v.push("cleanup.retention_hours 必须为正".to_string());
        }
        if self.cleanup.start_from != "push-success" {
            v.push(format!(
                "cleanup.start_from 目前仅支持 push-success，当前为 {}",
                self.cleanup.start_from
            ));
        }
        if !matches!(self.performance.profile.as_str(), "minimal" | "standard") {
            v.push(format!("未知性能预设: {}", self.performance.profile));
        }
        if !matches!(self.transcriber.engine.as_str(), "local" | "cloud") {
            v.push(format!("未知转写引擎: {}", self.transcriber.engine));
        }
        v.extend(self.overlay.validate());
        for (i, w) in self.processing_windows.iter().enumerate() {
            if crate::time::LocalDateTime::parse_hhmm(&w.start).is_none()
                || crate::time::LocalDateTime::parse_hhmm(&w.end).is_none()
            {
                v.push(format!("处理窗口 {} 的时间格式错误", i + 1));
            }
            if w.max_concurrent == 0 {
                v.push(format!("处理窗口 {} 的 max_concurrent 必须为正", i + 1));
            }
        }
        v
    }

    /// 应用性能预设（极简 / 标准）。
    pub fn apply_performance_profile(&mut self) {
        if self.performance.profile == "minimal" {
            self.record.fps = 8;
            self.record.height = 720;
            self.record.crf = 30;
            self.record.screenshot_interval_secs = 180;
            self.transcriber.model = "tiny".to_string();
            self.doc.max_screenshots = 4;
        }
    }
}

/// 学期对齐配置：用于把「单周/双周」映射到真实日期。
///
/// 各校的「第 1 周」定义不同，且第一周是单周还是双周也不同，
/// 因此必须由用户显式给出，程序不猜。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TermConfig {
    /// 本学期第 1 周的周一日期（`YYYY-MM-DD`）。
    #[serde(default)]
    pub first_monday: Option<String>,
    /// 第 1 周是单周还是双周：`odd` / `even`。
    #[serde(default = "default_parity")]
    pub first_week_parity: String,
}

fn default_parity() -> String {
    "odd".to_string()
}

impl Default for TermConfig {
    fn default() -> Self {
        Self {
            first_monday: None,
            first_week_parity: "odd".to_string(),
        }
    }
}

impl TermConfig {
    /// 给定日期是否为单周（依据学期对齐配置计算）。
    ///
    /// 未配置 `first_monday` 时返回 `None`，调用方应回退到
    /// 「按自然周序号奇偶」这条宽松规则。
    pub fn is_odd_week(&self, date: crate::time::LocalDate) -> Option<bool> {
        let first = self.first_monday.as_deref()?;
        let first = crate::time::LocalDateTime::parse(&format!("{first} 00:00"))?.date;
        let diff_days = date.to_days() - first.to_days();
        if diff_days < 0 {
            return None;
        }
        let week_index = diff_days / 7; // 0 = 第 1 周
        let first_odd = self.first_week_parity != "even";
        let this_odd = if first_odd {
            week_index % 2 == 0
        } else {
            week_index % 2 == 1
        };
        Some(this_odd)
    }
}

impl ScheduleFile {
    /// 解析周末模板：处理 `source: inherit` 的**自动沿用历史**。
    ///
    /// - `source: new`：直接用 `weekend_template.entries`；
    /// - `source: inherit`：从 `archive_dir` 里找 `inherit_from` 指定的周次存档，
    ///   读取其中的周末条目；找不到则回退到本地 `entries` 并给出提示。
    pub fn resolve_weekend(
        &self,
        archive_dir: &std::path::Path,
    ) -> (Vec<ClassEntry>, Option<String>) {
        let Some(w) = &self.weekend_template else {
            return (Vec::new(), None);
        };
        if w.source != "inherit" {
            return (w.entries.clone(), None);
        }
        let Some(key) = w.inherit_from.as_deref().filter(|s| !s.trim().is_empty()) else {
            return (
                w.entries.clone(),
                Some(
                    "weekend_template.source = inherit，但未填 inherit_from，已改用本地 entries"
                        .into(),
                ),
            );
        };

        // 在存档目录里找该周次的所有版本，取版本号最大的那个
        let mut best: Option<(u32, std::path::PathBuf)> = None;
        if let Ok(rd) = std::fs::read_dir(archive_dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if !name.starts_with(&format!("schedule_{key}_")) || !name.ends_with(".yaml") {
                    continue;
                }
                let ver = name
                    .trim_start_matches(&format!("schedule_{key}_"))
                    .trim_end_matches(".yaml")
                    .parse::<u32>()
                    .unwrap_or(0);
                if best.as_ref().map(|(b, _)| ver > *b).unwrap_or(true) {
                    best = Some((ver, e.path()));
                }
            }
        }

        match best {
            Some((ver, path)) => match load_schedule_file(&path) {
                Ok(f) => {
                    let entries = f
                        .weekend_template
                        .as_ref()
                        .map(|x| x.entries.clone())
                        .unwrap_or_default();
                    (
                        entries,
                        Some(format!(
                            "周末课表已沿用存档 {key} 第 {ver} 版（{}）",
                            path.display()
                        )),
                    )
                }
                Err(e) => (
                    w.entries.clone(),
                    Some(format!("读取存档失败（{e}），已改用本地 entries")),
                ),
            },
            None => (
                w.entries.clone(),
                Some(format!("在存档目录里没找到周次 {key}，已改用本地 entries")),
            ),
        }
    }
}

/// 插件路由：把处理流水线的某一步交给外部插件执行。
///
/// 留空表示用内置实现（Rust 内置推送 / Python 转写与文档）。
/// 填插件 id（如 `pusher.wecom`）后，该步骤改由 `plugins/<id>/` 里的进程完成。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginRouting {
    /// 推送步骤使用的插件 id。
    #[serde(default)]
    pub push: String,
    /// 转写步骤使用的插件 id。
    #[serde(default)]
    pub transcribe: String,
    /// 文档生成步骤使用的插件 id。
    #[serde(default)]
    pub docgen: String,
}

impl PluginRouting {
    /// 是否有任何步骤被路由到插件。
    pub fn any(&self) -> bool {
        !self.push.is_empty() || !self.transcribe.is_empty() || !self.docgen.is_empty()
    }

    /// 返回已启用的 (步骤名, 插件 id) 列表。
    pub fn entries(&self) -> Vec<(&'static str, &str)> {
        let mut v = Vec::new();
        if !self.push.is_empty() {
            v.push(("push", self.push.as_str()));
        }
        if !self.transcribe.is_empty() {
            v.push(("transcribe", self.transcribe.as_str()));
        }
        if !self.docgen.is_empty() {
            v.push(("docgen", self.docgen.as_str()));
        }
        v
    }
}
