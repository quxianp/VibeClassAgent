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
    /// 依赖下载源（镜像）自定义。
    ///
    /// 为什么要让用户自己填：程序要下载 ffmpeg / whisper / 模型（约 125 MB），
    /// 而**没有任何一组内置地址能在所有网络里都通** —— 教育网、单位内网、
    /// 只有自建镜像的环境各不相同。写死地址的后果是"在某些学校永远装不上"，
    /// 而用户明明知道哪个地址是通的。
    #[serde(default)]
    pub download: DownloadSettings,
}

/// 依赖下载源设置。
///
/// 语义是**覆盖**而不是替换：用户填的源会排在内置源**前面**先试，
/// 内置源仍然保底。这样填错了也只是"多等一会儿"，不会把程序弄成
/// 完全下不动 —— 用户填错地址是很常见的（手抖、复制少一段）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DownloadSettings {
    /// 各依赖的自定义下载地址，键是依赖 id（`ffmpeg` / `whisper` / `model`）。
    ///
    /// 允许填多个（界面里一行一个）：按顺序试，第一个通了就用。
    /// 用 `BTreeMap` 而不是 `HashMap` 是为了序列化后键序稳定 ——
    /// 配置文件给人看、也进版本管理，每次打开顺序都在变很烦。
    #[serde(default)]
    pub mirrors: std::collections::BTreeMap<String, Vec<String>>,
    /// 全局前置镜像（前缀形式），会拼在内置地址前面。
    ///
    /// 这是给"我知道一个 GitHub 加速域名"的场景准备的：不想给每个依赖
    /// 单独填地址，只想让所有 GitHub 直链都过一遍加速。
    /// 例：填 `https://ghfast.top/` 就会生成
    /// `https://ghfast.top/https://github.com/...`。
    #[serde(default)]
    pub prefix: Vec<String>,
    /// 是否只用自定义源（true = 不试内置地址）。
    ///
    /// 默认 false。给"内网完全出不去、只有自建镜像通"的环境用 ——
    /// 那种网络里试内置地址要等到超时才换下一个，白等很久。
    #[serde(default)]
    pub only_custom: bool,
}

