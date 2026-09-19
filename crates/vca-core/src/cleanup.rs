//! 清理器：按「推送成功后 72 小时」策略删除录像。
//!
//! 设计约束（策划书 3.2.4）：
//! - **推送未成功绝不删除**；
//! - 到期后仍需二次校验 `pushed == true` 才物理删除；
//! - 支持 dry-run 预览，便于人工确认。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::time::LocalDateTime;

/// 删除计划：与录像同目录的 `<jobId>.deleteplan.json`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeletePlan {
    /// 作业 id。
    pub job_id: String,
    /// 待删除的目标路径。
    pub target: String,
    /// 是否已推送成功（**必须为 true 才允许删除**）。
    pub pushed: bool,
    /// 推送成功时刻。
    pub pushed_at: Option<LocalDateTime>,
    /// 到期时刻 = 推送成功时刻 + 72h。
    pub expire_at: Option<LocalDateTime>,
}

impl DeletePlan {
    /// 依据推送成功时刻生成计划。
    pub fn new(
        job_id: impl Into<String>,
        target: impl Into<String>,
        pushed_at: LocalDateTime,
        retention_hours: u32,
    ) -> Self {
        Self {
            job_id: job_id.into(),
            target: target.into(),
            pushed: true,
            pushed_at: Some(pushed_at),
            expire_at: Some(pushed_at.add_minutes(retention_hours as i64 * 60)),
        }
    }

    /// 是否已到期。
    pub fn is_expired(&self, now: LocalDateTime) -> bool {
        match self.expire_at {
            Some(e) => now >= e && self.pushed,
            None => false,
        }
    }

    /// 剩余小时数（负数表示已过期）。
    pub fn hours_left(&self, now: LocalDateTime) -> Option<i64> {
        self.expire_at.map(|e| e.diff_minutes(now) / 60)
    }

    /// 计划文件路径（与 target 同目录）。
    pub fn path_for(target: &Path) -> PathBuf {
        let stem = target
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "job".to_string());
        target.with_file_name(format!("{stem}.deleteplan.json"))
    }

    /// 写入磁盘。
    pub fn save(&self, dir: &Path) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(dir)?;
        let p = dir.join(format!("{}.deleteplan.json", self.job_id));
        let text =
            serde_json::to_string_pretty(self).map_err(|e| std::io::Error::other(e.to_string()))?;
        std::fs::write(&p, text)?;
        Ok(p)
    }
}

/// 一次清扫的结果。
#[derive(Debug, Default, Clone)]
pub struct SweepReport {
    /// 扫描到的计划数。
    pub scanned: usize,
    /// 已删除的文件。
    pub deleted: Vec<String>,
    /// 跳过（未到期或未推送）的文件。
    pub skipped: Vec<String>,
    /// 删除失败的文件。
    pub failed: Vec<(String, String)>,
}

