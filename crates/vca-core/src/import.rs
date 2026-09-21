//! 课表 / 时间表导入：ClassIsland JSON 与 CSV 两种来源。
//!
//! ClassIsland 关键结构（基于实测数据）：
//! - `TimeLayouts.<guid>.Layouts[]`：`StartTime`/`EndTime`/`TimeType`(0=课,1=休息)/`BreakName`
//! - `ClassPlans.<guid>`：`Name`/`TimeLayoutId`/`TimeRule{WeekDay,WeekCountDiv}`/`Classes[]`
//! - `Subjects.<guid>`：`Name`/`TeacherName`
//!
//! 其中 `Classes` 与 `Layouts` 里 `TimeType==0` 的项**按下标一一对应**。
//!
//! 导入器**全程只读**，绝不修改 ClassIsland 的任何文件。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{ScheduleFile, TimetableFile, WeekendTemplate};
use crate::model::{ClassEntry, ClassPlan, SlotKind, Teacher, Timetable, TimetableSlot};

/// 导入错误。
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// 读取失败。
    #[error("读取 {path} 失败: {source}")]
    Io {
        /// 路径。
        path: String,
        /// 底层错误。
        source: std::io::Error,
    },
    /// JSON 解析失败。
    #[error("解析 ClassIsland JSON 失败: {0}")]
    Json(String),
    /// CSV 解析失败。
    #[error("解析 CSV 失败（第 {line} 行）: {reason}")]
    Csv {
        /// 行号。
        line: usize,
        /// 原因。
        reason: String,
    },
    /// 内容不可用。
    #[error("{0}")]
    Invalid(String),
}

fn read(path: &Path) -> Result<String, ImportError> {
    std::fs::read_to_string(path).map_err(|e| ImportError::Io {
        path: path.display().to_string(),
        source: e,
    })
}

// ---------------------------------------------------------------------------
// 教师名清洗
// ---------------------------------------------------------------------------

/// 清洗教师名：空值或纯占位符（`.`, `。`, `？` 等）视为「未填写」。
pub fn clean_teacher(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    let all_punct = t.chars().all(|c| {
        matches!(
            c,
            '.' | '\u{3002}' | '?' | '\u{FF1F}' | '!' | '\u{FF01}' | '-' | '_' | '~' | '\u{2014}'
        )
    });
    if all_punct {
        return None;
    }
    Some(t.to_string())
}

// ---------------------------------------------------------------------------
// ClassIsland 导入
// ---------------------------------------------------------------------------

/// 导入报告（供 CLI 展示）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportReport {
    /// 来源描述。
    pub source: String,
    /// 使用的时间表名称。
    pub timetable_name: String,
    /// 识别到的课程表份数。
    pub class_plan_count: usize,
    /// 生成的课程条目数。
    pub entry_count: usize,
    /// 教师名待补全的科目。
    pub teachers_missing: Vec<String>,
    /// 警告。
    pub warnings: Vec<String>,
}

/// ClassIsland 导入产物。
#[derive(Debug, Clone)]
pub struct ClassIslandImport {
    /// 时间表。
    pub timetable: Timetable,
    /// 课程表。
    pub schedule: ScheduleFile,
    /// 报告。
    pub report: ImportReport,
}