impl DownloadSettings {
    /// 这个依赖有没有自定义源。
    pub fn custom_for(&self, dep_id: &str) -> &[String] {
        self.mirrors
            .get(dep_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// 是否什么都没配（用来跳过处理、少几次分配）。
    pub fn is_empty(&self) -> bool {
        self.mirrors.values().all(|v| v.is_empty()) && self.prefix.is_empty()
    }
}

/// 自定义下载源是否可用（非空、是 http(s)、没有明显的手抖）。
///
/// # 为什么要校验
///
/// 用户填错地址是**大概率事件**（少个斜杠、复制到一半、把页面地址当直链）。
/// 不校验的话表现是"点了下载，进度条不动，等十分钟超时"，用户不知道
/// 是自己填错了还是网络问题。
///
/// # 为什么校验要宽松
///
/// 只挡**明显不可能工作**的：不是 http(s)、有空格、没有主机名。
/// 不挡"看起来对但实际不通"的 —— 那只能靠真的去连（见 `probe_mirror`），
/// 而且自建镜像的路径形态千奇百怪，收窄规则只会误伤。
pub fn validate_mirror_url(url: &str) -> Result<(), String> {
    let u = url.trim();
    if u.is_empty() {
        return Err("地址是空的".to_string());
    }
    if u.contains(char::is_whitespace) {
        return Err("地址里有空格（多半是复制时带进来的）".to_string());
    }
    if u.contains('\\') {
        return Err("地址里不能有反斜杠，请使用 /".to_string());
    }

    let Some((scheme, rest)) = u.split_once("://") else {
        return Err("必须以 http:// 或 https:// 开头".to_string());
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err("只支持 http:// 或 https:// 协议".to_string());
    }

    // authority 到第一个 / ? # 为止。只按 '/' 切会把 `https://?x=1`
    // 错当成“主机名是 ?x=1”，保存后才在下载时失败。
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() {
        return Err("缺少主机名".to_string());
    }
    // URL 里的 user:password@host 会被进度日志与错误信息原样显示，
    // 很容易泄露凭据；镜像鉴权应走代理/网络层，不把密码塞进 URL。
    if authority.contains('@') {
        return Err("地址不能包含账号或密码".to_string());
    }

    // IPv6 必须写成 [::1]:8080；裸 IPv6 与 host:port 无法可靠区分。
    if authority.starts_with('[') {
        let Some(close) = authority.find(']') else {
            return Err("IPv6 地址缺少右方括号 ]".to_string());
        };
        if close == 1 {
            return Err("IPv6 主机名是空的".to_string());
        }
        let tail = &authority[close + 1..];
        if !tail.is_empty()
            && (!tail.starts_with(':')
                || tail[1..].is_empty()
                || !tail[1..].chars().all(|c| c.is_ascii_digit()))
        {
            return Err("端口必须是数字".to_string());
        }
    } else {
        if authority.matches(':').count() > 1 {
            return Err("IPv6 地址请使用 [地址]:端口 的写法".to_string());
        }
        let (host, port) = authority
            .rsplit_once(':')
            .map_or((authority, None), |(h, p)| (h, Some(p)));
        if host.is_empty() {
            return Err("主机名是空的（只有一个端口号）".to_string());
        }
        if let Some(port) = port {
            if port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()) {
                return Err("端口必须是数字".to_string());
            }
        }
    }
    Ok(())
}

/// 校验“全局加速前缀”。
///
/// 前缀会继续拼接完整的默认 URL，所以不能带查询串或锚点；例如
/// `https://mirror.example/?url=` 会被拼成 `...?url=/https://...`，语义不确定。
/// 需要这类特殊镜像时，应在 `mirrors` 里为每个依赖填写完整下载地址。
pub fn validate_mirror_prefix(url: &str) -> Result<(), String> {
    validate_mirror_url(url)?;
    if url.contains('?') || url.contains('#') {
        return Err("全局前缀不能带 ? 查询参数或 # 锚点；请改填完整下载地址".to_string());
    }
    Ok(())
}

/// 把前缀规范成**恰好一个**结尾斜杠。
///
/// 用户填 `https://mirror.example`、`.../` 或 `...///` 都得到同一结果，
/// 避免拼接成 `examplehttps://...` 或产生重复候选。
pub fn normalize_mirror_prefix(url: &str) -> String {
    format!("{}/", url.trim().trim_end_matches('/'))
}

/// 把自定义源和内置源合成最终要试的列表。
///
/// # 顺序规则（这是本函数唯一重要的地方）
///
/// 1. 用户给这个依赖单独填的地址（最优先 —— 他最清楚哪个能用）
/// 2. 前缀镜像 + 内置地址（用户说"所有 GitHub 都走这个加速"）
/// 3. 内置地址原样（保底）
///
/// `only_custom` 为 true 时**只保留第 1、2 步**，并且把前缀镜像也保留 ——
/// 那是用户明确要求的方式，不是"内置源"。
///
/// 去重后返回：用户可能既填了具体地址又填了前缀，撞车时没必要试两遍。
pub fn merge_sources(dep_id: &str, builtin: &[&str], cfg: &DownloadSettings) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let push = |s: String, out: &mut Vec<String>| {
        let s = s.trim().to_string();
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    };

    // 1) 该依赖的自定义地址。
    // settings.yaml 允许用户手工编辑，所以即使 API 已校验，这里仍再守一道：
    // 非 http(s) / 明显畸形的值绝不能进入真实下载器。
    for u in cfg.custom_for(dep_id) {
        if validate_mirror_url(u).is_ok() {
            push(u.clone(), &mut out);
        }
    }

