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
    /// 企业微信：是否先把摘要渲染成一张卡片图再发（默认开）。
    ///
    /// 群机器人的 image 消息是 base64 直传的，**不需要公网图床** ——
    /// 手机上扫一眼就能看完要点，比一屏纯文字友好得多。
    #[serde(default = "default_true")]
    pub image_card: bool,
    /// 内网预览地址前缀，例如 `http://192.168.1.23:8765/preview`。
    ///
    /// 留空 = 推送里不带链接。程序开启「允许局域网访问」时会自动写进来。
    /// 只对**同一个局域网内**的收件人有效 —— 教室一体机没有公网 IP。
    #[serde(default)]
    pub preview_base: String,
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
    /// 允许局域网内的其它设备打开预览页（默认关）。
    ///
    /// 开了之后界面服务会绑到所有网卡，并自动写入 `push.preview_base`。
    /// 教室一体机与收件人通常在同一个校园网里，这是「发一个能在线看的链接」
    /// 唯一不需要公网的做法。默认关是因为它把端口暴露给了整个局域网。
    #[serde(default)]
    pub allow_lan: bool,
    /// 预览页的长期令牌（与每次启动都变的界面令牌分开）。
    ///
    /// 为什么不用界面那个令牌：它是**每次启动随机生成**的，而推送发生在
    /// daemon 里 —— 那个进程根本拿不到它，链接就拼不出来。
    /// 这个是生成一次就存下来的，daemon 与界面读同一份。
    #[serde(default)]
    pub preview_token: String,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            tray_icon: true,
            tray_badge: false,
            allow_lan: false,
            preview_token: String::new(),
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
    ///
    /// 对应 ClassIsland 的触发规则：`TimeRule.WeekDay` 不匹配时这份课表**不激活**，
    /// 也就是那天按它排不出任何课（`weekday = 0` 表示每天，是默认值）。
    ///
    /// 为什么把规则判断放在这里而不是排课里：ClassIsland 的 `WeekDay` 是
    /// **整份课表**的触发条件，而不是某一条课的属性；放在这一层语义才一致。
    pub fn plan_for_weekday(&self, weekday: u32) -> ClassPlan {
        if !self.week_template.time_rule.match_weekday(weekday) {
            return ClassPlan {
                cycle: self.week_template.cycle.clone(),
                ..ClassPlan::default()
            };
        }
        if weekday >= 5 {
            if let Some(w) = &self.weekend_template {
                return ClassPlan {
                    cycle: self.week_template.cycle.clone(),
                    entries: w.entries.clone(),
                    ..ClassPlan::default()
                };
            }
        }
        self.week_template.clone()
    }

    /// 把「新旧两套写法」统一成 `entries`，幂等。
    ///
    /// 对应 ClassIsland 的加载流程：`Classes` 是按时间点索引的课程格，
    /// 渲染与调度前会先取出课表绑定的时间表，把索引还原成起止时间。
    /// VCA 这里做的是同一件事，只是统一摊平进 `entries`，
    /// 好让排课逻辑（[`crate::schedule`]）对两种数据形状完全无感。
    ///
    /// 同时兼容旧格式：只写了 `period`（节次）没写 `start`/`end` 的条目由时间表补全，
    /// 只写了 `start` 没写节次的条目反过来补出节次 —— 两边的信息本来就是一回事。
    pub fn normalize(&mut self, timetables: &[Timetable]) {
        let tt = timetables
            .iter()
            .find(|t| t.is_active)
            .or(timetables.first());
        let weekday = plan_weekday(&mut self.week_template);
        if let Some(w) = self.weekend_template.as_mut() {
            let mut plan = ClassPlan {
                entries: std::mem::take(&mut w.entries),
                ..ClassPlan::default()
            };
            normalize_plan(&mut plan, tt, weekday);
            w.entries = plan.entries;
        }
        normalize_plan(&mut self.week_template, tt, weekday);
        for ov in self.overrides.iter_mut() {
            if let Some(t) = ov.target.as_mut() {
                let mut plan = ClassPlan {
                    entries: vec![std::mem::take(t)],
                    ..ClassPlan::default()
                };
                normalize_plan(&mut plan, tt, weekday);
                if let Some(e) = plan.entries.into_iter().next() {
                    *t = e;
                }
            }
        }
    }
}

