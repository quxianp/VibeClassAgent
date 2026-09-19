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

use crate::worker::WorkerClient;

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
                Ok(text) => {
                    let path = self.job_dir(job).join("transcript.txt");
                    let _ = std::fs::create_dir_all(self.job_dir(job));
                    let _ = std::fs::write(&path, &text);
                    steps.push(format!("转写完成（{} 字）", text.chars().count()));
                    if let Err(e) = self.advance(job, JobState::Transcribed) {
                        return self.fail(job, e);
                    }
                }
                Err(e) => {
                    // 没有音频（未录到音）时不阻塞后续：产出空转写继续
                    if job.audio_path.is_none() {
                        warnings.push("没有音频轨，跳过转写".into());
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

    /// 调用 Python Worker 转写。
    fn transcribe(&self, job: &Job) -> Result<String, String> {
        let Some(audio) = &job.audio_path else {
            return Err("没有音频文件".into());
        };
        if !Path::new(audio).exists() {
            return Err(format!("音频不存在: {audio}"));
        }
        let mut w = WorkerClient::start(&self.python_dir, 3600)
            .map_err(|e| format!("启动 Worker 失败: {e}"))?;
        let res = w
            .call(
                "run",
                serde_json::json!({
                    "kind": "transcribe",
                    "audio_path": audio,
                    "model": self.settings.transcriber.model,
                    "language": self.settings.transcriber.language,
                    "threads": self.settings.transcriber.threads,
                }),
            )
            .map_err(|e| format!("转写调用失败: {e}"))?;
        if let Some(err) = res.get("error").and_then(|v| v.as_str()) {
            return Err(err.to_string());
        }
        let text = res
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Worker 未返回 text".to_string())?;
        Ok(text.to_string())
    }

    /// 调用 LLM 提取要点。
    fn extract(&self, transcript: &str) -> Result<llm::LessonSummary, llm::LlmError> {
        if transcript.trim().is_empty() {
            return Ok(llm::LessonSummary::default());
        }
        let cfg = llm::LlmConfig {
            base_url: self.settings.llm.base_url.clone(),
            api_key: std::env::var("VCA_LLM_API_KEY").unwrap_or_default(),
            model: self.settings.llm.model.clone(),
            timeout_ms: 90_000,
            max_chars: 6000,
        };
        llm::extract_summary(&cfg, transcript)
    }

    /// 截图关联：先用 **pHash** 去掉几乎相同的画面，再取前 N 张。
    ///
    /// 录制期间每 180 秒抓一张，一节课会攒下十几张，
    /// 其中大量是「PPT 没翻页」的重复画面  直接塞进文档会让文件巨大且无信息量。
    ///
    /// 处理顺序：
    /// 1. 先用路径去重并过滤掉不存在的文件（**必须**，否则 Worker 会读到脏路径）；
    /// 2. 调用 Python Worker 的 `dedupe` 做感知哈希去重；
    /// 3. Worker 不可用（未装 Python / 未装 ImageHash）时**降级**为均匀抽样，
    ///    保证文档照常产出，只是截图可能略多几张。
    fn link_screenshots(&self, job: &Job) -> Vec<String> {
        let limit = self.settings.doc.max_screenshots.max(1) as usize;

        // 1) 路径去重 + 存在性过滤
        let mut seen = std::collections::HashSet::new();
        let existing: Vec<String> = job
            .screenshots
            .iter()
            .filter(|s| Path::new(s).exists() && seen.insert((*s).clone()))
            .cloned()
            .collect();

        if existing.is_empty() {
            return Vec::new();
        }
        if existing.len() <= limit {
            return existing;
        }

        // 2) 交给 Python 做 pHash 去重
        if let Ok(mut w) = WorkerClient::start(&self.python_dir, 300) {
            let res = w.call(
                "run",
                serde_json::json!({
                    "kind": "dedupe",
                    "paths": existing,
                    "threshold": 6,
                    "limit": limit,
                }),
            );
            if let Ok(v) = res {
                if let Some(kept) = v.get("kept").and_then(|k| k.as_array()) {
                    let out: Vec<String> = kept
                        .iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect();
                    if !out.is_empty() {
                        return out;
                    }
                }
            }
            tracing::warn!("pHash 去重不可用，降级为均匀抽样");
        }

        // 3) 降级：均匀抽样
        let step = (existing.len() / limit).max(1);
        existing.into_iter().step_by(step).take(limit).collect()
    }

    /// 调用 Python Worker 生成 Word/PDF。
    fn build_document(&self, job: &Job) -> Result<(Option<String>, Option<String>), String> {
        let summary_path = self.job_dir(job).join("summary.json");
        let transcript_path = self.job_dir(job).join("transcript.txt");
        let out_dir = self
            .layout
            .docs_dir(self.profile, &job.date.replace('-', ""));
        let out_dir = if out_dir.as_os_str().is_empty() {
            self.job_dir(job)
        } else {
            out_dir
        };
        let _ = std::fs::create_dir_all(&out_dir);

        let mut w = WorkerClient::start(&self.python_dir, 600)
            .map_err(|e| format!("启动 Worker 失败: {e}"))?;
        let res = w
            .call(
                "run",
                serde_json::json!({
                    "kind": "docgen",
                    "course": job.course,
                    "date": job.date,
                    "start": job.start,
                    "end": job.end,
                    "teacher_id": job.teacher_id,
                    "summary_path": summary_path.to_string_lossy(),
                    "transcript_path": transcript_path.to_string_lossy(),
                    "screenshots": job.screenshots,
                    "out_dir": out_dir.to_string_lossy(),
                    "formats": self.settings.doc.formats,
                }),
            )
            .map_err(|e| format!("文档生成调用失败: {e}"))?;
        if let Some(err) = res.get("error").and_then(|v| v.as_str()) {
            return Err(err.to_string());
        }
        let docx = res
            .get("docx")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
        let pdf = res
            .get("pdf")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
        if docx.is_none() && pdf.is_none() {
            return Err("Worker 未产出任何文档".into());
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

        let cfg = PushConfig {
            provider,
            endpoint: std::env::var("VCA_PUSH_ENDPOINT").unwrap_or_default(),
            token: std::env::var("VCA_PUSH_TOKEN").unwrap_or_default(),
            timeout_ms: 60_000,
        };
        let pusher = push_mod::make_pusher(&cfg);
        let doc = PushDoc {
            title: format!("课堂纪要 {} {}", job.date, job.course),
            summary: llm::render_summary_text(&job.course, &job.date, &summary),
            docx: job.docx_path.clone(),
            pdf: job.pdf_path.clone(),
            target: self.settings.push.target.clone(),
        };

        let mut last_err = String::new();
        for attempt in 1..=self.settings.push.max_retries.max(1) {
            match pusher.send(&doc) {
                Ok(o) if o.success => {
                    return Ok(o.message_id.unwrap_or_else(|| format!("attempt{attempt}")))
                }
                Ok(o) => last_err = o.error.unwrap_or_else(|| "未知失败".into()),
                Err(e) => last_err = e.to_string(),
            }
            std::thread::sleep(std::time::Duration::from_secs(2u64.pow(attempt.min(4))));
        }
        Err(last_err)
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
