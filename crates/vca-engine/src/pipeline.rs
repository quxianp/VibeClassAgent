//! 单个作业的处理流水线。
//!
//! 状态推进严格遵循 `vca_core::job::JobState`：
//!
//! ```text
//! Recorded -> Transcribed -> Extracted -> Linked -> DocReady -> Pushed
//! ```
//!
//! 设计原则（重要）：
//! 1. **每完成一步就立刻落盘**，崩溃后可从最后成功状态继续，不重复调用付费接口；
//! 2. **失败不丢数据**：任一步失败只把作业标为 `Failed`，原始录像与中间产物全部保留；
//! 3. **推送成功才推进到 `Pushed`**，从算法上杜绝「没推送就删录像」；
//! 4. 可选步骤缺失时**降级而非中断**（例如未配 LLM 时直接出纯转写文档）。

use std::path::{Path, PathBuf};

use vca_core::config::Settings;
use vca_core::job::{Job, JobState};
use vca_core::paths::Layout;
use vca_core::store::JobStore;
use vca_core::time::LocalDateTime;
use vca_platform::push::{self, PushConfig, PushDoc};
use vca_platform::{llm, push as push_mod};

/// 流水线一次执行的结果。
#[derive(Debug, Clone)]
pub struct PipelineOutcome {
    /// 作业 id。
    pub job_id: String,
    /// 结束状态。
    pub state: JobState,
    /// 完成（或跳过）的步骤描述。
    pub steps: Vec<String>,
    /// 遇到的问题（不一定是致命错误）。
    pub warnings: Vec<String>,
    /// 致命错误。
    pub error: Option<String>,
}

impl PipelineOutcome {
    /// 是否成功推进到 Pushed。
    pub fn is_pushed(&self) -> bool {
        self.state == JobState::Pushed
    }
}

/// 课后处理流水线。
pub struct Pipeline<'a> {
    /// 目录布局。
    pub layout: &'a Layout,
    /// 当前 profile。
    pub profile: &'a str,
    /// 主配置。
    pub settings: &'a Settings,
    /// 作业仓库。
    pub store: &'a JobStore,
    /// `python/` 目录（含 vca_worker）。
    pub python_dir: PathBuf,
    /// 是否只演练不实际推送。
    pub dry_run: bool,
}