/// 取出课表写在 `day` 上的 `TimeRule.WeekDay`（1=周一 … 7=周日，0=每天）。
fn plan_weekday(plan: &mut ClassPlan) -> Option<u32> {
    let w = plan.day.take().and_then(|v| v.as_u64())? as u32;
    plan.time_rule.weekday = w;
    Some(w)
}

/// 把 `classes` 摊平进 `entries`，并补全缺的节次 / 时间 / 星期。
fn normalize_plan(plan: &mut ClassPlan, tt: Option<&Timetable>, weekday: Option<u32>) {
    for d in std::mem::take(&mut plan.classes) {
        for (i, c) in d.slots.into_iter().enumerate() {
            plan.entries.push(ClassEntry {
                day: d.day.clone(),
                // 课程格按上课时间点顺序排列，索引即第几节；
                // 但格子上写了 period 时以格子为准（从 ClassIsland 导出的数据会带）
                period: Some(if c.period > 0 {
                    c.period as u32
                } else {
                    i as u32 + 1
                }),
                start: String::new(),
                end: String::new(),
                course: c.subject,
                teacher_id: c.teacher_id,
                record: c.record,
                merge: c.merge,
                room: None,
                cycle: c.cycle,
            });
        }
    }
    for e in plan.entries.iter_mut() {
        if let Some(tt) = tt {
            fill_from_timetable(e, tt);
        }
        if e.day.trim().is_empty() {
            if let Some(w) = weekday.filter(|w| (1..=7).contains(w)) {
                e.day = crate::schedule::weekday_name(w - 1).to_string();
            }
        }
    }
}