    for b in builtin {
        // 2) 前缀镜像 + 内置地址。
        // 结尾有没有斜杠都统一规范；非法手工配置安静跳过，继续走保底源。
        for pfx in &cfg.prefix {
            if validate_mirror_prefix(pfx).is_ok() {
                push(format!("{}{}", normalize_mirror_prefix(pfx), b), &mut out);
            }
        }

        // 3) 内置原样。用户明确勾了 only_custom 才不加。
        if !cfg.only_custom {
            push((*b).to_string(), &mut out);
        }
    }

    out
}

#[cfg(test)]
mod download_tests {
    use super::*;

    fn cfg_with(pairs: &[(&str, &[&str])], prefix: &[&str], only: bool) -> DownloadSettings {
        let mut mirrors = std::collections::BTreeMap::new();
        for (k, v) in pairs {
            mirrors.insert(
                (*k).to_string(),
                v.iter().map(|s| (*s).to_string()).collect::<Vec<_>>(),
            );
        }
        DownloadSettings {
            mirrors,
            prefix: prefix.iter().map(|s| (*s).to_string()).collect(),
            only_custom: only,
        }
    }

    /// 用户填的源必须**排在最前面**。
    ///
    /// 这是整个功能的立足点：用户填这个地址，就是因为**只有它通**。
    /// 排在后面等于没填 —— 程序还是会先去试那个不通的内置地址等到超时。
    #[test]
    fn custom_source_comes_first() {
        let cfg = cfg_with(&[("ffmpeg", &["https://my.mirror/ff.zip"])], &[], false);
        let got = merge_sources("ffmpeg", &["https://builtin/a.zip"], &cfg);
        assert_eq!(
            got[0], "https://my.mirror/ff.zip",
            "自定义源没排第一：{got:?}"
        );
        assert!(
            got.contains(&"https://builtin/a.zip".to_string()),
            "内置源被弄丢了"
        );
    }

    /// 没填自定义源时，行为必须和以前**一模一样**。
    ///
    /// 这是个向后兼容的开关，不能让没填的人受影响。
    #[test]
    fn empty_config_keeps_builtin_order() {
        let cfg = DownloadSettings::default();
        let got = merge_sources("ffmpeg", &["https://a/1.zip", "https://b/2.zip"], &cfg);
        assert_eq!(got, vec!["https://a/1.zip", "https://b/2.zip"]);
    }

    /// `only_custom` = true 时不能出现内置原始地址。
    ///
    /// 内网环境用这个：试不通的地址要等到超时才换，白等很久。
    #[test]
    fn only_custom_drops_builtin() {
        let cfg = cfg_with(&[("ffmpeg", &["https://my/ff.zip"])], &[], true);
        let got = merge_sources("ffmpeg", &["https://builtin/a.zip"], &cfg);
        assert_eq!(got, vec!["https://my/ff.zip"]);
        assert!(!got.contains(&"https://builtin/a.zip".to_string()));
    }

    /// 前缀镜像要正确拼接（这是最容易写错的一处）。
    #[test]
    fn prefix_is_joined_with_slash() {
        // 带斜杠的
        let cfg = cfg_with(&[], &["https://ghfast.top/"], false);
        let got = merge_sources("whisper", &["https://github.com/x/y.zip"], &cfg);
        assert!(
            got.contains(&"https://ghfast.top/https://github.com/x/y.zip".to_string()),
            "带斜杠前缀拼错了：{got:?}"
        );

        // 不带斜杠的也必须能拼对（用户很常见地会漏掉）
        let cfg2 = cfg_with(&[], &["https://ghfast.top"], false);
        let got2 = merge_sources("whisper", &["https://github.com/x/y.zip"], &cfg2);
        assert!(
            got2.contains(&"https://ghfast.top/https://github.com/x/y.zip".to_string()),
            "不带斜杠前缀拼错了（漏斜杠会拼成 ghfast.tophttps://）：{got2:?}"
        );
    }