/// 从 ClassIsland 的 Default.json 导入。
pub fn import_classisland(
    path: &Path,
    timetable_hint: Option<&str>,
) -> Result<ClassIslandImport, ImportError> {
    let text = read(path)?;
    let root: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| ImportError::Json(e.to_string()))?;

    let time_layouts = root
        .get("TimeLayouts")
        .and_then(|v| v.as_object())
        .ok_or_else(|| ImportError::Invalid("缺少 TimeLayouts 节点".into()))?;
    let class_plans = root
        .get("ClassPlans")
        .and_then(|v| v.as_object())
        .ok_or_else(|| ImportError::Invalid("缺少 ClassPlans 节点".into()))?;
    let subjects = root
        .get("Subjects")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    // 选时间表：hint 优先 -> IsActive -> 第一个
    let mut chosen: Option<(&String, &serde_json::Value)> = None;
    if let Some(hint) = timetable_hint {
        for (k, v) in time_layouts {
            let name = v.get("Name").and_then(|n| n.as_str()).unwrap_or("");
            if name.contains(hint) {
                chosen = Some((k, v));
                break;
            }
        }
    }
    if chosen.is_none() {
        for (k, v) in time_layouts {
            if v.get("IsActive").and_then(|b| b.as_bool()).unwrap_or(false) {
                chosen = Some((k, v));
                break;
            }
        }
    }
    if chosen.is_none() {
        chosen = time_layouts.iter().next();
    }
    let (layout_id, layout) =
        chosen.ok_or_else(|| ImportError::Invalid("没有任何可用的时间表".into()))?;

    let tt_name = layout
        .get("Name")
        .and_then(|n| n.as_str())
        .unwrap_or("未命名时间表")
        .to_string();

    let layouts = layout
        .get("Layouts")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ImportError::Invalid("时间表缺少 Layouts 数组".into()))?;

    let mut slots: Vec<TimetableSlot> = Vec::new();
    let mut class_slots: Vec<(String, String, Option<String>)> = Vec::new();
    let mut period_no: u32 = 0;

    for s in layouts {
        let start_hhmm = truncate_hhmm(s.get("StartTime").and_then(|v| v.as_str()).unwrap_or(""));
        let end_hhmm = truncate_hhmm(s.get("EndTime").and_then(|v| v.as_str()).unwrap_or(""));
        if start_hhmm.is_empty() || end_hhmm.is_empty() {
            continue;
        }
        let ty = s.get("TimeType").and_then(|v| v.as_u64()).unwrap_or(0);
        let brk = s.get("BreakName").and_then(|v| v.as_str()).unwrap_or("");

        if ty == 0 {
            period_no += 1;
            let def = s
                .get("DefaultClassId")
                .and_then(|v| v.as_str())
                .filter(|s| !s.starts_with("00000000"))
                .map(|s| s.to_string());
            class_slots.push((start_hhmm.clone(), end_hhmm.clone(), def));
            slots.push(TimetableSlot {
                period: Some(period_no),
                start: start_hhmm,
                end: end_hhmm,
                kind: SlotKind::Class,
                name: None,
            });
        } else {
            slots.push(TimetableSlot {
                period: None,
                start: start_hhmm,
                end: end_hhmm,
                kind: SlotKind::Break,
                name: if brk.trim().is_empty() {
                    None
                } else {
                    Some(brk.to_string())
                },
            });
        }
    }

    if class_slots.is_empty() {
        return Err(ImportError::Invalid("时间表里没有上课节".into()));
    }

    let mut teachers: Vec<Teacher> = Vec::new();
    let mut teacher_id_by_name: HashMap<String, String> = HashMap::new();
    let mut missing: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut all_entries: Vec<ClassEntry> = Vec::new();
    let mut plan_count = 0usize;

    for (_gid, cp) in class_plans {
        if !cp
            .get("IsEnabled")
            .and_then(|b| b.as_bool())
            .unwrap_or(true)
        {
            continue;
        }
        let cp_layout = cp
            .get("TimeLayoutId")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !cp_layout.is_empty() && cp_layout != layout_id.as_str() {
            continue;
        }
        plan_count += 1;

        let weekday = cp
            .get("TimeRule")
            .and_then(|t| t.get("WeekDay"))
            .and_then(|v| v.as_u64())
            .unwrap_or(1) as u32;
        let div = cp
            .get("TimeRule")
            .and_then(|t| t.get("WeekCountDiv"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let cycle = match div {
            1 => "odd",
            2 => "even",
            _ => "every",
        };
        let day = weekday_name(weekday);
        let cells = cp
            .get("Classes")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        for (i, (start, end, def)) in class_slots.iter().enumerate() {
            let Some(cell) = cells.get(i) else { continue };
            if !cell
                .get("IsEnabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true)
            {
                continue;
            }
            let raw_sid = cell
                .get("SubjectId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let sid = if raw_sid.is_empty() || raw_sid.starts_with("00000000") {
                def.clone().unwrap_or_default()
            } else {
                raw_sid
            };
            if sid.is_empty() {
                continue;
            }

            let (course_raw, teacher_raw) = subjects
                .get(&sid)
                .map(|s| {
                    (
                        s.get("Name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        s.get("TeacherName")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    )
                })
                .unwrap_or_default();

            let course = if course_raw.trim().is_empty() {
                "未命名课程".to_string()
            } else {
                course_raw.trim().to_string()
            };

            let teacher_id = match clean_teacher(&teacher_raw) {
                Some(name) => {
                    if let Some(id) = teacher_id_by_name.get(&name) {
                        id.clone()
                    } else {
                        let id = format!("t{}", teacher_id_by_name.len() + 1);
                        teacher_id_by_name.insert(name.clone(), id.clone());
                        teachers.push(Teacher {
                            id: id.clone(),
                            name,
                            profile: id.clone(),
                        });
                        id
                    }
                }
                None => {
                    if !missing.contains(&course) {
                        missing.push(course.clone());
                    }
                    "unassigned".to_string()
                }
            };

            all_entries.push(ClassEntry {
                day: day.to_string(),
                period: Some(i as u32 + 1),
                start: start.clone(),
                end: end.clone(),
                course,
                teacher_id,
                // ClassIsland 的课表里没有「录不录」这个概念，导入时**一律先勾上**。
                // 导入这个动作本身就是「这些课我要录」的意思。
                // 反过来（默认不录）是最糟的默认值：守护进程照常跑、日志一切正常，
                // 却一节课都不会录，用户只会以为程序坏了。
                // 想跳过某几节，在 GUI 课表页把勾去掉即可。
                record: true,
                merge: false,
                room: None,
                cycle: Some(cycle.to_string()),
            });
        }
    }

    if !missing.is_empty() && !teachers.iter().any(|t| t.id == "unassigned") {
        teachers.push(Teacher {
            id: "unassigned".to_string(),
            name: "（待补全）".to_string(),
            profile: "unassigned".to_string(),
        });
    }
    if time_layouts.len() > 1 {
        warnings.push(format!(
            "该文件含 {} 份时间表，已选用「{}」",
            time_layouts.len(),
            tt_name
        ));
    }
    if !missing.is_empty() {
        warnings.push(format!(
            "{} 个科目未填写教师，已归入 unassigned",
            missing.len()
        ));
    }

    let report = ImportReport {
        source: path.display().to_string(),
        timetable_name: tt_name.clone(),
        class_plan_count: plan_count,
        entry_count: all_entries.len(),
        teachers_missing: missing,
        warnings,
    };

    let timetable = Timetable {
        id: "imported".to_string(),
        name: tt_name,
        is_active: true,
        source: "classisland".to_string(),
        slots,
    };

    let weekend: Vec<ClassEntry> = all_entries
        .iter()
        .filter(|e| e.day == "Sat" || e.day == "Sun")
        .cloned()
        .collect();
    let weekday: Vec<ClassEntry> = all_entries
        .iter()
        .filter(|e| e.day != "Sat" && e.day != "Sun")
        .cloned()
        .collect();

    let schedule = ScheduleFile {
        teachers,
        week_template: ClassPlan {
            cycle: "every".to_string(),
            entries: weekday,
        },
        weekend_template: Some(WeekendTemplate {
            source: "new".to_string(),
            inherit_from: None,
            entries: weekend,
        }),
        overrides: Vec::new(),
    };

    Ok(ClassIslandImport {
        timetable,
        schedule,
        report,
    })
}

/// `08:00:00` -> `08:00`
fn truncate_hhmm(t: &str) -> String {
    let t = t.trim();
    let b = t.as_bytes();
    if t.len() >= 5 && b.get(2) == Some(&b':') {
        t[..5].to_string()
    } else if t.len() == 4 && b.get(1) == Some(&b':') {
        format!("0{t}")
    } else {
        String::new()
    }
}

/// 1..7 -> Mon..Sun
fn weekday_name(w: u32) -> &'static str {
    match w {
        1 => "Mon",
        2 => "Tue",
        3 => "Wed",
        4 => "Thu",
        5 => "Fri",
        6 => "Sat",
        7 => "Sun",
        _ => "Mon",
    }
}

// ---------------------------------------------------------------------------
// CSV 导入
// ---------------------------------------------------------------------------

/// 一条 CSV 记录。
#[derive(Debug, Clone)]
pub struct CsvRow {
    /// 星期（Mon..Sun）。
    pub day: String,
    /// 开始时间 HH:mm。
    pub start: String,
    /// 结束时间 HH:mm。
    pub end: String,
    /// 课程。
    pub course: String,
    /// 教师。
    pub teacher: String,
    /// 是否录制。
    pub record: bool,
    /// 节次。
    pub period: Option<u32>,
    /// 教室。
    pub room: Option<String>,
    /// 周次范围/循环。
    pub cycle: Option<String>,
}

/// CSV 导入产物。
#[derive(Debug, Clone)]
pub struct CsvImport {
    /// 课程表。
    pub schedule: ScheduleFile,
    /// 报告。
    pub report: ImportReport,
}

/// 解析 CSV 文本（支持引号与 `""` 转义）。
pub fn parse_csv(text: &str) -> Result<Vec<CsvRow>, ImportError> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header_line = lines
        .next()
        .ok_or_else(|| ImportError::Invalid("CSV 为空".into()))?;
    let header = split_csv_line(header_line);
    let norm = |s: &str| s.trim().trim_start_matches('\u{feff}').to_string();
    let header: Vec<String> = header.iter().map(|h| norm(h)).collect();
    let idx = |names: &[&str]| -> Option<usize> {
        header.iter().position(|h| names.iter().any(|n| h == n))
    };

    let i_day = idx(&["星期", "day", "Day"])
        .ok_or_else(|| ImportError::Invalid("CSV 缺少「星期」列".into()))?;
    let i_start = idx(&["开始时间", "start", "Start"])
        .ok_or_else(|| ImportError::Invalid("CSV 缺少「开始时间」列".into()))?;
    let i_end = idx(&["结束时间", "end", "End"])
        .ok_or_else(|| ImportError::Invalid("CSV 缺少「结束时间」列".into()))?;
    let i_course = idx(&["课程", "course", "Course"])
        .ok_or_else(|| ImportError::Invalid("CSV 缺少「课程」列".into()))?;
    let i_teacher = idx(&["教师", "teacher", "Teacher"])
        .ok_or_else(|| ImportError::Invalid("CSV 缺少「教师」列".into()))?;
    let i_record = idx(&["是否录制", "record", "Record"]);
    let i_period = idx(&["节次", "period", "Period"]);
    let i_room = idx(&["教室", "room", "Room"]);
    let i_cycle = idx(&["周次范围", "周次", "cycle", "Cycle"]);

    let get = |row: &[String], i: Option<usize>| -> String {
        i.and_then(|i| row.get(i)).cloned().unwrap_or_default()
    };

    let mut out = Vec::new();
    for (n, line) in lines.enumerate() {
        let row = split_csv_line(line);
        let lineno = n + 2;
        let start = normalize_hhmm(&get(&row, Some(i_start))).ok_or_else(|| ImportError::Csv {
            line: lineno,
            reason: "开始时间格式错误（应为 HH:mm）".into(),
        })?;
        let end = normalize_hhmm(&get(&row, Some(i_end))).ok_or_else(|| ImportError::Csv {
            line: lineno,
            reason: "结束时间格式错误（应为 HH:mm）".into(),
        })?;
        if end <= start {
            return Err(ImportError::Csv {
                line: lineno,
                reason: "结束时间必须晚于开始时间".into(),
            });
        }
        let day_raw = get(&row, Some(i_day));
        let day = normalize_day(&day_raw).ok_or_else(|| ImportError::Csv {
            line: lineno,
            reason: format!("无法识别的星期：{day_raw}"),
        })?;
        let course = get(&row, Some(i_course)).trim().to_string();
        if course.is_empty() {
            return Err(ImportError::Csv {
                line: lineno,
                reason: "课程为空".into(),
            });
        }
        let rec = get(&row, i_record);
        let room = get(&row, i_room).trim().to_string();
        let cyc = get(&row, i_cycle).trim().to_string();
        out.push(CsvRow {
            day,
            start,
            end,
            course,
            teacher: get(&row, Some(i_teacher)).trim().to_string(),
            record: parse_bool(&rec),
            period: get(&row, i_period).trim().parse().ok(),
            room: if room.is_empty() { None } else { Some(room) },
            cycle: if cyc.is_empty() { None } else { Some(cyc) },
        });
    }
    Ok(out)
}

/// 把 CSV 行转成课程表。
pub fn csv_to_schedule(rows: &[CsvRow], source: &str) -> CsvImport {
    let mut teachers: Vec<Teacher> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    let mut entries: Vec<ClassEntry> = Vec::new();

    for r in rows {
        let teacher_id = match clean_teacher(&r.teacher) {
            Some(name) => {
                if let Some(t) = teachers.iter().find(|t| t.name == name) {
                    t.id.clone()
                } else {
                    let id = format!("t{}", teachers.len() + 1);
                    teachers.push(Teacher {
                        id: id.clone(),
                        name,
                        profile: id.clone(),
                    });
                    id
                }
            }
            None => {
                if !missing.contains(&r.course) {
                    missing.push(r.course.clone());
                }
                "unassigned".to_string()
            }
        };
        entries.push(ClassEntry {
            day: r.day.clone(),
            period: r.period,
            start: r.start.clone(),
            end: r.end.clone(),
            course: r.course.clone(),
            teacher_id,
            record: r.record,
            merge: false,
            room: r.room.clone(),
            cycle: r.cycle.clone(),
        });
    }

    if !missing.is_empty() && !teachers.iter().any(|t| t.id == "unassigned") {
        teachers.push(Teacher {
            id: "unassigned".to_string(),
            name: "（待补全）".to_string(),
            profile: "unassigned".to_string(),
        });
    }

    let weekend: Vec<ClassEntry> = entries
        .iter()
        .filter(|e| e.day == "Sat" || e.day == "Sun")
        .cloned()
        .collect();
    let weekday: Vec<ClassEntry> = entries
        .iter()
        .filter(|e| e.day != "Sat" && e.day != "Sun")
        .cloned()
        .collect();

    let report = ImportReport {
        source: source.to_string(),
        timetable_name: String::new(),
        class_plan_count: 1,
        entry_count: entries.len(),
        teachers_missing: missing,
        warnings: Vec::new(),
    };

    CsvImport {
        schedule: ScheduleFile {
            teachers,
            week_template: ClassPlan {
                cycle: "every".to_string(),
                entries: weekday,
            },
            weekend_template: if weekend.is_empty() {
                None
            } else {
                Some(WeekendTemplate {
                    source: "new".to_string(),
                    inherit_from: None,
                    entries: weekend,
                })
            },
            overrides: Vec::new(),
        },
        report,
    }
}

/// CSV 模板内容（UTF-8 BOM，Excel 可直接打开）。
pub fn csv_template() -> String {
    let mut s = String::from("\u{feff}");
    s.push_str("星期,节次,开始时间,结束时间,课程,教师,是否录制,教室,周次范围,备注\n");
    s.push_str("周一,1,08:00,08:45,课程一,教师甲,是,教室一,,\n");
    s.push_str("周一,2,08:55,09:40,课程一,教师甲,否,教室一,,\n");
    s.push_str("周二,1,08:00,08:45,课程二,教师乙,是,教室一,,\n");
    s.push_str("周六,1,09:00,10:30,课程三,教师甲,是,教室二,,\n");
    s
}

/// 生成时间表模板 YAML。
pub fn timetable_template() -> String {
    String::from(
        "# 时间表：一天的作息骨架\n\
         # type: class = 上课节；type: break = 课间/休息段\n\
         timetables:\n\
         \x20 - id: default\n\
         \x20   name: 本校作息\n\
         \x20   is_active: true\n\
         \x20   source: manual\n\
         \x20   slots:\n\
         \x20     - { period: 1, start: \"08:00\", end: \"08:45\", type: class }\n\
         \x20     - { period: 2, start: \"08:55\", end: \"09:40\", type: class }\n\
         \x20     - { start: \"09:40\", end: \"10:00\", type: break, name: 大课间 }\n\
         \x20     - { period: 3, start: \"10:00\", end: \"10:45\", type: class }\n\
         \x20     - { start: \"12:10\", end: \"13:30\", type: break, name: 午餐午休 }\n\
         \x20     - { period: 4, start: \"14:00\", end: \"14:45\", type: class }\n",
    )
}

fn split_csv_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                if in_quotes && chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    in_quotes = !in_quotes;
                }
            }
            ',' if !in_quotes => {
                out.push(cur.clone());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

fn normalize_hhmm(s: &str) -> Option<String> {
    let t = s.trim();
    let parts: Vec<&str> = t.split(':').collect();
    if parts.len() < 2 {
        return None;
    }
    let h: u32 = parts[0].trim().parse().ok()?;
    let m: u32 = parts[1].trim().parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    Some(format!("{h:02}:{m:02}"))
}

fn normalize_day(s: &str) -> Option<String> {
    let t = s.trim();
    let r = match t {
        "周一" | "星期一" | "Mon" | "mon" | "MON" => "Mon",
        "周二" | "星期二" | "Tue" | "tue" | "TUE" => "Tue",
        "周三" | "星期三" | "Wed" | "wed" | "WED" => "Wed",
        "周四" | "星期四" | "Thu" | "thu" | "THU" => "Thu",
        "周五" | "星期五" | "Fri" | "fri" | "FRI" => "Fri",
        "周六" | "星期六" | "Sat" | "sat" | "SAT" => "Sat",
        "周日" | "星期日" | "星期天" | "周天" | "Sun" | "sun" | "SUN" => "Sun",
        _ => return None,
    };
    Some(r.to_string())
}

fn parse_bool(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "是" | "true" | "1" | "y" | "yes" | "on"
    )
}

