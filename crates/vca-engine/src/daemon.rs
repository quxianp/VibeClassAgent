//! 调度守护进程：系统的「大脑」。
//!
//! 一轮循环做五件事：
//! 1. **录制联动**：到点开始录、到点停止，录制期间按间隔抓截图；
//! 2. **悬浮窗**：下课后 5 分钟显示、上课前 2 分钟关闭；
//! 3. **处理窗口**：进入午休/放学后/晚自习后窗口时，批量跑课后流水线；
//! 4. **清理**：定期扫描并删除已到期的录像；
//! 5. **崩溃静默退出**：由 `vca_platform::crash` 保证。
//!
//! 时间控制：算出「下一个事件时刻」后精确睡眠（上限 30 秒兜底），
//! 空闲时几乎不占 CPU，也不会错过边界。

use std::path::PathBuf;
use std::time::Duration;

use vca_core::config::{load_schedule_file, load_settings, ScheduleFile, Settings, TimetableFile};
use vca_core::model::{OverlaySettings, Timetable};
use vca_core::paths::Layout;
use vca_core::schedule::{self, LessonInstance, OverlayTiming, OverlayWindow, WeekParity};
use vca_core::store::JobStore;
use vca_core::time::{LocalDate, LocalDateTime};
use vca_core::windows::{self, WindowSpec};
use vca_platform::capture::{CaptureParams, CaptureSession};
use vca_platform::overlay::{self, OverlayStyle};
use vca_platform::probe::{self, LoadLevel};

use crate::pipeline::Pipeline;

/// 守护进程配置。
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    /// profile 名。
    pub profile: String,
    /// 是否启用录制（关闭则只跑悬浮窗与处理）。
    pub enable_recording: bool,
    /// 是否启用悬浮窗。
    pub enable_overlay: bool,
    /// 是否只演练（不真正录制、不真正推送）。
    pub dry_run: bool,
    /// 录制期间截图间隔（秒）。
    pub screenshot_interval_secs: u64,
    /// 单轮最长睡眠（秒）。
    pub max_sleep_secs: u64,
    /// 处理窗口内每个作业之间的间隔（秒），避免弱机连续满载。
    pub job_gap_secs: u64,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            profile: "default".to_string(),
            enable_recording: true,
            enable_overlay: true,
            dry_run: false,
            screenshot_interval_secs: 180,
            max_sleep_secs: 30,
            job_gap_secs: 3,
        }
    }
}

/// 守护进程。
pub struct Daemon {
    /// 目录布局。
    pub layout: Layout,
    /// 配置。
    pub cfg: DaemonConfig,
    /// 主配置。
    pub settings: Settings,
    /// 课程表。
    pub schedule: ScheduleFile,
    /// 时间表。
    pub timetable: Option<Timetable>,
    /// 作业仓库。
    pub store: JobStore,
    /// `python/` 目录。
    pub python_dir: PathBuf,
    /// 已解析的周末课表条目（`inherit` 已在载入时展开为实际条目）。
    pub weekend_entries: Vec<vca_core::model::ClassEntry>,
    /// 周末模板解析时的提示（供日志展示）。
    pub weekend_note: Option<String>,
}