    /// 前缀镜像下，内置原样地址也要保留（保底还是要有的）。
    #[test]
    fn prefix_keeps_builtin_too() {
        let cfg = cfg_with(&[], &["https://p/"], false);
        let got = merge_sources("model", &["https://h/a.bin"], &cfg);
        assert_eq!(got.len(), 2, "应当既有前缀版也有原版：{got:?}");
        assert!(got.contains(&"https://h/a.bin".to_string()));
    }

    /// 重复地址只试一次。
    #[test]
    fn duplicates_are_removed() {
        let cfg = cfg_with(&[("ffmpeg", &["https://same/a.zip"])], &[], false);
        let got = merge_sources("ffmpeg", &["https://same/a.zip"], &cfg);
        assert_eq!(got, vec!["https://same/a.zip"], "重复没去掉：{got:?}");
    }

    /// 校验要挡住明显填错的，但不能误伤正常的。
    #[test]
    fn url_validation_blocks_obvious_mistakes() {
        // 该挡的
        for bad in [
            "",
            "ftp://x/a.zip",
            "example.com/a.zip",
            "https://a b/c.zip",
            "https://",
            "https://?file=x.zip",
            "https://:8080/a.zip",
            "https://host:abc/a.zip",
            "https://user:password@host/a.zip",
            r"https://host\path\a.zip",
        ] {
            assert!(
                validate_mirror_url(bad).is_err(),
                "明显非法的地址被放过了：{bad}"
            );
        }

        // 不该挡的（自建镜像形态很多，别误伤）
        for good in [
            "https://my.mirror/a.zip",
            "http://192.168.1.10:8080/files/a.zip",
            "https://hf-mirror.com/x/y/resolve/main/z.bin",
            "HTTP://intranet.local:8080/a.zip",
            "http://[::1]:8080/a.zip",
        ] {
            assert!(
                validate_mirror_url(good).is_ok(),
                "合法镜像被误伤了：{good}"
            );
        }
    }

    /// 前缀可带或不带结尾斜杠，保存/拼接后都规范成一个斜杠。
    #[test]
    fn prefix_normalization_handles_trailing_slashes() {
        assert_eq!(
            normalize_mirror_prefix("https://mirror.example"),
            "https://mirror.example/"
        );
        assert_eq!(
            normalize_mirror_prefix(" https://mirror.example/// "),
            "https://mirror.example/"
        );
        assert!(validate_mirror_prefix("https://mirror.example/").is_ok());
        assert!(
            validate_mirror_prefix("https://mirror.example/?url=").is_err(),
            "带查询串的值不是可安全拼接的前缀"
        );
    }

    /// 用户手改 settings.yaml 可能绕过 API；真实下载合并时仍要丢掉非法源。
    #[test]
    fn merge_drops_invalid_manually_edited_sources() {
        let cfg = cfg_with(
            &[("ffmpeg", &["ftp://bad/a.zip", "https://good/a.zip"])],
            &["file:///bad"],
            false,
        );
        let got = merge_sources("ffmpeg", &["https://builtin/a.zip"], &cfg);
        assert_eq!(
            got,
            vec!["https://good/a.zip", "https://builtin/a.zip"],
            "非法手工配置不应进入下载器：{got:?}"
        );
    }

    /// 没配置时要能被识别为"空"，方便调用方跳过处理。
    #[test]
    fn empty_detection() {
        assert!(DownloadSettings::default().is_empty());
        let c = cfg_with(&[("ffmpeg", &["https://a/b"])], &[], false);
        assert!(!c.is_empty());
        let c2 = cfg_with(&[], &["https://p/"], false);
        assert!(!c2.is_empty(), "只填前缀也算配置过");
    }

