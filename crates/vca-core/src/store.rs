//! 作业持久化：把 [`Job`] 以 JSON 落盘到 `jobs/<id>/job.json`。
//!
//! 为什么要落盘：转写/文档生成可能耗时数十分钟，进程崩溃或断电后
//! 必须能从最后一个成功状态继续，避免重复调用付费 API。

use std::path::{Path, PathBuf};

use crate::error::{CoreError, Result};
use crate::job::{Job, JobState};
use crate::schedule::LessonInstance;

/// 作业仓库（对应某个 profile 的数据目录）。
#[derive(Debug, Clone)]
pub struct JobStore {
    root: PathBuf,
}

impl JobStore {
    /// 以 `data/profiles/<profile>/jobs` 为根构造。
    pub fn new(profile_data_dir: impl Into<PathBuf>) -> Self {
        Self {
            root: profile_data_dir.into().join("jobs"),
        }
    }

    /// 仓库根目录。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 作业目录。
    pub fn dir_of(&self, job_id: &str) -> PathBuf {
        self.root.join(job_id)
    }

    /// `job.json` 路径。
    pub fn path_of(&self, job_id: &str) -> PathBuf {
        self.dir_of(job_id).join("job.json")
    }

    /// 保存（原子写：先写临时文件再重命名）。
    pub fn save(&self, job: &Job) -> Result<PathBuf> {
        let dir = self.dir_of(&job.id);
        std::fs::create_dir_all(&dir)
            .map_err(|e| CoreError::Path(format!("创建作业目录失败 {}: {e}", dir.display())))?;
        let path = dir.join("job.json");
        let tmp = dir.join("job.json.tmp");
        let text = serde_json::to_string_pretty(job)
            .map_err(|e| CoreError::Config(format!("序列化作业失败: {e}")))?;
        std::fs::write(&tmp, text)
            .map_err(|e| CoreError::Path(format!("写入失败 {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &path)
            .map_err(|e| CoreError::Path(format!("重命名失败 {}: {e}", path.display())))?;
        Ok(path)
    }

    /// 读取。
    pub fn load(&self, job_id: &str) -> Result<Job> {
        let path = self.path_of(job_id);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| CoreError::Path(format!("读取作业失败 {}: {e}", path.display())))?;
        serde_json::from_str(&text)
            .map_err(|e| CoreError::Config(format!("解析作业失败 {}: {e}", path.display())))
    }

    /// 列出全部作业（按 id 排序）。
    pub fn list(&self) -> Vec<Job> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(&self.root) else {
            return out;
        };
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let jf = p.join("job.json");
            if let Ok(text) = std::fs::read_to_string(&jf) {
                if let Ok(j) = serde_json::from_str::<Job>(&text) {
                    out.push(j);
                }
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// 列出处于指定状态的作业。
    pub fn list_by_state(&self, state: JobState) -> Vec<Job> {
        self.list()
            .into_iter()
            .filter(|j| j.state == state)
            .collect()
    }

    /// 列出「待处理」的作业：已录制但尚未推送成功。
    pub fn pending(&self) -> Vec<Job> {
        self.list()
            .into_iter()
            .filter(|j| {
                matches!(
                    j.state,
                    JobState::Recorded
                        | JobState::Transcribed
                        | JobState::Extracted
                        | JobState::Linked
                        | JobState::DocReady
                        | JobState::Failed
                )
            })
            .collect()
    }

    /// 由一节课创建作业（幂等：已存在则直接返回）。
    pub fn ensure_for_lesson(&self, lesson: &LessonInstance, profile: &str) -> Result<Job> {
        let id = job_id_for(lesson, profile);
        let path = self.path_of(&id);
        if path.exists() {
            return self.load(&id);
        }
        let job = Job {
            id: id.clone(),
            profile: profile.to_string(),
            state: JobState::Pending,
            push_succeeded_at: None,
            expire_at: None,
            last_error: None,
            course: lesson.course.clone(),
            teacher_id: lesson.teacher_id.clone(),
            date: lesson.date.to_string(),
            start: lesson.start.to_string(),
            end: lesson.end.to_string(),
            video_path: None,
            audio_path: None,
            screenshots: Vec::new(),
            docx_path: None,
            pdf_path: None,
        };
        self.save(&job)?;
        Ok(job)
    }
}

/// 生成稳定的作业 id：`<日期>-<开始>-<课程>`（同一节课重复调用结果一致）。
pub fn job_id_for(lesson: &LessonInstance, profile: &str) -> String {
    let course: String = lesson
        .course
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    let start = lesson.start.to_string().replace(':', "");
    format!("{}_{}_{}_{}", lesson.date, start, course, profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::{LocalDate, LocalDateTime};

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vca_store_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn lesson(course: &str, h: u32, m: u32) -> LessonInstance {
        let d = LocalDate::new(2025, 3, 18);
        LessonInstance {
            date: d,
            period: Some(1),
            start: LocalDateTime::new(d, h, m),
            end: LocalDateTime::new(d, h, m + 45),
            course: course.to_string(),
            teacher_id: "t1".to_string(),
            record: true,
            room: None,
            merge_group: None,
        }
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = tmp_dir("rt");
        let store = JobStore::new(&dir);
        let l = lesson("数学", 8, 0);
        let job = store.ensure_for_lesson(&l, "p1").unwrap();
        assert_eq!(job.state, JobState::Pending);

        let loaded = store.load(&job.id).unwrap();
        assert_eq!(loaded.id, job.id);
        assert_eq!(loaded.course, "数学");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_is_idempotent() {
        let dir = tmp_dir("idem");
        let store = JobStore::new(&dir);
        let l = lesson("数学", 8, 0);
        let a = store.ensure_for_lesson(&l, "p1").unwrap();
        let b = store.ensure_for_lesson(&l, "p1").unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(store.list().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn different_profile_different_job() {
        let dir = tmp_dir("prof");
        let store = JobStore::new(&dir);
        let l = lesson("数学", 8, 0);
        let a = store.ensure_for_lesson(&l, "p1").unwrap();
        let b = store.ensure_for_lesson(&l, "p2").unwrap();
        assert_ne!(a.id, b.id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_by_state_and_pending() {
        let dir = tmp_dir("state");
        let store = JobStore::new(&dir);
        let mut j = store.ensure_for_lesson(&lesson("A", 8, 0), "p").unwrap();
        j.state = JobState::Pushed;
        store.save(&j).unwrap();
        let mut k = store.ensure_for_lesson(&lesson("B", 9, 0), "p").unwrap();
        k.state = JobState::Recorded;
        store.save(&k).unwrap();

        assert_eq!(store.list_by_state(JobState::Pushed).len(), 1);
        assert_eq!(store.pending().len(), 1);
        assert_eq!(store.list().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn job_id_is_stable_and_safe() {
        let l = lesson("数学/提高", 8, 0);
        let a = job_id_for(&l, "p1");
        let b = job_id_for(&l, "p1");
        assert_eq!(a, b);
        assert!(!a.contains('/'));
        assert!(a.contains("08"));
    }
}