impl Daemon {
    /// 载入配置并构造。
    pub fn load(layout: Layout, cfg: DaemonConfig) -> anyhow::Result<Self> {
        // 主配置：优先 profile 目录，其次全局，最后用默认值
        let pdir = layout.profile_config_dir(&cfg.profile);
        let candidates = [
            pdir.join("settings.yaml"),
            layout.config_root.join("global.yaml"),
        ];
        let mut settings = Settings::default();
        for c in candidates.iter() {
            if c.exists() {
                settings = load_settings(c)?;
                break;
            }
        }
        if settings.profile.is_empty() {
            settings.profile = cfg.profile.clone();
        }
        settings.apply_performance_profile();

        // 课程表
        let sc_path = layout.config_root.join("schedule").join("current.yaml");
        let sc_path = if sc_path.exists() {
            sc_path
        } else {
            PathBuf::from("config/schedule.example.yaml")
        };
        let schedule = load_schedule_file(&sc_path)?;

        // 时间表
        let tt_path = layout.config_root.join("timetable").join("current.yaml");
        let timetable = if tt_path.exists() {
            let tf: TimetableFile = serde_yaml::from_str(&std::fs::read_to_string(&tt_path)?)?;
            tf.timetables.into_iter().find(|t| t.is_active)
        } else {
            None
        };

        let python_dir = std::env::var("VCA_PYTHON_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("python"));

        let store = JobStore::new(layout.profile_data_dir(&cfg.profile));

        // 解析周末课表：若配置为 `inherit`，这里会去存档目录里把历史条目读出来
        let (weekend_entries, weekend_note) =
            schedule.resolve_weekend(&layout.schedule_archive_dir());

        Ok(Self {
            layout,
            cfg,
            settings,
            schedule,
            timetable,
            store,
            python_dir,
            weekend_entries,
            weekend_note,
        })
    }

    /// 今天的课程列表。
    pub fn lessons_today(&self, date: LocalDate) -> Vec<LessonInstance> {
        let plan = self.schedule.plan_for_weekday(date.weekday());
        schedule::plan_for_date(
            date,
            WeekParity::Every,
            &plan,
            &self.schedule.overrides,
            true,
        )
    }

    /// 悬浮窗时段。
    pub fn overlay_windows(&self, lessons: &[LessonInstance]) -> Vec<OverlayWindow> {
        schedule::overlay_windows(lessons, self.settings.overlay.timing())
    }

    /// 处理窗口（优先用配置，未配置则由时间表推导）。
    pub fn processing_windows(&self) -> Vec<vca_core::model::ProcessingWindow> {
        if !self.settings.processing_windows.is_empty() {
            return self.settings.processing_windows.clone();
        }
        match &self.timetable {
            Some(tt) => windows::derive_windows(tt, WindowSpec::default()),
            None => Vec::new(),
        }
    }

    /// 主循环（永不返回，除非出错或被要求退出）。
    pub fn run(&mut self) -> anyhow::Result<()> {
        // 崩溃静默退出
        let crash = vca_platform::crash::CrashHandler::new(
            self.layout.profile_data_dir(&self.cfg.profile).join("logs"),
        );
        crash.install();

        // 会话自检：服务（Session 0）无法采集桌面，提前给出明确告警
        if let Some(w) = vca_platform::session::startup_warning() {
            tracing::warn!("{w}");
        } else {
            tracing::info!(
                "运行会话：{}",
                vca_platform::session::detect_run_mode().label()
            );
        }

        // 优雅退出：Ctrl+C / 关窗时只置标志，让主循环走正常退出路径，
        // 从而保证 CaptureSession::drop 能停掉 ffmpeg（否则会留下孤儿进程与损坏录像）。
        if vca_platform::shutdown::install() {
            tracing::info!("已安装优雅退出处理器（Ctrl+C 将先停录制再退出）");
        }

        tracing::info!("守护进程启动 profile={}", self.cfg.profile);

        // 首次运行检测：没走过引导就先自动修复一遍并提示用户
        if !vca_core::setup::is_initialized(&self.layout) {
            tracing::warn!("检测到这是首次运行（或未完成初始化）。正在自动补齐目录与配置");
            let rep = vca_core::setup::repair(&self.layout, &self.cfg.profile);
            tracing::info!("自动修复结果：{}", rep.summary());
            for n in &rep.needs_user {
                tracing::warn!("还需要你提供：{n}");
            }
            tracing::warn!(
                "建议先运行 `vca setup` 走一遍引导（或双击 vca.exe 选「首次引导向导」）"
            );
            // 标记为已初始化：目录与配置已经补齐，下次不再重复提示。
            // 用户想看引导可以随时 `vca setup` 或 `vca reset-setup`。
            let _ = vca_core::setup::mark_initialized(&self.layout);
        }

        // 配置自检：把问题**在启动时一次说清楚**，而不是等运行中出怪现象
        let cfg_issues = self.settings.validate();
        for i in &cfg_issues {
            tracing::warn!("配置问题: {i}");
        }
        if !cfg_issues.is_empty() {
            tracing::warn!(
                "共 {} 项配置问题，程序会尽量用默认值继续运行",
                cfg_issues.len()
            );
        }

        // 课表 / 时间表自检
        let plan_issues = schedule::validate(self.timetable.as_ref(), &self.schedule.week_template);
        for i in &plan_issues {
            tracing::warn!("课表问题: {i}");
        }

        // 处理窗口自检（未配置时由时间表推导，推导结果也要校验）
        let pws = self.processing_windows();
        for i in windows::validate_windows(&pws) {
            tracing::warn!("处理窗口问题: {i}");
        }
        if pws.is_empty() {
            tracing::warn!(
                "没有可用的处理窗口：请检查时间表里是否有 >= 15 分钟的 break 段，\n\
                 或在 settings.yaml 里显式配置 processing_windows。\n\
                 没有处理窗口时，作业不会被自动处理后处理。"
            );
        } else {
            let names: Vec<String> = pws
                .iter()
                .map(|w| format!("{} {}", w.name, w.start))
                .collect();
            tracing::info!("处理窗口 {} 个：{}", pws.len(), names.join(" / "));
        }

        tracing::info!("课程表 {} 条", self.schedule.week_template.entries.len());
        if let Some(n) = &self.weekend_note {
            tracing::info!("周末课表：{n}");
        }
        tracing::info!("周末条目 {} 条", self.weekend_entries.len());
        match self.settings.term.first_monday.as_deref() {
            Some(d) => tracing::info!(
                "学期对齐：第 1 周周一 = {}（首周为{}周）",
                d,
                if self.settings.term.first_week_parity == "even" {
                    "双"
                } else {
                    "单"
                }
            ),
            None => tracing::warn!(
                "未配置 term.first_monday，单双周将按自然周序号推断，可能与学校口径不一致。\n\
                 如需精确，请在 settings.yaml 里补：\n\
                 term:\n  first_monday: \"2026-09-01\"\n  first_week_parity: odd"
            ),
        }
        if let Some(tt) = &self.timetable {
            tracing::info!("时间表「{}」{} 段", tt.name, tt.slots.len());
        }

        // 悬浮窗
        let mut window: Option<overlay::Overlay> = None;
        if self.cfg.enable_overlay && !self.cfg.dry_run {
            match overlay::spawn(style_from(&self.settings.overlay)) {
                Ok((w, h)) => {
                    std::mem::forget(h);
                    tracing::info!("悬浮窗已创建 0x{:X}", w.raw());
                    window = Some(w);
                }
                Err(e) => tracing::warn!("悬浮窗创建失败（继续运行）: {e}"),
            }
        }

        // 托盘图标：一个小图标 + 右键菜单，无气泡无弹窗
        let mut tray: Option<vca_platform::tray::Tray> = None;
        if self.settings.ui.tray_icon && !self.cfg.dry_run {
            match vca_platform::tray::spawn("VibeClassAgent  课堂录制") {
                Ok((t, h)) => {
                    std::mem::forget(h); // 托盘线程随进程结束
                    tracing::info!("托盘图标已创建（右键可停止录制 / 打开目录 / 退出）");
                    tray = Some(t);
                }
                Err(e) => tracing::warn!("托盘图标创建失败（继续运行）: {e}"),
            }
        }

        let mut recorder: Option<CaptureSession> = None;
        let mut current_job: Option<String> = None;
        let mut last_date: Option<LocalDate> = None;
        let mut overlay_visible = false;
        let mut last_shot = std::time::Instant::now();
        let mut shot_seq: u32 = 0;
        let mut last_cleanup = std::time::Instant::now() - Duration::from_secs(3600);
        let mut recording_engine = String::new();
        let mut handled_windows: Vec<String> = Vec::new();

        loop {
            // 收到 Ctrl+C / 关窗 -> 跳出循环，交由 Drop 收尾
            if vca_platform::shutdown::is_shutdown_requested() {
                tracing::info!("收到退出请求，正在优雅收尾");
                break;
            }

            // ---- 托盘命令 ----
            if let Some(t) = &tray {
                while let Some(cmd) = t.poll() {
                    match cmd {
                        vca_platform::tray::TrayCommand::StopRecording => {
                            tracing::info!("托盘：请求立即停止录制");
                            if let Some(mut s) = recorder.take() {
                                let _ = s.stop();
                                current_job = None;
                            }
                        }
                        vca_platform::tray::TrayCommand::OpenLogs => {
                            let dir = self.layout.profile_data_dir(&self.cfg.profile).join("logs");
                            let _ = std::fs::create_dir_all(&dir);
                            tracing::info!("托盘：打开日志目录 {}", dir.display());
                            open_in_explorer(&dir);
                        }
                        vca_platform::tray::TrayCommand::OpenDataDir => {
                            let dir = self.layout.profile_data_dir(&self.cfg.profile);
                            let _ = std::fs::create_dir_all(&dir);
                            tracing::info!("托盘：打开数据目录 {}", dir.display());
                            open_in_explorer(&dir);
                        }
                        vca_platform::tray::TrayCommand::Quit => {
                            tracing::info!("托盘：请求退出");
                            vca_platform::shutdown::request_shutdown();
                        }
                    }
                }
            }

            let now = vca_platform::clock::now_local();
            let today = now.date;
            let now_min = now.minute_of_day();

            if last_date != Some(today) {
                last_date = Some(today);
                handled_windows.clear();
                shot_seq = 0;
                let ls = self.lessons_today(today);
                tracing::info!(
                    "[{}] 载入 {} 节课，{} 个悬浮窗时段，{} 个处理窗口",
                    today,
                    ls.len(),
                    self.overlay_windows(&ls).len(),
                    self.processing_windows().len()
                );
            }

            let lessons = self.lessons_today(today);

            // ---------- 1) 录制联动 ----------
            if self.cfg.enable_recording && !self.cfg.dry_run {
                let active = lessons.iter().find(|l| {
                    l.record
                        && now_min >= l.start.minute_of_day()
                        && now_min < l.end.minute_of_day()
                });

                match (active, recorder.as_mut()) {
                    (Some(lesson), None) => {
                        // 开始录制。注意：这里**不能用 ?**一次磁盘抖动不该让
                        // 整个守护进程退出。失败只记录，下一轮会自然重试。
                        let job = match self.store.ensure_for_lesson(lesson, &self.cfg.profile) {
                            Ok(j) => j,
                            Err(e) => {
                                tracing::warn!("创建作业失败（将在下一轮重试）: {e}");
                                continue;
                            }
                        };
                        let raw_dir = self
                            .layout
                            .raw_dir(&self.cfg.profile, &today.to_string().replace('-', ""));
                        let _ = std::fs::create_dir_all(&raw_dir);
                        let out = raw_dir.join(format!(
                            "{}_{}-{}_{}.mp4",
                            today.to_string().replace('-', ""),
                            lesson
                                .start
                                .to_string()
                                .split('T')
                                .nth(1)
                                .unwrap_or("0000")
                                .replace(':', ""),
                            lesson
                                .end
                                .to_string()
                                .split('T')
                                .nth(1)
                                .unwrap_or("0000")
                                .replace(':', ""),
                            lesson.course
                        ));

                        let mut params = CaptureParams {
                            fps: self.settings.record.fps,
                            height: self.settings.record.height,
                            crf: self.settings.record.crf,
                            monitor: self.settings.record.monitor,
                            audio_source: self.settings.record.audio_source.clone(),
                            screenshot_interval_secs: self.settings.record.screenshot_interval_secs
                                as u64,
                            ..Default::default()
                        };
                        // 自适应降档
                        if let Ok(snap) = probe::sample_load() {
                            params.degrade(LoadLevel::from_snapshot(&snap));
                        }

                        // 截图由 ffmpeg 的第二路输出产出（与视频共用同一次采集，
                        // 不再另开一路屏幕抓取，省一次拷贝）。
                        let shot_dir = self.layout.screenshots_dir(
                            &self.cfg.profile,
                            &today.to_string().replace('-', ""),
                        );
                        match CaptureSession::new(params, &out).map(|s| s.with_shot_dir(&shot_dir))
                        {
                            Ok(mut sess) => {
                                let engine_label = sess.engine_label();
                                match sess.start() {
                                    Ok(()) => {
                                        tracing::info!(
                                            "开始录制「{}」-> {}（引擎：{}）",
                                            lesson.course,
                                            out.display(),
                                            engine_label
                                        );
                                        recording_engine = engine_label;
                                        let mut j = job;
                                        j.state = vca_core::job::JobState::Recorded;
                                        j.video_path = Some(out.to_string_lossy().to_string());
                                        let _ = self.store.save(&j);
                                        current_job = Some(j.id.clone());
                                        recorder = Some(sess);
                                        last_shot =
                                            std::time::Instant::now() - Duration::from_secs(3600);
                                    }
                                    Err(e) => {
                                        // 录制启动失败不崩溃，只记录；下轮会再试
                                        tracing::warn!("录制启动失败（将重试）: {e}");
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!("录制会话初始化失败（将重试）: {e}");
                            }
                        }
                    }
                    (None, Some(_)) => {
                        // 停止录制
                        if let Some(mut s) = recorder.take() {
                            let _ = s.stop();
                            tracing::info!("录制结束（引擎：{recording_engine}）");

                            // 收尾：把视频与 WASAPI 采到的音频合并成最终 mp4。
                            // 中间产物不会被删除，收尾失败可重跑，不必重录一节课。
                            let (shots, notes) = match s.finalize() {
                                Ok(o) => {
                                    for n in &o.notes {
                                        tracing::info!("录制备注：{n}");
                                    }
                                    if let Some(v) = &o.video {
                                        tracing::info!("收尾产物：{}", v.display());
                                    }
                                    (
                                        o.screenshots
                                            .iter()
                                            .map(|p| p.to_string_lossy().to_string())
                                            .collect::<Vec<_>>(),
                                        o.notes,
                                    )
                                }
                                Err(e) => {
                                    tracing::warn!("收尾合并失败（素材保留，可重跑）: {e}");
                                    (Vec::new(), Vec::new())
                                }
                            };
                            let _ = notes;

                            if let Some(id) = current_job.take() {
                                match self.store.load(&id) {
                                    Ok(mut j) => {
                                        j.end = now.to_string();
                                        if !shots.is_empty() {
                                            j.screenshots.extend(shots.clone());
                                        }
                                        if let Err(e) = self.store.save(&j) {
                                            tracing::error!("保存结束时刻失败: {e}");
                                        }
                                    }
                                    Err(e) => tracing::warn!("读取作业失败: {e}"),
                                }
                            }
                            recording_engine.clear();
                        }
                    }
                    _ => {}
                }

                // 录制期间抓截图
                // 仅当使用 ffmpeg 引擎时，才由 daemon 用 GDI 抓图；
                // Python 引擎的录制器自己会抓（复用已解码帧，更省资源）。
                let daemon_should_shoot = recording_engine.starts_with("ffmpeg");
                if daemon_should_shoot
                    && recorder.as_mut().map(|r| r.is_running()).unwrap_or(false)
                    && last_shot.elapsed().as_secs() >= self.cfg.screenshot_interval_secs
                {
                    last_shot = std::time::Instant::now();
                    shot_seq += 1;
                    let dir = self
                        .layout
                        .screenshots_dir(&self.cfg.profile, &today.to_string().replace('-', ""));
                    let hhmm = format!("{:02}{:02}", now.hour, now.minute);
                    match vca_platform::screenshot::capture_timed(
                        &dir,
                        &today.to_string(),
                        &format!("{}:{}", &hhmm[..2], &hhmm[2..]),
                        shot_seq,
                    ) {
                        Ok(p) => {
                            tracing::debug!("截图 {}", p.display());
                            if let Some(id) = &current_job {
                                match self.store.load(id) {
                                    Ok(mut j) => {
                                        j.screenshots.push(p.to_string_lossy().to_string());
                                        if let Err(e) = self.store.save(&j) {
                                            tracing::warn!("保存截图列表失败: {e}");
                                        }
                                    }
                                    Err(e) => tracing::warn!("读取作业失败: {e}"),
                                }
                            }
                        }
                        Err(e) => tracing::warn!("截图失败: {e}"),
                    }
                }
            }

            // ---------- 2) 悬浮窗 ----------
            if self.cfg.enable_overlay {
                let wins = self.overlay_windows(&lessons);
                let should = schedule::overlay_visible_at(&wins, now);
                if should != overlay_visible {
                    overlay_visible = should;
                    if let Some(w) = &window {
                        if should {
                            w.show();
                        } else {
                            w.hide();
                        }
                    }
                    tracing::info!("悬浮窗 {}", if should { "显示" } else { "隐藏" });
                }
            }

            // ---------- 3) 处理窗口 ----------
            let pws = self.processing_windows();
            if let Some(w) = windows::window_at(&pws, now_min) {
                let key = format!("{today}-{}-{}", w.name, w.scope);
                if !handled_windows.contains(&key) {
                    handled_windows.push(key);
                    tracing::info!("进入处理窗口「{}」（scope={}）", w.name, w.scope);
                    // 把正在录制的那个作业排除掉：它这会儿文件还没写完
                    let _ = self.process_scope(&w.scope, &lessons, today, current_job.as_deref());
                }
            }

            // ---------- 4) 清理 ----------
            if last_cleanup.elapsed() >= Duration::from_secs(1800) {
                last_cleanup = std::time::Instant::now();
                let data = self.layout.profile_data_dir(&self.cfg.profile);
                let rep = vca_core::cleanup::sweep(&data, now, self.cfg.dry_run);
                if rep.scanned > 0 {
                    tracing::info!(
                        "清理：扫描 {}，删除 {}，跳过 {}，失败 {}",
                        rep.scanned,
                        rep.deleted.len(),
                        rep.skipped.len(),
                        rep.failed.len()
                    );
                }
            }

            // ---------- 睡眠到下一个事件（分片睡眠以便及时响应退出）----------
            let total = self.next_sleep_secs(&lessons, &pws, now);
            let mut slept = 0u64;
            while slept < total && !vca_platform::shutdown::is_shutdown_requested() {
                let step = (total - slept).min(1);
                std::thread::sleep(Duration::from_secs(step));
                slept += step;
            }
        }

        // ---------- 优雅收尾 ----------
        if let Some(mut s) = recorder.take() {
            tracing::info!("正在停止录制并封装文件");
            match s.stop() {
                Ok(()) => tracing::info!("录制已安全停止"),
                Err(e) => tracing::error!("停止录制出错: {e}"),
            }
        }
        if let Some(w) = &window {
            w.hide();
            w.close();
        }
        if let Some(t) = &tray {
            t.close();
        }
        tracing::info!("守护进程已退出");
        Ok(())
    }

    /// 处理某个 scope 下的所有待办作业。
    ///
    /// `recording_job` 是**此刻正在录制**的作业 id，必须排除掉。
    ///
    /// 为什么：录制一开始就把作业标成 `Recorded` 并存盘，于是它立刻出现在
    /// `pending()` 里。如果处理窗口恰好与上课时段重叠（用户把午休时段填错、
    /// 或者时间表里没有像样的休息段），这个**还没写完的录像**就会被捞去处理，
    /// 结果是「转写失败：没有可转写的音视频文件」，作业被无谓地标成 Failed。
    fn process_scope(
        &mut self,
        scope: &str,
        lessons: &[LessonInstance],
        today: LocalDate,
        recording_job: Option<&str>,
    ) -> anyhow::Result<()> {
        let targets: Vec<String> = lessons
            .iter()
            .filter(|l| {
                l.record
                    && l.date == today
                    && windows::scope_for_lesson(l.start.minute_of_day()) == scope
            })
            .map(|l| vca_core::store::job_id_for(l, &self.cfg.profile))
            .collect();

        let pending = self.store.pending();
        let todo: Vec<_> = pending
            .into_iter()
            .filter(|j| targets.is_empty() || targets.contains(&j.id))
            .filter(|j| {
                if Some(j.id.as_str()) == recording_job {
                    tracing::warn!("[{}] 正在录制中，本轮跳过（等它录完再处理）", j.id);
                    false
                } else {
                    true
                }
            })
            .collect();

        if todo.is_empty() {
            return Ok(());
        }
        tracing::info!("待处理作业 {} 个", todo.len());

        let mut pipe = Pipeline::new(
            &self.layout,
            &self.cfg.profile,
            &self.settings,
            &self.store,
            &self.python_dir,
        );
        pipe.dry_run = self.cfg.dry_run;

        for mut job in todo {
            let out = pipe.process(&mut job);
            for s in &out.steps {
                tracing::info!("[{}] {s}", job.id);
            }
            for w in &out.warnings {
                tracing::warn!("[{}] {w}", job.id);
            }
            if let Some(e) = &out.error {
                tracing::error!("[{}] {e}", job.id);
            }
            std::thread::sleep(Duration::from_secs(self.cfg.job_gap_secs));
        }
        Ok(())
    }

    /// 距离下一个需要行动的时刻还有多少秒。
    fn next_sleep_secs(
        &self,
        lessons: &[LessonInstance],
        pws: &[vca_core::model::ProcessingWindow],
        now: LocalDateTime,
    ) -> u64 {
        let mut earliest: Option<i64> = None;
        let mut consider = |t: LocalDateTime| {
            let d = t.diff_minutes(now);
            if d >= 0 {
                earliest = Some(match earliest {
                    Some(e) => e.min(d),
                    None => d,
                });
            }
        };

        for l in lessons {
            consider(l.start);
            consider(l.end);
        }
        for w in self.overlay_windows(lessons) {
            consider(w.show_at);
            consider(w.hide_at);
        }
        for w in pws {
            if let (Some(s), Some(e)) = (
                LocalDateTime::parse_hhmm(&w.start),
                LocalDateTime::parse_hhmm(&w.end),
            ) {
                consider(LocalDateTime::from_minutes(now.date, s as i64));
                consider(LocalDateTime::from_minutes(now.date, e as i64));
            }
        }
        // 跨天
        consider(LocalDateTime::new(now.date.add_days(1), 0, 0));

        let secs = earliest.unwrap_or(60).max(1) as u64 * 60;
        secs.clamp(1, self.cfg.max_sleep_secs)
    }
}

/// 由配置构造窗口样式。
pub fn style_from(s: &OverlaySettings) -> OverlayStyle {
    let bg = vca_core::model::parse_hex_color(&s.bg_color).unwrap_or((0x1F, 0x6F, 0xEB));
    let fg = vca_core::model::parse_hex_color(&s.text_color).unwrap_or((0xFF, 0xFF, 0xFF));
    OverlayStyle {
        text: s.text.clone(),
        width: s.width,
        height: s.height,
        margin_top: s.margin_top,
        margin_right: s.margin_right,
        bg_rgb: (bg.0 as u32) | ((bg.1 as u32) << 8) | ((bg.2 as u32) << 16),
        text_rgb: (fg.0 as u32) | ((fg.1 as u32) << 8) | ((fg.2 as u32) << 16),
        font_size: s.font_size,
        corner_radius: 14,
        click_through: false,
        font_face: s.font_face.clone(),
        liquid_glass: s.liquid_glass,
        glass_alpha: s.glass_alpha,
        use_system_accent: s.use_system_accent,
    }
}

/// 供外部（CLI）使用的默认时序。
pub fn default_timing() -> OverlayTiming {
    OverlayTiming::default()
}

/// 用资源管理器打开一个目录（托盘菜单用）。
///
/// 找不到资源管理器时静默忽略打不开目录不该影响录制。
fn open_in_explorer(dir: &std::path::Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("explorer")
            .arg(dir)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_from_settings_maps_colors() {
        let s = OverlaySettings {
            bg_color: "#112233".into(),
            text_color: "#FFFFFF".into(),
            text: "T".into(),
            font_face: "Consolas".into(),
            ..Default::default()
        };
        let st = style_from(&s);
        assert_eq!(st.text, "T");
        assert_eq!(st.font_face, "Consolas");
        // #112233 -> COLORREF 0x00332211
        assert_eq!(st.bg_rgb, 0x0033_2211);
        assert_eq!(st.text_rgb, 0x00FF_FFFF);
    }

    #[test]
    fn default_cfg_is_conservative() {
        let c = DaemonConfig::default();
        assert!(c.enable_recording);
        assert!(c.enable_overlay);
        assert!(!c.dry_run);
        assert!(c.max_sleep_secs <= 60);
    }

    #[test]
    fn default_timing_matches_requirement() {
        let t = default_timing();
        assert_eq!(t.show_delay_minutes, 5);
        assert_eq!(t.hide_before_minutes, 2);
    }
}