    /// 键顺序稳定（配置文件给人看，顺序乱跳很烦）。
    #[test]
    fn serialization_order_is_stable() {
        let mut c = DownloadSettings::default();
        c.mirrors.insert("whisper".into(), vec!["https://w".into()]);
        c.mirrors.insert("ffmpeg".into(), vec!["https://f".into()]);
        c.mirrors.insert("model".into(), vec!["https://m".into()]);

        let y1 = serde_yaml::to_string(&c).expect("序列化");
        let y2 = serde_yaml::to_string(&c).expect("序列化");
        assert_eq!(y1, y2, "两次序列化结果不同，说明键顺序不稳定");

        // BTreeMap 应当是按字母序
        let fi = y1.find("ffmpeg").expect("有 ffmpeg");
        let mi = y1.find("model").expect("有 model");
        let wi = y1.find("whisper").expect("有 whisper");
        assert!(
            fi < mi && mi < wi,
            "顺序不是字母序：
{y1}"
        );
    }
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
    /// 是否允许局域网内的其它设备打开预览页。
    ///
    /// # 当前状态：未实现，保留字段仅为兼容旧配置
    ///
    /// 这个开关曾承诺「绑所有网卡 + 自动写入 `push.preview_base`」，
    /// 但 `vca_gui::server` 始终只监听 127.0.0.1，绑定逻辑从未接上 ——
    /// 开了它不会有任何效果（评审 R-08）。
    ///
    /// 保留字段而不是删掉，是因为旧配置里已经写进去了：删字段会让 serde
    /// 直接报错，用户升级后连程序都起不来。界面上该开关已移除。
    ///
    /// 真要开放局域网，必须连同「预览令牌强制校验 + 绑定地址显式确认」
    /// 一起做，不能只翻这一个布尔值。
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