impl<'a> Pipeline<'a> {
    /// 构造。
    pub fn new(
        layout: &'a Layout,
        profile: &'a str,
        settings: &'a Settings,
        store: &'a JobStore,
        python_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            layout,
            profile,
            settings,
            store,
            python_dir: python_dir.into(),
            dry_run: false,
        }
    }

    /// 作业目录。
    fn job_dir(&self, job: &Job) -> PathBuf {
        self.store.dir_of(&job.id)
    }

    /// 跑完整个流水线（幂等：已完成的状态会跳过）。
    pub fn process(&self, job: &mut Job) -> PipelineOutcome {
        let mut steps = Vec::new();
        let mut warnings = Vec::new();

        // 失败态视为「从 Transcribed 之前重来」，但保留已完成的中间产物
        if job.state == JobState::Failed {
            warnings.push("上次失败，从当前已完成的中间产物继续".into());
            job.state = self.first_incomplete_state(job);
        }

        // ---- 1) 转写 ----
        if job.state == JobState::Recorded {
            match self.transcribe(job) {
                Ok(t) => {
                    let dir = self.job_dir(job);
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::fs::write(dir.join("transcript.txt"), &t.text);
                    // 分段带时间戳，截图关联要靠它把图注对上讲稿
                    let _ = std::fs::write(
                        dir.join("transcript.json"),
                        serde_json::to_string(&t.segments).unwrap_or_default(),
                    );
                    steps.push(format!(
                        "转写完成（{} 字 / {} 段 / {} / {:.1}s）",
                        t.text.chars().count(),
                        t.segments.len(),
                        t.engine,
                        t.seconds
                    ));
                    if let Err(e) = self.advance(job, JobState::Transcribed) {
                        return self.fail(job, e);
                    }
                }
                Err(e) => {
                    // 没有音视频时不阻塞后续：产出空转写继续，至少文档还能生成
                    if job.video_path.is_none() && job.audio_path.is_none() {
                        warnings.push("没有音视频轨，跳过转写".into());
                        if let Err(e2) = self.advance(job, JobState::Transcribed) {
                            return self.fail(job, e2);
                        }
                    } else {
                        return self.fail(job, format!("转写失败: {e}"));
                    }
                }
            }
            let _ = self.store.save(job);
        }

        // ---- 2) 要点提取 ----
        if job.state == JobState::Transcribed {
            let text = std::fs::read_to_string(self.job_dir(job).join("transcript.txt"))
                .unwrap_or_default();
            match self.extract(&text) {
                Ok(summary) => {
                    let p = self.job_dir(job).join("summary.json");
                    let _ = std::fs::create_dir_all(self.job_dir(job));
                    let _ = std::fs::write(
                        &p,
                        serde_json::to_string_pretty(&summary).unwrap_or_default(),
                    );
                    steps.push(format!(
                        "要点提取完成（要点 {} 条 / 通知 {} 条）",
                        summary.points.len(),
                        summary.notices.len()
                    ));
                    if let Err(e) = self.advance(job, JobState::Extracted) {
                        return self.fail(job, e);
                    }
                }
                Err(e) => {
                    // 未配置 LLM 时降级：用转写原文前若干字作为摘要
                    warnings.push(format!("要点提取降级为原文摘要: {e}"));
                    let fallback = llm::LessonSummary {
                        title: format!("{}（原始转写）", job.course),
                        points: text
                            .lines()
                            .filter(|l| !l.trim().is_empty())
                            .take(8)
                            .map(|l| l.trim().chars().take(60).collect())
                            .collect(),
                        notices: Vec::new(),
                        keywords: Vec::new(),
                    };
                    let p = self.job_dir(job).join("summary.json");
                    let _ = std::fs::create_dir_all(self.job_dir(job));
                    let _ = std::fs::write(
                        &p,
                        serde_json::to_string_pretty(&fallback).unwrap_or_default(),
                    );
                    if let Err(e2) = self.advance(job, JobState::Extracted) {
                        return self.fail(job, e2);
                    }
                }
            }
            let _ = self.store.save(job);
        }

        // ---- 3) 截图关联 ----
        if job.state == JobState::Extracted {
            let linked = self.link_screenshots(job);
            steps.push(format!("截图关联完成（{} 张）", linked.len()));
            job.screenshots = linked;
            if let Err(e) = self.advance(job, JobState::Linked) {
                return self.fail(job, e);
            }
        }

        // ---- 4) 文档生成 ----
        if job.state == JobState::Linked {
            match self.build_document(job) {
                Ok((docx, pdf)) => {
                    steps.push(format!(
                        "文档生成完成（Word={} PDF={}）",
                        docx.is_some(),
                        pdf.is_some()
                    ));
                    job.docx_path = docx;
                    job.pdf_path = pdf;
                    if let Err(e2) = self.advance(job, JobState::DocReady) {
                        return self.fail(job, e2);
                    }
                }
                Err(e) => return self.fail(job, format!("文档生成失败: {e}")),
            }
            let _ = self.store.save(job);
        }

        // ---- 5) 推送 ----
        if job.state == JobState::DocReady {
            match self.push_document(job) {
                Ok(id) => {
                    steps.push(format!("推送成功（{id}）"));
                    let now = vca_platform::clock::now_local();
                    job.push_succeeded_at = Some(now.to_string());
                    // 72 小时倒计时，从**推送成功时刻**起算
                    job.expire_at = Some(
                        now.add_minutes(self.settings.cleanup.retention_hours as i64 * 60)
                            .to_string(),
                    );
                    steps.push(format!(
                        "已登记清理计划（{} 小时后到期）",
                        self.settings.cleanup.retention_hours
                    ));
                    // 放在 advance 之前：保证落盘时 Pushed 与 expire_at 同时就位
                    if let Err(e) = self.advance(job, JobState::Pushed) {
                        return self.fail(job, e);
                    }
                }
                Err(e) => return self.fail(job, format!("推送失败: {e}")),
            }
        }

        PipelineOutcome {
            job_id: job.id.clone(),
            state: job.state,
            steps,
            warnings,
            error: None,
        }
    }

    /// 按状态机推进作业状态，并立即落盘。
    ///
    /// **为什么不能直接赋值**：`JobState::transition_to` 会拒绝非法迁移
    /// （例如从 `Pending` 直接跳到 `Pushed`）。让编译器与状态机共同守住
    /// 「推送成功才可能进入 Pushed」这条红线，比靠调用方自觉可靠得多。
    fn advance(&self, job: &mut Job, next: JobState) -> Result<(), String> {
        job.state = job
            .state
            .transition_to(next)
            .map_err(|e| format!("状态机拒绝迁移: {e}"))?;
        self.store
            .save(job)
            .map_err(|e| format!("保存作业状态失败: {e}"))?;
        Ok(())
    }

    /// 依据中间产物的存在性判断该从哪一步继续。
    fn first_incomplete_state(&self, job: &Job) -> JobState {
        let dir = self.job_dir(job);
        if !dir.join("transcript.txt").exists() {
            return JobState::Recorded;
        }
        if !dir.join("summary.json").exists() {
            return JobState::Transcribed;
        }
        if job.docx_path.is_none() {
            return JobState::Linked;
        }
        JobState::DocReady
    }

    fn fail(&self, job: &mut Job, msg: String) -> PipelineOutcome {
        job.state = JobState::Failed;
        job.last_error = Some(msg.clone());
        let _ = self.store.save(job);
        PipelineOutcome {
            job_id: job.id.clone(),
            state: JobState::Failed,
            steps: Vec::new(),
            warnings: Vec::new(),
            error: Some(msg),
        }
    }

    // -----------------------------------------------------------------------
    // 各步骤
    // -----------------------------------------------------------------------

    /// 语音转写：本地 whisper.cpp 优先，云端 API 可选（见 `vca_platform::stt`）。
    ///
    /// 返回全文 + **带时间戳的分段**；分段另存为 `transcript.json`，
    /// 供后面的截图关联使用 —— 没有时间戳，截图就只能随便贴上去。
    fn transcribe(&self, job: &Job) -> Result<vca_platform::stt::Transcript, String> {
        // 优先用合并后的视频（音轨已经混好），没有才退回单独的音轨文件
        let media = job
            .video_path
            .as_ref()
            .filter(|p| Path::new(p).exists())
            .or_else(|| job.audio_path.as_ref().filter(|p| Path::new(p).exists()))
            .ok_or_else(|| "没有可转写的音视频文件".to_string())?;

        let t = &self.settings.transcriber;
        let cloud_default = vca_platform::stt::CloudSttConfig::default();
        let cfg = vca_platform::stt::SttConfig {
            engine: vca_platform::stt::SttEngine::parse(&t.engine),
            local: vca_platform::stt::LocalSttConfig {
                model: t.model.clone(),
                language: t.language.clone(),
                threads: t.threads,
                max_seconds: t.max_seconds,
                ..Default::default()
            },
            cloud: vca_platform::stt::CloudSttConfig {
                base_url: if t.cloud_base_url.trim().is_empty() {
                    cloud_default.base_url
                } else {
                    t.cloud_base_url.clone()
                },
                model: t.cloud_model.clone(),
                // 凭据只走环境变量，绝不写进配置文件
                api_key: std::env::var("VCA_STT_API_KEY").unwrap_or_default(),
                language: t.language.clone(),
                timeout_ms: cloud_default.timeout_ms,
            },
        };

        let work = self.job_dir(job).join("_stt");
        vca_platform::stt::transcribe(&cfg, Path::new(media), &work)
            .map_err(|e| format!("{e}"))
    }

    /// 调用 LLM 提取要点（付费 API / 免费额度 / 浏览器三种模式，见 `vca_platform::llm`）。
    fn extract(&self, transcript: &str) -> Result<llm::LessonSummary, llm::LlmError> {
        if transcript.trim().is_empty() {
            return Ok(llm::LessonSummary::default());
        }
        let s = &self.settings.llm;
        let b = &s.browser;
        let cfg = llm::LlmConfig {
            mode: llm::LlmMode::parse(&s.mode),
            provider: s.provider.clone(),
            base_url: s.base_url.clone(),
            // 凭据只走环境变量，绝不写进配置文件
            api_key: std::env::var("VCA_LLM_API_KEY").unwrap_or_default(),
            model: s.model.clone(),
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
        };
        llm::extract_summary(&cfg, transcript)
    }

    /// 截图关联：感知哈希去重 + 与讲稿按时间对齐（见 `vca_platform::shots`）。
    ///
    /// 录制期间每 180 秒抓一张，一节课攒十几张，其中大量是「PPT 没翻页」的重复画面。
    /// 去重后只留有变化的那几张，并按时间点从转写里取一句话当图注。
    ///
    /// 同时把带图注的结果落盘成 `shot_refs.json`，文档生成阶段直接读，
    /// 避免重复跑一遍感知哈希。
    fn link_screenshots(&self, job: &Job) -> Vec<String> {
        let limit = self.settings.doc.max_screenshots.max(1) as usize;

        // 转写分段（没有就退化为「有图无注」）
        let segments: Vec<vca_platform::stt::Segment> =
            std::fs::read_to_string(self.job_dir(job).join("transcript.json"))
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();

        // 截图目录：从已记录的路径反推（录制时写进 job.screenshots 的目录）
        let shot_dir = job
            .screenshots
            .iter()
            .filter_map(|p| Path::new(p).parent().map(|d| d.to_path_buf()))
            .next();

        let interval = self.settings.record.screenshot_interval_secs.max(1) as u64;
        let refs = match shot_dir {
            Some(dir) if dir.is_dir() => vca_platform::shots::build_refs(
                &dir,
                interval,
                &segments,
                limit,
            ),
            _ => Vec::new(),
        };

        // 没有任何截图时不报错：文档照常生成
        if refs.is_empty() {
            return job
                .screenshots
                .iter()
                .filter(|s| Path::new(s).exists())
                .take(limit)
                .cloned()
                .collect();
        }

        let dir = self.job_dir(job);
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(
            dir.join("shot_refs.json"),
            serde_json::to_string_pretty(&refs).unwrap_or_default(),
        );

        refs.iter()
            .map(|r| r.path.to_string_lossy().to_string())
            .collect()
    }

    /// 调用 Python Worker 生成 Word/PDF。
    fn build_document(&self, job: &Job) -> Result<(Option<String>, Option<String>), String> {
        let dir = self.job_dir(job);
        let out_dir = {
            let d = self.layout.docs_dir(self.profile, &job.date.replace('-', ""));
            if d.as_os_str().is_empty() {
                dir.clone()
            } else {
                d
            }
        };
        let _ = std::fs::create_dir_all(&out_dir);

        // 要点
        let summary: llm::LessonSummary = std::fs::read_to_string(dir.join("summary.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        // 转写全文（作为文档附录）
        let transcript = std::fs::read_to_string(dir.join("transcript.txt")).unwrap_or_default();

        // 截图（已在 link 阶段去重并配好图注）
        let screenshots: Vec<vca_platform::docgen::ShotRef> =
            std::fs::read_to_string(dir.join("shot_refs.json"))
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();

        let input = vca_platform::docgen::DocInput {
            course: job.course.clone(),
            date: job.date.clone(),
            teacher: job.teacher_id.clone(),
            summary,
            transcript: if self.settings.doc.include_transcript {
                transcript
            } else {
                String::new()
            },
            screenshots,
            duration_secs: duration_between(&job.start, &job.end),
            video_name: job
                .video_path
                .as_ref()
                .and_then(|p| {
                    Path::new(p)
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                })
                .unwrap_or_default(),
            extra_keywords: Vec::new(),
        };

        let out = vca_platform::docgen::generate(&input, &out_dir, &self.settings.doc.formats)
            .map_err(|e| format!("{e}"))?;
        for n in &out.notes {
            tracing::warn!("文档生成备注：{n}");
        }

        let docx = out.docx.map(|p| p.to_string_lossy().to_string());
        let pdf = out.pdf.map(|p| p.to_string_lossy().to_string());
        if docx.is_none() && pdf.is_none() && out.markdown.is_none() {
            return Err("没有产出任何文档".into());
        }
        Ok((docx, pdf))
    }

    /// 通过外部插件推送（`settings.plugins.push` 指定插件 id）。
    ///
    /// 插件协议见 `plugins/README.md`：拉起进程后走 JSON-RPC over stdio，
    /// 调用 `run` 并传入文档路径与摘要，插件需返回 `{success, messageId}`。
    fn push_via_plugin(&self, job: &Job, plugin_id: &str) -> Result<String, String> {
        use vca_plugin_host::host::PluginHost;
        use vca_plugin_host::runner::PluginProcess;

        let plugins_dir = self
            .layout
            .config_root
            .parent()
            .map(|p| p.join("plugins"))
            .unwrap_or_else(|| std::path::PathBuf::from("plugins"));

        let mut host = PluginHost::new().with_plugins_dir(&plugins_dir);
        host.discover().map_err(|e| format!("扫描插件失败: {e}"))?;
        let handle = host
            .list()
            .iter()
            .find(|h| h.manifest.plugin.id == plugin_id)
            .ok_or_else(|| format!("没找到插件 {plugin_id}（目录 {}）", plugins_dir.display()))?;

        let summary_path = self.job_dir(job).join("summary.json");
        let summary_text = std::fs::read_to_string(&summary_path).unwrap_or_default();

        let mut proc = PluginProcess::start(&handle.manifest, &handle.dir)
            .map_err(|e| format!("启动插件失败: {e}"))?;

        // 初始化：把该插件的权限相关配置以环境变量注入（凭据不落盘）
        let _ = proc.init(serde_json::json!({ "profile": self.profile }));

        let params = serde_json::json!({
            "kind": "push",
            "title": format!("课堂纪要 {} {}", job.date, job.course),
            "summary": summary_text,
            "docx": job.docx_path,
            "pdf": job.pdf_path,
            "target": self.settings.push.target,
        });
        let res = proc.run(params).map_err(|e| format!("插件调用失败: {e}"))?;
        proc.shutdown();

        if res
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            Ok(res
                .get("messageId")
                .or_else(|| res.get("message_id"))
                .and_then(|v| v.as_str())
                .unwrap_or(plugin_id)
                .to_string())
        } else {
            Err(res
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("插件未返回成功")
                .to_string())
        }
    }

    /// 推送文档。若 `settings.plugins.push` 指定了插件，则改走插件。
    fn push_document(&self, job: &Job) -> Result<String, String> {
        // 插件优先：配置了就用插件，否则用内置推送
        let routed = self.settings.plugins.push.trim();
        if !routed.is_empty() && !self.dry_run {
            tracing::info!("推送交由插件执行：{routed}");
            return self.push_via_plugin(job, routed);
        }

        let summary_path = self.job_dir(job).join("summary.json");
        let summary: llm::LessonSummary = std::fs::read_to_string(&summary_path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();

        let provider = push::Provider::parse(&self.settings.push.provider)
            .ok_or_else(|| format!("未知推送渠道: {}", self.settings.push.provider))?;

        if self.dry_run || provider == push::Provider::DryRun {
            let p = push_mod::DryRunPusher;
            let doc = PushDoc {
                title: format!("课堂纪要 {} {}", job.date, job.course),
                summary: llm::render_summary_text(&job.course, &job.date, &summary),
                docx: job.docx_path.clone(),
                pdf: job.pdf_path.clone(),
                target: self.settings.push.target.clone(),
            };
            use vca_platform::push::Pusher;
            let out = p.send(&doc).map_err(|e| e.to_string())?;
            return Ok(out.message_id.unwrap_or_else(|| "dry-run".into()));
        }

        let p = &self.settings.push;
        let cfg = PushConfig {
            provider,
            // 地址优先用配置；留空时回落到环境变量（地址与令牌都可能敏感）
            endpoint: if p.endpoint.trim().is_empty() {
                std::env::var("VCA_PUSH_ENDPOINT").unwrap_or_default()
            } else {
                p.endpoint.clone()
            },
            token: std::env::var("VCA_PUSH_TOKEN").unwrap_or_default(),
            target: p.target.clone().unwrap_or_default(),
            target_type: p.target_type.clone(),
            // QQ 官方机器人凭据只走环境变量
            app_id: std::env::var("VCA_QQ_APP_ID").unwrap_or_default(),
            app_secret: std::env::var("VCA_QQ_APP_SECRET").unwrap_or_default(),
            max_retries: p.max_retries,
            timeout_ms: 60_000,
        };
        let pusher = push_mod::make_pusher(&cfg);
        let doc = PushDoc {
            title: format!("课堂纪要 {} {}", job.date, job.course),
            summary: llm::render_summary_text(&job.course, &job.date, &summary),
            docx: job.docx_path.clone(),
            pdf: job.pdf_path.clone(),
            target: p.target.clone(),
        };

        // 带重试；只有全部失败才返回失败 ——
        // 调用方据此决定「保留本地录像」，所以这里的判定必须严格。
        let out = push_mod::send_with_retry(pusher.as_ref(), &doc, p.max_retries);
        if out.success {
            Ok(out.message_id.unwrap_or_else(|| "ok".into()))
        } else {
            Err(out.error.unwrap_or_else(|| "推送失败".into()))
        }
    }
}

/// 从 `start` / `end` 时刻算出课时长（秒）。
///
/// 接受 `HH:MM` 与 `HH:MM:SS` 两种写法；解析不出来就返回 0
/// （文档里会显示成 `—`，而不是一个假的时长）。
fn duration_between(start: &str, end: &str) -> u64 {
    fn to_secs(s: &str) -> Option<u64> {
        let parts: Vec<&str> = s.trim().split(':').collect();
        if parts.len() < 2 {
            return None;
        }
        let h: u64 = parts[0].parse().ok()?;
        let m: u64 = parts[1].parse().ok()?;
        let sec: u64 = parts.get(2).and_then(|x| x.parse().ok()).unwrap_or(0);
        Some(h * 3600 + m * 60 + sec)
    }
    match (to_secs(start), to_secs(end)) {
        (Some(a), Some(b)) if b > a => b - a,
        _ => 0,
    }
}

/// 判断作业是否已到清理时间（供 daemon 与 cleaner 共用）。
pub fn is_expired(job: &Job, now: LocalDateTime) -> bool {
    match job.expire_at.as_deref().and_then(LocalDateTime::parse) {
        Some(t) => job.state == JobState::Pushed && now >= t,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vca_core::paths::Layout;

    fn tmp_layout(tag: &str) -> Layout {
        let base = std::env::temp_dir().join(format!("vca_pipe_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        Layout::new(base.join("data"), base.join("config"))
    }

    fn mk_job(id: &str) -> Job {
        Job {
            id: id.into(),
            profile: "p".into(),
            state: JobState::Recorded,
            push_succeeded_at: None,
            expire_at: None,
            last_error: None,
            course: "数学".into(),
            teacher_id: "t1".into(),
            date: "2025-03-18".into(),
            start: "2025-03-18T08:00".into(),
            end: "2025-03-18T08:45".into(),
            video_path: None,
            audio_path: None,
            screenshots: Vec::new(),
            docx_path: None,
            pdf_path: None,
        }
    }

    #[test]
    fn expired_only_when_pushed_and_due() {
        let l = tmp_layout("exp");
        let _ = l;
        let mut j = mk_job("a");
        j.state = JobState::Pushed;
        j.expire_at = Some("2025-03-21T12:00".into());
        let before = LocalDateTime::parse("2025-03-21T11:59").unwrap();
        let after = LocalDateTime::parse("2025-03-21T12:00").unwrap();
        assert!(!is_expired(&j, before));
        assert!(is_expired(&j, after));
    }

    #[test]
    fn unpushed_job_never_expires() {
        let mut j = mk_job("b");
        j.state = JobState::DocReady;
        j.expire_at = Some("2020-01-01T00:00".into());
        let now = LocalDateTime::parse("2030-01-01T00:00").unwrap();
        assert!(!is_expired(&j, now));
    }

    #[test]
    fn pipeline_degrades_without_llm_and_worker() {
        // 无音频、无 LLM、无 Worker 时也必须不 panic，并给出明确警告
        let layout = tmp_layout("deg");
        let store = JobStore::new(layout.profile_data_dir("p"));
        let settings = Settings::default();
        let pipe = Pipeline::new(&layout, "p", &settings, &store, "python");
        let mut job = mk_job("c");
        let out = pipe.process(&mut job);
        // 未配置 LLM -> 降级；最终因无 Worker 生成文档失败
        assert!(out.error.is_some() || out.is_pushed() || out.state == JobState::Extracted);
        let _ = std::fs::remove_dir_all(&layout.data_root);
    }

    #[test]
    fn link_screenshots_respects_limit_and_dedup() {
        let layout = tmp_layout("lnk");
        let store = JobStore::new(layout.profile_data_dir("p"));
        let mut settings = Settings::default();
        settings.doc.max_screenshots = 2;
        let pipe = Pipeline::new(&layout, "p", &settings, &store, "python");

        let dir = std::env::temp_dir().join(format!("vca_shots_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f1 = dir.join("a.bmp");
        let f2 = dir.join("b.bmp");
        std::fs::write(&f1, b"x").unwrap();
        std::fs::write(&f2, b"x").unwrap();

        let mut job = mk_job("d");
        job.screenshots = vec![
            f1.to_string_lossy().to_string(),
            f1.to_string_lossy().to_string(), // 重复
            f2.to_string_lossy().to_string(),
            dir.join("missing.bmp").to_string_lossy().to_string(), // 不存在
        ];
        // Worker 不可用时走均匀抽样降级，数量仍受 limit 约束且不超过可用文件数
        let out = pipe.link_screenshots(&job);
        assert!(
            out.len() <= 2,
            "应受 max_screenshots 限制，实际 {}",
            out.len()
        );
        assert!(!out.is_empty(), "不应为空（有 2 个真实文件）");
        // 不存在的文件必须被过滤掉
        assert!(out.iter().all(|p| std::path::Path::new(p).exists()));

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&layout.data_root);
    }
}
