//! 课后文档生成：Markdown / Word(.docx) / PDF。
//!
//! # 三种格式各自的定位
//!
//! | 格式 | 怎么生成 | 为什么要有 |
//! |---|---|---|
//! | Markdown | 直接拼字符串 | 永远生成。纯文本、可 diff、可二次编辑，是另外两种的**源** |
//! | Word | `docx-rs` 写 OOXML | 老师要能改、能交教务处 |
//! | PDF | HTML → **Edge 无头打印** | 发出去不会被改，且**中文渲染零配置** |
//!
//! # PDF 为什么绕道 HTML
//!
//! Rust 直接写 PDF 遇到中文就要嵌字体：`printpdf` 这类库不做字体子集化，
//! 一份文档动辄带 10 MB 字体；而中文排版（断行、标点挤压）本来也不是它们的强项。
//!
//! Windows 上**必定装有 Edge**，它的 `--print-to-pdf` 用的是完整的 Chromium
//! 排版引擎：中文、字体、CSS、图片分页全都免费拿到，产物体积还很小。
//! 代价只是多一次进程调用。
//!
//! # 截图与转写的关联
//!
//! 每张截图带一个时间点（[`ShotRef::at_ms`]），生成时会把**该时间点附近的
//! 转写文本**作为图注。这样文档里是「讲到二次函数顶点公式时的板书」，
//! 而不是六张不知所谓的屏幕照片。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::browser_bot;
use crate::llm::LessonSummary;
use crate::proc;

/// 一张与课堂内容关联的截图。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ShotRef {
    /// 图片路径。
    pub path: PathBuf,
    /// 出现在课堂的第几毫秒。
    pub at_ms: u64,
    /// 图注（通常是该时刻附近的转写文本）。
    pub caption: String,
}

impl ShotRef {
    /// 格式化成 `mm:ss` 或 `h:mm:ss`。
    pub fn timestamp(&self) -> String {
        format_ts(self.at_ms)
    }
}