/// 用时间表补全一条课程条目里缺的节次与起止时间。
fn fill_from_timetable(e: &mut ClassEntry, tt: &Timetable) {
    // 节次优先：新格式只写第几节，时间是算出来的，不需要也不应该让人再抄一遍
    if let Some(p) = e.period {
        if let Some(s) = tt.class_slot(p.saturating_sub(1) as usize) {
            e.start = s.start.clone();
            e.end = s.end.clone();
            return;
        }
    }
    if e.start.trim().is_empty() {
        return;
    }
    let Some(idx) = tt.class_slot_index_at(&e.start) else {
        return;
    };
    e.period = Some(idx as u32 + 1);
    if let Some(s) = tt.class_slot(idx) {
        e.end = s.end.clone();
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

/// 读时间表列表；文件不存在或读不动都返回空表。
///
/// 「课表要把第几节还原成起止时间」这条链路在 daemon 与界面里都要用，
/// 统一走这里，免得各处各写一遍 `if exists`。
pub fn load_timetables(path: &std::path::Path) -> Vec<Timetable> {
    load_timetable_file(path)
        .map(|tf| tf.timetables)
        .unwrap_or_default()
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
                Ok(mut f) => {
                    // 存档里可能是新格式（只写第几节），过一遍归一化再取条目，
                    // 否则拿到的条目没有起止时间，排课时会当作坏条目丢掉
                    f.normalize(&[]);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DayClasses, SlotKind, TimeRule, TimetableSlot};

    fn tt() -> Timetable {
        Timetable {
            id: "default".into(),
            name: "作息".into(),
            is_active: true,
            source: "manual".into(),
            group: "global".into(),
            slots: vec![
                TimetableSlot {
                    period: Some(1),
                    start: "08:00".into(),
                    end: "08:40".into(),
                    kind: SlotKind::Class,
                    ..TimetableSlot::default()
                },
                TimetableSlot {
                    start: "08:40".into(),
                    end: "08:50".into(),
                    kind: SlotKind::Break,
                    name: Some("课间".into()),
                    ..TimetableSlot::default()
                },
                TimetableSlot {
                    period: Some(2),
                    start: "08:50".into(),
                    end: "09:30".into(),
                    kind: SlotKind::Class,
                    ..TimetableSlot::default()
                },
            ],
        }
    }

    fn sched(yaml: &str) -> ScheduleFile {
        serde_yaml::from_str(yaml).expect("课表应能解析")
    }

    #[test]
    fn normalize_fills_times_from_timetable() {
        // 新格式的核心：课表只写第几节，时间由时间表算出来
        let mut f = sched(
            "week_template:\n  entries:\n    - { day: Mon, period: 2, course: 数学, teacherId: t1 }\n",
        );
        f.normalize(&[tt()]);
        let e = &f.week_template.entries[0];
        assert_eq!(e.start, "08:50", "第 2 节的开始时间应由时间表给出");
        assert_eq!(e.end, "09:30", "课间不占节次，第 2 节仍是真正的第 2 节");
        assert_eq!(e.course, "数学");
    }

    #[test]
    fn normalize_recovers_period_from_start_time() {
        // 旧格式只写了起止时间，反过来把节次补出来（升级时用）
        let mut f = sched(
            "week_template:\n  entries:\n    - { day: Tue, start: \"08:00\", end: \"08:40\", course: 语文, teacherId: t1 }\n",
        );
        f.normalize(&[tt()]);
        assert_eq!(f.week_template.entries[0].period, Some(1));
    }

    #[test]
    fn normalize_flattens_day_classes() {
        // ClassIsland 那种「按星期分块」的写法，与 entries 完全等价
        let mut f = sched(
            "week_template:\n  classes:\n    - day: Wed\n      slots:\n        - { period: 2, subject: 英语, teacherId: t2, record: false }\n",
        );
        f.normalize(&[tt()]);
        let e = &f.week_template.entries[0];
        assert_eq!((e.day.as_str(), e.period), ("Wed", Some(2)));
        assert_eq!((e.course.as_str(), e.start.as_str()), ("英语", "08:50"));
        assert!(!e.record);
        assert!(f.week_template.classes.is_empty(), "摊平后不该留两份数据");
    }

    #[test]
    fn normalize_is_idempotent() {
        let mut f = sched(
            "week_template:\n  entries:\n    - { day: Mon, period: 1, course: x, teacherId: t }\n",
        );
        f.normalize(&[tt()]);
        let once = serde_yaml::to_string(&f).unwrap();
        f.normalize(&[tt()]);
        assert_eq!(once, serde_yaml::to_string(&f).unwrap(), "读两次不该改两遍");
    }

    #[test]
    fn weekday_number_becomes_day_name() {
        // ClassIsland 的 TimeRule.WeekDay 是数字，落成 VCA 的星期名
        let mut f = sched(
            "week_template:\n  day: 3\n  entries:\n    - { period: 1, course: x, teacherId: t }\n",
        );
        f.normalize(&[tt()]);
        assert_eq!(f.week_template.time_rule.weekday, 3);
        assert_eq!(f.week_template.entries[0].day, "Wed");
    }

    /// 一份「每天都有课」的最小课表，用来单独验触发规则。
    fn one_lesson() -> ScheduleFile {
        sched("week_template:\n  entries:\n    - { day: Mon, period: 1, start: \"08:00\", end: \"08:40\", course: 语文, teacherId: t1 }\n")
    }

    fn monday() -> crate::time::LocalDate {
        crate::time::LocalDate::new(2025, 3, 17)
    }

    #[test]
    fn trigger_weekday_blocks_other_days() {
        let mut f = one_lesson();
        assert_eq!(
            f.plan_for_weekday(0).entries.len(),
            1,
            "默认 weekday=0 表示每天"
        );
        // 改成「只在周二生效」（ClassIsland 的 2 = 周二）
        f.week_template.time_rule.weekday = 2;
        assert!(f.plan_for_weekday(0).entries.is_empty(), "周一不该生效");
        assert_eq!(f.plan_for_weekday(1).entries.len(), 1, "周二才生效");
    }

    #[test]
    fn trigger_week_count_rotates_in_planning() {
        use crate::model::CycleRule;
        let mut f = one_lesson();
        // 只在「第 2 周」上（配合双周轮换）
        f.week_template.time_rule.week_count = CycleRule { week: 2, total: 2 };
        let plan = f.plan_for_weekday(0);
        let run = |p: crate::schedule::WeekParity| {
            crate::schedule::plan_for_date(monday(), p, &plan, &[], false).len()
        };
        assert_eq!(run(crate::schedule::WeekParity::Odd), 0, "单周不该上课");
        assert_eq!(run(crate::schedule::WeekParity::Even), 1, "双周才上课");
    }

    #[test]
    fn unknown_week_number_does_not_drop_lessons() {
        use crate::model::CycleRule;
        let mut f = one_lesson();
        // 三周及以上轮换：VCA 算不出「现在是第几教学周」，必须放行而不是漏课
        f.week_template.time_rule.week_count = CycleRule { week: 3, total: 4 };
        let plan = f.plan_for_weekday(0);
        for p in [
            crate::schedule::WeekParity::Every,
            crate::schedule::WeekParity::Odd,
            crate::schedule::WeekParity::Even,
        ] {
            assert_eq!(
                crate::schedule::plan_for_date(monday(), p, &plan, &[], false).len(),
                1,
                "算不出周次时应照常上课（{p:?}）"
            );
        }
    }

    #[test]
    fn trigger_date_range_limits_plan() {
        let mut f = one_lesson();
        f.week_template.time_rule.from = Some(crate::time::LocalDate::new(2025, 4, 1));
        let plan = f.plan_for_weekday(0);
        assert_eq!(
            crate::schedule::plan_for_date(
                monday(),
                crate::schedule::WeekParity::Every,
                &plan,
                &[],
                false
            )
            .len(),
            0,
            "3 月还没到生效期"
        );
        let later = crate::time::LocalDate::new(2025, 4, 7);
        assert_eq!(
            crate::schedule::plan_for_date(
                later,
                crate::schedule::WeekParity::Every,
                &plan,
                &[],
                false
            )
            .len(),
            1,
            "4 月的周一应生效"
        );
    }

    #[test]
    fn time_rule_matches_weekday_and_rotation() {
        use crate::model::CycleRule;
        let r = TimeRule {
            weekday: 3, // 周三
            week_count: CycleRule { week: 1, total: 2 },
            ..TimeRule::default()
        };
        let wed = crate::time::LocalDate::new(2025, 3, 19);
        assert_eq!(wed.weekday(), 2, "2025-03-19 是周三");
        assert!(r.matches(wed, 1), "第 1 周应命中");
        assert!(!r.matches(wed, 2), "第 2 周不该命中");
        let thu = crate::time::LocalDate::new(2025, 3, 20);
        assert!(!r.matches(thu, 1), "星期四不该命中");
    }

    #[test]
    fn timetable_issues_flag_overlap_and_missing_class() {
        let t = Timetable {
            slots: vec![
                TimetableSlot {
                    start: "08:00".into(),
                    end: "09:00".into(),
                    kind: SlotKind::Break,
                    ..TimetableSlot::default()
                },
                TimetableSlot {
                    start: "08:30".into(),
                    end: "09:30".into(),
                    kind: SlotKind::Break,
                    ..TimetableSlot::default()
                },
            ],
            ..Timetable::default()
        };
        let issues = t.issues();
        assert!(
            issues.iter().any(|s| s.contains("没有上课节")),
            "{issues:?}"
        );
        assert!(issues.iter().any(|s| s.contains("重叠")), "{issues:?}");
        assert!(tt().issues().is_empty(), "正常作息不该报问题");
    }

    #[test]
    fn duration_comes_from_start_and_end() {
        let s = tt().slots[0].clone();
        assert_eq!(s.duration_minutes(), 40);
    }

    #[test]
    fn day_classes_round_trips_through_yaml() {
        // 写出去的格式要能读回来（界面保存后立刻回读校验，这条是它的保证）
        let plan = ClassPlan {
            time_layout_id: "default".into(),
            classes: vec![DayClasses {
                day: "Mon".into(),
                slots: vec![crate::model::ClassSlot {
                    period: 1,
                    subject: "语文".into(),
                    teacher_id: "t1".into(),
                    record: true,
                    merge: false,
                    cycle: None,
                }],
            }],
            ..ClassPlan::default()
        };
        let y = serde_yaml::to_string(&plan).unwrap();
        let back: ClassPlan = serde_yaml::from_str(&y).unwrap();
        assert_eq!(back.time_layout_id, "default");
        assert_eq!(back.classes[0].slots[0].subject, "语文");
    }
}