    /// 解析某个课表（ClassPlan）实际应绑定的时间表。
    ///
    /// # 为什么不能只看 is_active
    ///
    /// 一份课表用 time_layout_id 指明自己的作息（ClassIsland 的
    /// ClassPlan.TimeLayoutId）。只要用户维护了不止一份时间表，
    /// "当前激活的那份"和"这份课表绑的那份"就可能是两个不同的东西，
    /// 旧实现一律取 active（没有则取第一个），于是把所有课表的 period
    /// 都按另一份时间表换算：09:00 上课的班被排成 08:30（评审 R-02）。
    ///
    /// 优先级：
    /// 1. time_layout_id 非空 —— 必须按 id 找到，找不到返回 None；
    /// 2. 没写 id —— 退回 active，再退回第一个（兼容单表的老配置）。
    ///
    /// 第 1 条找不到时**故意返回 None 而不是 fallback**：拿错误的时间去排课，
    /// 比明确报"绑定的时间表不存在"危险得多 —— 后者用户一眼看懂，
    /// 前者会安静地录错课。
    pub fn resolve_timetable<'a>(
        plan: &ClassPlan,
        timetables: &'a [Timetable],
    ) -> Option<&'a Timetable> {
        let want = plan.time_layout_id.trim();
        if !want.is_empty() {
            return timetables.iter().find(|t| t.id == want);
        }
        timetables
            .iter()
            .find(|t| t.is_active)
            .or_else(|| timetables.first())
    }

    /// 把「新旧两种数据形状」统一成 `entries`，幂等。
    ///
    /// 对应 ClassIsland 的加载流程：`Classes` 是按时间点索引的课程格，
    /// 渲染与调度前会先读时间表，把索引还原成起止时间。
    /// VCA 这边只认 `start` / `end`（一件事只有一种表达），
    /// 好让排课逻辑（[`crate::schedule`]）对两种数据形状完全无感。
    ///
    /// 同时兼容旧格式：只写了 `period`（节次）没写 `start`/`end` 的条目
    /// 由时间表补全，只写了 `start` 没写节次的条目反过来补出节次。
    ///
    /// 三处（周中 / 周末 / 覆盖项）各按**自己绑定的时间表**换算（评审 R-02）。
    pub fn normalize(&mut self, timetables: &[Timetable]) {
        let weekday = plan_weekday(&mut self.week_template);
        let tt = Self::resolve_timetable(&self.week_template, timetables);
        if let Some(w) = self.weekend_template.as_mut() {
            let mut plan = ClassPlan {
                entries: std::mem::take(&mut w.entries),
                // 周末模板自己不带绑定，沿用周中课表的
                time_layout_id: self.week_template.time_layout_id.clone(),
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
                    time_layout_id: self.week_template.time_layout_id.clone(),
                    ..ClassPlan::default()
                };
                // 覆盖项走自己的解析：它带 time_layout_id 时按自己的来
                let ov_tt = Self::resolve_timetable(&plan, timetables).or(tt);
                normalize_plan(&mut plan, ov_tt, weekday);
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

    // ---- resolve_timetable（评审 R-02）----
    //
    // 这一组是为了钉住「多份时间表时按绑定选表」这个行为：
    // 旧实现一律取 active，导致绑了别的表的课表被按错误的时间换算。

    /// 造一份 id 不同、作息不同的时间表（第 1 节 09:00 开始）。
    fn tt_late() -> Timetable {
        Timetable {
            id: "late".into(),
            name: "晚作息".into(),
            // 刻意设成非 active：用来验证「绑定优先于 active」
            is_active: false,
            source: "manual".into(),
            group: "global".into(),
            slots: vec![TimetableSlot {
                period: Some(1),
                start: "09:00".into(),
                end: "09:45".into(),
                kind: SlotKind::Class,
                ..TimetableSlot::default()
            }],
        }
    }

    #[test]
    fn resolve_prefers_bound_layout_over_active() {
        // active 是 08:00 那份，但课表明确绑了 09:00 那份
        let tables = vec![tt(), tt_late()];
        let plan = ClassPlan {
            time_layout_id: "late".into(),
            ..ClassPlan::default()
        };
        let got = ScheduleFile::resolve_timetable(&plan, &tables).expect("应能按 id 找到");
        assert_eq!(got.id, "late", "绑定优先于 is_active");
    }

    #[test]
    fn resolve_falls_back_to_active_without_binding() {
        let tables = vec![tt(), tt_late()];
        let plan = ClassPlan::default(); // 没写 time_layout_id
        let got = ScheduleFile::resolve_timetable(&plan, &tables).expect("应退回 active");
        assert_eq!(got.id, "default");
    }

    #[test]
    fn resolve_returns_none_for_dangling_binding() {
        // 绑了一个不存在的时间表：必须返回 None，而不是悄悄用别的表
        let tables = vec![tt()];
        let plan = ClassPlan {
            time_layout_id: "missing".into(),
            ..ClassPlan::default()
        };
        assert!(
            ScheduleFile::resolve_timetable(&plan, &tables).is_none(),
            "绑定不存在时不能 fallback 到 active —— 那会安静地排错课"
        );
    }

    #[test]
    fn normalize_uses_bound_layout_times() {
        // 端到端：绑了 late 的课表，第 1 节应拿到 09:00 而不是 active 的 08:00
        let mut f = sched(
            "week_template:\n  timeLayoutId: late\n  entries:\n    - { day: Mon, period: 1, course: 数学, teacherId: t1 }\n",
        );
        f.normalize(&[tt(), tt_late()]);
        let e = &f.week_template.entries[0];
        assert_eq!(e.start, "09:00", "应按绑定的时间表补时间（评审 R-02）");
        assert_eq!(e.end, "09:45");
    }

    #[test]
    fn normalize_leaves_times_unfilled_when_binding_dangling() {
        // 绑定悬空时不做换算：宁可留空让校验报出来，也不要按拍脑袋的表填
        let mut f = sched(
            "week_template:\n  timeLayoutId: nope\n  entries:\n    - { day: Mon, period: 1, course: 数学, teacherId: t1 }\n",
        );
        f.normalize(&[tt(), tt_late()]);
        let e = &f.week_template.entries[0];
        assert!(
            e.start.is_empty(),
            "绑定不存在时不该用别的表补时间，实际补成了 {}",
            e.start
        );
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