/// 把毫秒格式化成时间戳。
pub fn format_ts(ms: u64) -> String {
    let total = ms / 1000;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// 文档生成的输入。
#[derive(Debug, Clone, Default)]
pub struct DocInput {
    /// 课程名。
    pub course: String,
    /// 日期（`YYYY-MM-DD`）。
    pub date: String,
    /// 教师（可空）。
    pub teacher: String,
    /// 结构化要点。
    pub summary: LessonSummary,
    /// 转写全文（作为附录；可空）。
    pub transcript: String,
    /// 关联截图。
    pub screenshots: Vec<ShotRef>,
    /// 课堂时长（秒）。
    pub duration_secs: u64,
    /// 录像文件名（写进文档便于溯源）。
    pub video_name: String,
    /// 附加关键词。
    pub extra_keywords: Vec<String>,
}

impl DocInput {
    /// 把时长格式化成「45 分 12 秒」。
    pub fn duration_text(&self) -> String {
        let s = self.duration_secs;
        if s == 0 {
            return "—".to_string();
        }
        let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
        if h > 0 {
            format!("{h} 小时 {m} 分")
        } else if m > 0 {
            format!("{m} 分 {sec} 秒")
        } else {
            format!("{sec} 秒")
        }
    }

    /// 文档标题。
    pub fn title(&self) -> String {
        if !self.summary.title.trim().is_empty() {
            self.summary.title.clone()
        } else if !self.course.trim().is_empty() {
            format!("{} 课堂纪要", self.course)
        } else {
            "课堂纪要".to_string()
        }
    }

    /// 全部关键词（去重）。
    pub fn all_keywords(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for k in self.summary.keywords.iter().chain(self.extra_keywords.iter()) {
            let k = k.trim();
            if !k.is_empty() && !out.iter().any(|x| x == k) {
                out.push(k.to_string());
            }
        }
        out
    }

    /// 安全的基础文件名（不含扩展名）。
    pub fn base_name(&self) -> String {
        let raw = if self.course.trim().is_empty() {
            "课堂纪要".to_string()
        } else {
            format!("{}课堂纪要", self.course.trim())
        };
        let safe: String = raw
            .chars()
            .map(|c| match c {
                '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
                _ => c,
            })
            .collect();
        format!("{}_{}", self.date.trim(), safe.trim())
    }
}

/// 生成结果。
#[derive(Debug, Clone, Default)]
pub struct DocOutput {
    /// Markdown 路径。
    pub markdown: Option<PathBuf>,
    /// Word 路径。
    pub docx: Option<PathBuf>,
    /// PDF 路径。
    pub pdf: Option<PathBuf>,
    /// 备注（哪一步降级了、为什么）。
    pub notes: Vec<String>,
}

impl DocOutput {
    /// 是否至少产出了一份文档。
    pub fn has_any(&self) -> bool {
        self.markdown.is_some() || self.docx.is_some() || self.pdf.is_some()
    }
}

// ---------------------------------------------------------------------------
// Markdown
// ---------------------------------------------------------------------------

/// 渲染 Markdown。
pub fn render_markdown(input: &DocInput) -> String {
    let mut md = String::new();
    md.push_str(&format!("# {}\n\n", input.title()));

    md.push_str("| 项目 | 内容 |\n|---|---|\n");
    if !input.course.trim().is_empty() {
        md.push_str(&format!("| 课程 | {} |\n", input.course));
    }
    md.push_str(&format!("| 日期 | {} |\n", input.date));
    if !input.teacher.trim().is_empty() {
        md.push_str(&format!("| 教师 | {} |\n", input.teacher));
    }
    md.push_str(&format!("| 时长 | {} |\n", input.duration_text()));
    if !input.video_name.trim().is_empty() {
        md.push_str(&format!("| 录像 | {} |\n", input.video_name));
    }
    md.push('\n');

    if !input.summary.points.is_empty() {
        md.push_str("## 课堂重点\n\n");
        for (i, p) in input.summary.points.iter().enumerate() {
            md.push_str(&format!("{}. {}\n", i + 1, p));
        }
        md.push('\n');
    }

    if !input.summary.notices.is_empty() {
        md.push_str("## 重点通知\n\n");
        for n in &input.summary.notices {
            md.push_str(&format!("- **{n}**\n"));
        }
        md.push('\n');
    }

    let kws = input.all_keywords();
    if !kws.is_empty() {
        md.push_str("## 关键词\n\n");
        md.push_str(
            &kws.iter()
                .map(|k| format!("`{k}`"))
                .collect::<Vec<_>>()
                .join(" "),
        );
        md.push_str("\n\n");
    }

    if !input.screenshots.is_empty() {
        md.push_str("## 课堂截图\n\n");
        for s in &input.screenshots {
            let name = s
                .path
                .file_name()
                .map(|x| x.to_string_lossy().to_string())
                .unwrap_or_default();
            md.push_str(&format!("### {}\n\n", s.timestamp()));
            md.push_str(&format!("![{name}]({})\n\n", rel_link(&s.path)));
            if !s.caption.trim().is_empty() {
                md.push_str(&format!("> {}\n\n", s.caption.trim()));
            }
        }
    }

    if !input.transcript.trim().is_empty() {
        md.push_str("## 附：完整转写\n\n");
        md.push_str("> 由本地语音识别生成，可能存在识别错误，仅供检索参考。\n\n");
        md.push_str(input.transcript.trim());
        md.push_str("\n");
    }

    md
}

/// 生成相对链接（正斜杠，跨平台可读）。
fn rel_link(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

// ---------------------------------------------------------------------------
// HTML（PDF 的中间格式）
// ---------------------------------------------------------------------------

/// HTML 转义。
pub fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// 渲染用于打印的 HTML。
///
/// 样式按 A4 纸排版设计：正文用系统自带的中文字体，避免任何字体加载问题。
pub fn render_html(input: &DocInput) -> String {
    let mut h = String::new();
    h.push_str("<!DOCTYPE html>\n<html lang=\"zh-CN\">\n<head>\n<meta charset=\"utf-8\">\n");
    h.push_str(&format!("<title>{}</title>\n", escape_html(&input.title())));
    h.push_str(
        r#"<style>
  @page { size: A4; margin: 18mm 16mm; }
  * { box-sizing: border-box; }
  body { font-family: "Microsoft YaHei", "微软雅黑", "PingFang SC", sans-serif;
         font-size: 11pt; line-height: 1.75; color: #1f2328; margin: 0; }
  h1 { font-size: 20pt; margin: 0 0 4px 0; color: #0b3d91; }
  h1 + .meta { color: #6a737d; font-size: 10pt; margin-bottom: 16px; }
  h2 { font-size: 14pt; margin: 22px 0 8px 0; color: #0b3d91;
       border-left: 4px solid #1f6feb; padding-left: 9px; }
  h3 { font-size: 11.5pt; margin: 16px 0 6px 0; color: #444; }
  table { border-collapse: collapse; margin: 8px 0 16px 0; font-size: 10.5pt; }
  td { border: 1px solid #d8dee4; padding: 4px 10px; }
  td:first-child { background: #f6f8fa; color: #57606a; white-space: nowrap; }
  ol { padding-left: 22px; } ol li { margin: 4px 0; }
  .notices { background: #fff8e1; border-left: 4px solid #f0a000;
             padding: 10px 14px; margin: 8px 0; border-radius: 3px; }
  .notices li { margin: 4px 0; }
  .kw { display: inline-block; background: #eef2ff; color: #1f4bd8;
        border-radius: 3px; padding: 1px 8px; margin: 2px 5px 2px 0; font-size: 10pt; }
  .shot { margin: 14px 0; page-break-inside: avoid; }
  .shot img { width: 100%; border: 1px solid #d8dee4; border-radius: 4px; }
  .shot .cap { font-size: 9.5pt; color: #57606a; margin-top: 5px;
               border-left: 3px solid #d8dee4; padding-left: 8px; }
  .transcript { font-size: 9.5pt; color: #444; background: #f6f8fa;
                padding: 12px 14px; border-radius: 4px; white-space: pre-wrap;
                page-break-before: always; }
</style>
</head>
<body>
"#,
    );

    h.push_str(&format!("<h1>{}</h1>\n", escape_html(&input.title())));
    h.push_str("<div class=\"meta\">");
    let mut meta: Vec<String> = Vec::new();
    if !input.course.trim().is_empty() {
        meta.push(escape_html(&input.course));
    }
    meta.push(escape_html(&input.date));
    if !input.teacher.trim().is_empty() {
        meta.push(format!("教师：{}", escape_html(&input.teacher)));
    }
    meta.push(format!("时长：{}", input.duration_text()));
    h.push_str(&meta.join(" ｜ "));
    h.push_str("</div>\n");

    if !input.summary.points.is_empty() {
        h.push_str("<h2>课堂重点</h2>\n<ol>\n");
        for p in &input.summary.points {
            h.push_str(&format!("<li>{}</li>\n", escape_html(p)));
        }
        h.push_str("</ol>\n");
    }

    if !input.summary.notices.is_empty() {
        h.push_str("<h2>重点通知</h2>\n<ul class=\"notices\">\n");
        for n in &input.summary.notices {
            h.push_str(&format!("<li>{}</li>\n", escape_html(n)));
        }
        h.push_str("</ul>\n");
    }

    let kws = input.all_keywords();
    if !kws.is_empty() {
        h.push_str("<h2>关键词</h2>\n<p>");
        for k in &kws {
            h.push_str(&format!("<span class=\"kw\">{}</span>", escape_html(k)));
        }
        h.push_str("</p>\n");
    }

    if !input.screenshots.is_empty() {
        h.push_str("<h2>课堂截图</h2>\n");
        for s in &input.screenshots {
            h.push_str("<div class=\"shot\">\n");
            h.push_str(&format!(
                "<img src=\"{}\" alt=\"课堂截图\">\n",
                escape_html(&rel_link(&s.path))
            ));
            h.push_str(&format!(
                "<div class=\"cap\"><b>{}</b> {}</div>\n",
                s.timestamp(),
                escape_html(s.caption.trim())
            ));
            h.push_str("</div>\n");
        }
    }

    if !input.transcript.trim().is_empty() {
        h.push_str("<h2>附：完整转写</h2>\n");
        h.push_str(&format!(
            "<div class=\"transcript\">{}</div>\n",
            escape_html(input.transcript.trim())
        ));
    }

    h.push_str("</body>\n</html>\n");
    h
}

// ---------------------------------------------------------------------------
// Word
// ---------------------------------------------------------------------------

/// 写出一份 .docx。
///
/// 用的是 `docx-rs`：它直接产出 OOXML，不依赖本机安装 Word。
pub fn write_docx(input: &DocInput, path: &Path) -> Result<()> {
    use docx_rs::*;

    let mut doc = Docx::new();

    // 标题
    doc = doc.add_paragraph(
        Paragraph::new()
            .add_run(Run::new().add_text(input.title()).size(36).bold())
            .align(AlignmentType::Left),
    );

    // 元信息表
    let mut meta_rows: Vec<(&str, String)> = Vec::new();
    if !input.course.trim().is_empty() {
        meta_rows.push(("课程", input.course.clone()));
    }
    meta_rows.push(("日期", input.date.clone()));
    if !input.teacher.trim().is_empty() {
        meta_rows.push(("教师", input.teacher.clone()));
    }
    meta_rows.push(("时长", input.duration_text()));
    if !input.video_name.trim().is_empty() {
        meta_rows.push(("录像", input.video_name.clone()));
    }
    for (k, v) in meta_rows {
        doc = doc.add_paragraph(
            Paragraph::new().add_run(
                Run::new()
                    .add_text(format!("{k}：{v}"))
                    .size(20)
                    .color("666666"),
            ),
        );
    }
    doc = doc.add_paragraph(Paragraph::new());

    // 课堂重点
    if !input.summary.points.is_empty() {
        doc = doc.add_paragraph(
            Paragraph::new().add_run(Run::new().add_text("课堂重点").size(28).bold()),
        );
        for (i, p) in input.summary.points.iter().enumerate() {
            doc = doc.add_paragraph(
                Paragraph::new()
                    .add_run(Run::new().add_text(format!("{}. {p}", i + 1)).size(22)),
            );
        }
        doc = doc.add_paragraph(Paragraph::new());
    }

    // 重点通知
    if !input.summary.notices.is_empty() {
        doc = doc.add_paragraph(
            Paragraph::new().add_run(Run::new().add_text("重点通知").size(28).bold()),
        );
        for n in &input.summary.notices {
            doc = doc.add_paragraph(
                Paragraph::new().add_run(Run::new().add_text(format!("• {n}")).size(22)),
            );
        }
        doc = doc.add_paragraph(Paragraph::new());
    }

    // 关键词
    let kws = input.all_keywords();
    if !kws.is_empty() {
        doc = doc.add_paragraph(
            Paragraph::new().add_run(Run::new().add_text("关键词").size(28).bold()),
        );
        doc = doc.add_paragraph(
            Paragraph::new().add_run(Run::new().add_text(kws.join("  ")).size(22)),
        );
        doc = doc.add_paragraph(Paragraph::new());
    }

    // 截图（读成字节交给 docx-rs —— 它的 Pic::new 接受 &[u8] 而不是路径）
    if !input.screenshots.is_empty() {
        doc = doc.add_paragraph(
            Paragraph::new().add_run(Run::new().add_text("课堂截图").size(28).bold()),
        );
        for s in &input.screenshots {
            let bytes = match std::fs::read(&s.path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            // 内容宽度 A4 去掉页边距约 16.5cm；按 16:9 估算高度。
            // 1 cm = 360000 EMU。
            let w_emu: u32 = 16 * 360_000;
            let h_emu: u32 = (w_emu as f64 * 9.0 / 16.0) as u32;
            doc = doc.add_paragraph(
                Paragraph::new().add_run(
                    Run::new()
                        .add_text(format!("{}  ", s.timestamp()))
                        .size(20)
                        .color("666666"),
                ),
            );
            doc = doc.add_paragraph(
                Paragraph::new()
                    .add_run(Run::new().add_image(Pic::new(&bytes).size(w_emu, h_emu))),
            );
            if !s.caption.trim().is_empty() {
                doc = doc.add_paragraph(
                    Paragraph::new().add_run(
                        Run::new()
                            .add_text(s.caption.trim().to_string())
                            .size(18)
                            .color("666666"),
                    ),
                );
            }
            doc = doc.add_paragraph(Paragraph::new());
        }
    }

    // 附：完整转写
    if !input.transcript.trim().is_empty() {
        doc = doc.add_paragraph(
            Paragraph::new().add_run(Run::new().add_text("附：完整转写").size(28).bold()),
        );
        doc = doc.add_paragraph(
            Paragraph::new().add_run(
                Run::new()
                    .add_text("由本地语音识别生成，可能存在识别错误，仅供检索参考。")
                    .size(18)
                    .color("888888"),
            ),
        );
        // Word 单段太长的文本会很难看，按行拆段
        for line in input.transcript.lines().take(3000) {
            if line.trim().is_empty() {
                continue;
            }
            doc = doc.add_paragraph(
                Paragraph::new().add_run(Run::new().add_text(line.trim()).size(19)),
            );
        }
    }

    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let file = std::fs::File::create(path)
        .with_context(|| format!("创建 docx 失败: {}", path.display()))?;
    doc.build()
        .pack(file)
        .map_err(|e| anyhow::anyhow!("写入 docx 失败: {e}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// PDF（经 Edge 无头打印）
// ---------------------------------------------------------------------------

/// 用系统浏览器的无头模式把 HTML 打印成 PDF。
pub fn html_to_pdf(html: &Path, pdf: &Path) -> Result<()> {
    let browser = browser_bot::find_system_browser().ok_or_else(|| {
        anyhow::anyhow!("找不到 Edge 或 Chrome，无法生成 PDF（可用 VCA_BROWSER 指定）")
    })?;
    if let Some(p) = pdf.parent() {
        std::fs::create_dir_all(p)?;
    }
    let _ = std::fs::remove_file(pdf);

    // file:/// URL 必须是正斜杠
    let abs = std::fs::canonicalize(html).unwrap_or_else(|_| html.to_path_buf());
    let url = format!("file:///{}", abs.to_string_lossy().replace('\\', "/"));

    let out = proc::silent(&browser)
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--no-sandbox")
        .arg("--no-pdf-header-footer")
        .arg(format!("--print-to-pdf={}", pdf.display()))
        .arg(&url)
        .output()
        .context("启动浏览器打印 PDF 失败")?;

    if !pdf.is_file() || std::fs::metadata(pdf).map(|m| m.len()).unwrap_or(0) == 0 {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow::anyhow!(
            "浏览器没有产出 PDF（退出码 {:?}）：{}",
            out.status.code(),
            err.trim().chars().take(300).collect::<String>()
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 入口
// ---------------------------------------------------------------------------

/// 生成文档。`formats` 取值 `md` / `docx` / `pdf`（不区分大小写）。
///
/// 单个格式失败不会中断整体：能出几种就出几种，失败原因记进 [`DocOutput::notes`]。
pub fn generate(input: &DocInput, out_dir: &Path, formats: &[String]) -> Result<DocOutput> {
    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("创建文档目录失败: {}", out_dir.display()))?;

    let want: Vec<String> = if formats.is_empty() {
        vec!["md".into(), "docx".into(), "pdf".into()]
    } else {
        formats.iter().map(|s| s.to_lowercase()).collect()
    };
    let base = input.base_name();
    let mut out = DocOutput::default();

    let md_path = out_dir.join(format!("{base}.md"));
    let html_path = out_dir.join(format!("{base}.html"));
    let docx_path = out_dir.join(format!("{base}.docx"));
    let pdf_path = out_dir.join(format!("{base}.pdf"));

    // Markdown 总是生成：它是另外两种格式的源，也是失败时的兜底。
    std::fs::write(&md_path, render_markdown(input))
        .with_context(|| format!("写 Markdown 失败: {}", md_path.display()))?;
    out.markdown = Some(md_path);

    let html = render_html(input);

    if want.iter().any(|f| f == "pdf") {
        // HTML 只作为 PDF 的中间产物，写完就删（和 md 内容重复，留着只会让人困惑）
        match std::fs::write(&html_path, &html) {
            Ok(()) => match html_to_pdf(&html_path, &pdf_path) {
                Ok(()) => {
                    out.pdf = Some(pdf_path);
                    let _ = std::fs::remove_file(&html_path);
                }
                Err(e) => {
                    out.notes.push(format!("PDF 生成失败：{e}"));
                    // 保留 HTML，用户可以直接用浏览器打开另存为 PDF
                    out.notes
                        .push(format!("已保留网页版本，可用浏览器打开后另存为 PDF：{}", html_path.display()));
                }
            },
            Err(e) => out.notes.push(format!("写 HTML 失败：{e}")),
        }
    }

    if want.iter().any(|f| f == "docx") {
        match write_docx(input, &docx_path) {
            Ok(()) => out.docx = Some(docx_path),
            Err(e) => out.notes.push(format!("Word 生成失败：{e}")),
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LessonSummary;

    fn sample() -> DocInput {
        DocInput {
            course: "数学".into(),
            date: "2026-09-19".into(),
            teacher: "张老师".into(),
            summary: LessonSummary {
                title: "二次函数图象".into(),
                points: vec!["顶点坐标公式".into(), "开口方向判别".into()],
                notices: vec!["完成练习册 P32".into()],
                keywords: vec!["二次函数".into()],
            },
            transcript: "今天我们讲二次函数".into(),
            screenshots: vec![ShotRef {
                path: PathBuf::from("shots/shot00001.jpg"),
                at_ms: 754_000,
                caption: "讲解顶点公式".into(),
            }],
            duration_secs: 2712,
            video_name: "20260919_数学_1.mp4".into(),
            extra_keywords: vec!["抛物线".into()],
        }
    }

    #[test]
    fn timestamp_formats_mm_ss_and_h_mm_ss() {
        assert_eq!(format_ts(0), "00:00");
        assert_eq!(format_ts(65_000), "01:05");
        assert_eq!(format_ts(3_725_000), "1:02:05");
    }

    #[test]
    fn duration_text_is_human_readable() {
        let mut d = sample();
        d.duration_secs = 2712;
        assert_eq!(d.duration_text(), "45 分 12 秒");
        d.duration_secs = 30;
        assert_eq!(d.duration_text(), "30 秒");
        d.duration_secs = 7200;
        assert_eq!(d.duration_text(), "2 小时 0 分");
        d.duration_secs = 0;
        assert_eq!(d.duration_text(), "—");
    }

    #[test]
    fn base_name_strips_path_separators() {
        let mut d = sample();
        d.course = "数学/物理".into();
        let n = d.base_name();
        assert!(!n.contains('/'), "文件名不能含路径分隔符: {n}");
        assert!(n.contains("2026-09-19"));
    }

    #[test]
    fn markdown_contains_all_sections() {
        let md = render_markdown(&sample());
        assert!(md.contains("# 二次函数图象"));
        assert!(md.contains("## 课堂重点"));
        assert!(md.contains("## 重点通知"));
        assert!(md.contains("## 关键词"));
        assert!(md.contains("## 课堂截图"));
        assert!(md.contains("## 附：完整转写"));
        // 截图要带时间戳与图注
        assert!(md.contains("12:34"), "截图时间戳缺失: {md}");
        assert!(md.contains("讲解顶点公式"));
        // 图片链接必须是正斜杠，Markdown 阅读器才认
        assert!(md.contains("shots/shot00001.jpg"));
    }

    #[test]
    fn markdown_omits_empty_sections() {
        let mut d = sample();
        d.summary.notices.clear();
        d.summary.keywords.clear();
        d.extra_keywords.clear();
        d.screenshots.clear();
        d.transcript.clear();
        let md = render_markdown(&d);
        assert!(!md.contains("## 重点通知"));
        assert!(!md.contains("## 关键词"));
        assert!(!md.contains("## 课堂截图"));
        assert!(!md.contains("## 附：完整转写"));
    }

    #[test]
    fn html_escapes_dangerous_characters() {
        let mut d = sample();
        d.summary.points = vec!["a < b && c > d \"引号\"".into()];
        let html = render_html(&d);
        assert!(html.contains("&lt;"), "尖括号必须转义");
        assert!(html.contains("&amp;"), "& 必须转义");
        assert!(!html.contains("a < b"), "不能出现未转义的原文");
    }

    #[test]
    fn html_has_print_styles_and_a4_page() {
        let html = render_html(&sample());
        assert!(html.contains("@page"), "缺少打印分页设置");
        assert!(html.contains("A4"));
        // 中文字体必须用系统自带的名字，否则 PDF 会掉字
        assert!(html.contains("Microsoft YaHei"));
        assert!(html.contains("page-break-inside: avoid"), "截图不应被分页截断");
    }

    #[test]
    fn keywords_are_deduplicated_in_order() {
        let mut d = sample();
        d.extra_keywords = vec!["二次函数".into(), "抛物线".into()];
        let k = d.all_keywords();
        assert_eq!(k, vec!["二次函数", "抛物线"]);
    }

    #[test]
    fn generate_always_writes_markdown_even_without_other_formats() {
        let dir = std::env::temp_dir().join("vca_docgen_test_md");
        let _ = std::fs::remove_dir_all(&dir);
        let out = generate(&sample(), &dir, &["md".into()]).unwrap();
        assert!(out.markdown.is_some());
        assert!(out.docx.is_none());
        assert!(out.pdf.is_none());
        assert!(out.markdown.as_ref().unwrap().is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generate_docx_produces_a_real_file() {
        let dir = std::env::temp_dir().join("vca_docgen_test_docx");
        let _ = std::fs::remove_dir_all(&dir);
        let out = generate(&sample(), &dir, &["md".into(), "docx".into()]).unwrap();
        let docx = out.docx.expect("应产出 docx");
        let size = std::fs::metadata(&docx).map(|m| m.len()).unwrap_or(0);
        // docx 是 zip 包，至少要有几百字节
        assert!(size > 500, "docx 太小，可能没写成功: {size} 字节");
        // 校验确实是 zip（前两字节 PK）
        let head = std::fs::read(&docx).unwrap();
        assert_eq!(&head[0..2], b"PK", "docx 应当是 zip 容器");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