/// 扫描目录树，删除所有已到期的录像。
///
/// 只处理「计划文件存在、`pushed == true`、且已到期」的目标；
/// 其余一律跳过。`dry_run` 为真时只报告不删除。
pub fn sweep(root: &Path, now: LocalDateTime, dry_run: bool) -> SweepReport {
    let mut rep = SweepReport::default();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = path.file_name().map(|s| s.to_string_lossy().to_string());
            if !name.as_deref().unwrap_or("").ends_with(".deleteplan.json") {
                continue;
            }
            rep.scanned += 1;

            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    rep.failed.push((path.display().to_string(), e.to_string()));
                    continue;
                }
            };
            let plan: DeletePlan = match serde_json::from_str(&text) {
                Ok(p) => p,
                Err(e) => {
                    rep.failed.push((path.display().to_string(), e.to_string()));
                    continue;
                }
            };

            if !plan.is_expired(now) {
                rep.skipped.push(plan.target.clone());
                continue;
            }

            if dry_run {
                rep.deleted.push(format!("{}（dry-run）", plan.target));
                continue;
            }

            let target = PathBuf::from(&plan.target);
            match std::fs::remove_file(&target) {
                Ok(()) => {
                    rep.deleted.push(plan.target.clone());
                    let _ = std::fs::remove_file(&path);
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    rep.deleted.push(plan.target.clone());
                    let _ = std::fs::remove_file(&path);
                }
                Err(e) => rep.failed.push((plan.target.clone(), e.to_string())),
            }
        }
    }
    rep
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::LocalDate;

    fn t(h: u32, m: u32) -> LocalDateTime {
        LocalDateTime::new(LocalDate::new(2025, 3, 18), h, m)
    }

    #[test]
    fn plan_expires_exactly_after_72h() {
        let p = DeletePlan::new("job1", "a.mp4", t(12, 0), 72);
        assert_eq!(
            p.expire_at.unwrap(),
            LocalDateTime::new(LocalDate::new(2025, 3, 21), 12, 0)
        );
        assert!(!p.is_expired(LocalDateTime::new(LocalDate::new(2025, 3, 21), 11, 59)));
        assert!(p.is_expired(LocalDateTime::new(LocalDate::new(2025, 3, 21), 12, 0)));
    }

    #[test]
    fn unpushed_plan_never_expires() {
        let mut p = DeletePlan::new("job1", "a.mp4", t(12, 0), 72);
        p.pushed = false;
        assert!(!p.is_expired(LocalDateTime::new(LocalDate::new(2030, 1, 1), 0, 0)));
    }

    #[test]
    fn hours_left_is_signed() {
        let p = DeletePlan::new("job1", "a.mp4", t(12, 0), 72);
        assert_eq!(p.hours_left(t(12, 0)), Some(72));
        assert_eq!(
            p.hours_left(LocalDateTime::new(LocalDate::new(2025, 3, 21), 12, 0)),
            Some(0)
        );
        assert!(
            p.hours_left(LocalDateTime::new(LocalDate::new(2025, 3, 22), 12, 0))
                .unwrap()
                < 0
        );
    }

    #[test]
    fn sweep_deletes_only_expired() {
        let dir = std::env::temp_dir().join(format!("vca_clean_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 两个录像：一个已到期，一个未到期
        let old = dir.join("old.mp4");
        let new = dir.join("new.mp4");
        std::fs::write(&old, b"x").unwrap();
        std::fs::write(&new, b"x").unwrap();

        DeletePlan::new("old", old.display().to_string(), t(0, 0), 72)
            .save(&dir)
            .unwrap();
        DeletePlan::new("new", new.display().to_string(), t(23, 59), 72)
            .save(&dir)
            .unwrap();

        // 时间线：
        //   old：推送于 03-18 00:00 -> 到期 03-21 00:00
        //   new：推送于 03-18 23:59 -> 到期 03-21 23:59
        // 取「现在」= 03-21 12:00：old 已到期（删），new 未到期（留）。
        let now = LocalDateTime::new(LocalDate::new(2025, 3, 21), 12, 0);
        let rep = sweep(&dir, now, false);
        assert_eq!(rep.scanned, 2);
        assert!(rep.deleted.iter().any(|s| s.contains("old.mp4")));
        assert!(rep.skipped.iter().any(|s| s.contains("new.mp4")));
        assert!(!old.exists(), "到期文件应被删除");
        assert!(new.exists(), "未到期文件必须保留");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dry_run_does_not_delete() {
        let dir = std::env::temp_dir().join(format!("vca_clean_dry_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.mp4");
        std::fs::write(&f, b"x").unwrap();
        DeletePlan::new("a", f.display().to_string(), t(0, 0), 72)
            .save(&dir)
            .unwrap();

        let now = LocalDateTime::new(LocalDate::new(2025, 3, 30), 0, 0);
        let rep = sweep(&dir, now, true);
        assert_eq!(rep.deleted.len(), 1);
        assert!(f.exists(), "dry-run 不应真的删除");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