// ---------------------------------------------------------------------------
// 存档
// ---------------------------------------------------------------------------

/// 生成存档文件名（按周，不按学期）。
pub fn archive_name(prefix: &str, week_key: &str, version: u32) -> String {
    format!("{prefix}_{week_key}_{version:03}.yaml")
}

/// 写出一份 YAML 存档（旧版本永不覆盖）。
pub fn save_archive(
    dir: &Path,
    prefix: &str,
    week_key: &str,
    value: &impl serde::Serialize,
) -> Result<PathBuf, ImportError> {
    std::fs::create_dir_all(dir).map_err(|e| ImportError::Io {
        path: dir.display().to_string(),
        source: e,
    })?;
    let stem = format!("{prefix}_{week_key}_");
    let mut version = 1u32;
    if let Ok(rd) = std::fs::read_dir(dir) {
        version = rd
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(&stem))
            .count() as u32
            + 1;
    }
    let path = dir.join(archive_name(prefix, week_key, version));
    let text = crate::config::to_yaml(value)
        .map_err(|e| ImportError::Invalid(format!("序列化 YAML 失败: {e}")))?;
    std::fs::write(&path, text).map_err(|e| ImportError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    Ok(path)
}

/// 写入运行时配置：`timetable/current.yaml` 与 `schedule/current.yaml`。
pub fn write_current(
    config_root: &Path,
    timetable: &Timetable,
    schedule: &ScheduleFile,
) -> Result<(PathBuf, PathBuf), ImportError> {
    let tt_dir = config_root.join("timetable");
    let sc_dir = config_root.join("schedule");
    for d in [&tt_dir, &sc_dir] {
        std::fs::create_dir_all(d).map_err(|e| ImportError::Io {
            path: d.display().to_string(),
            source: e,
        })?;
    }

    let tt_path = tt_dir.join("current.yaml");
    let tf = TimetableFile {
        timetables: vec![timetable.clone()],
    };
    std::fs::write(
        &tt_path,
        crate::config::to_yaml(&tf).map_err(|e| ImportError::Invalid(e.to_string()))?,
    )
    .map_err(|e| ImportError::Io {
        path: tt_path.display().to_string(),
        source: e,
    })?;

    let sc_path = sc_dir.join("current.yaml");
    std::fs::write(
        &sc_path,
        crate::config::to_yaml(schedule).map_err(|e| ImportError::Invalid(e.to_string()))?,
    )
    .map_err(|e| ImportError::Io {
        path: sc_path.display().to_string(),
        source: e,
    })?;

    Ok((tt_path, sc_path))
}
