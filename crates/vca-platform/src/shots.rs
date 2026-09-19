//! 截图关联：感知哈希去重 + 与转写时间对齐。
//!
//! # 解决什么问题
//!
//! 录制时按固定间隔（默认 180 秒）抓屏，一节课下来会拿到十几张图，
//! 但其中大部分是**同一个课件页面**——老师在上面讲了三分钟，屏幕一个字没变。
//! 直接全塞进文档，结果是十几张一模一样的图，反而把真正的重点淹没了。
//!
//! 这里做两件事：
//!
//! 1. **去重**：用 dHash（差分感知哈希）比较相邻截图，只保留画面发生
//!    明显变化的那几张。选 dHash 而不是逐像素比较，是因为它对 JPEG 压缩噪声、
//!    鼠标位置微动、编码抖动都不敏感。
//! 2. **对齐**：每张截图带一个时间点，去转写结果里找那一刻老师在说什么，
//!    作为图注。于是文档里是「讲到顶点公式时的板书」，
//!    而不是一张不知所谓的屏幕照片。
//!
//! # 时间点是怎么来的
//!
//! ffmpeg 的 `fps=1/N` 序列输出只给序号（`shot00001.jpg`），
//! 所以第 k 张对应 `(k-1) * N` 秒。这比读文件修改时间更可靠
//! （文件时间会在拷贝、解压、同步时被改写）。

use std::path::{Path, PathBuf};

use crate::docgen::ShotRef;
use crate::stt::Segment;

/// 一张候选截图。
#[derive(Debug, Clone, PartialEq)]
pub struct ShotCandidate {
    /// 图片路径。
    pub path: PathBuf,
    /// 序号（从 1 开始，取自文件名）。
    pub seq: u32,
    /// 在课堂里的时间点（毫秒）。
    pub at_ms: u64,
    /// dHash（没算出来时为 `None`）。
    pub hash: Option<u64>,
}

/// 从截图目录收集候选，按文件名序号推算时间点。
///
/// 只认 `shot<数字>.<ext>` 这种命名（ffmpeg 的 image2 muxer 产出的格式）。
pub fn collect(shot_dir: &Path, interval_secs: u64) -> Vec<ShotCandidate> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(shot_dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let is_img = path
            .extension()
            .map(|e| {
                let e = e.to_string_lossy().to_ascii_lowercase();
                e == "jpg" || e == "jpeg" || e == "png"
            })
            .unwrap_or(false);
        if !is_img {
            continue;
        }
        let Some(seq) = parse_seq(&path) else {
            continue;
        };
        out.push(ShotCandidate {
            path,
            seq,
            at_ms: (seq.saturating_sub(1) as u64) * interval_secs * 1000,
            hash: None,
        });
    }
    out.sort_by_key(|s| s.seq);
    out
}

/// 从 `shot00007.jpg` 里抠出 7。
pub fn parse_seq(path: &Path) -> Option<u32> {
    let stem = path.file_stem()?.to_string_lossy().to_string();
    let digits: String = stem
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    // 同时也接受整串都是数字的情况（`00007.jpg`）
    if digits.is_empty() {
        return None;
    }
    digits.parse::<u32>().ok().filter(|n| *n > 0)
}

/// 计算 dHash（8×8 差分哈希）。
///
/// 做法：缩到 9×8 灰度，逐行比较相邻像素亮度，得到 64 位指纹。
/// 相邻像素的相对关系比绝对亮度稳定，所以对整体明暗变化不敏感。
pub fn dhash(path: &Path) -> Option<u64> {
    let img = image::open(path).ok()?;
    let gray = img.to_luma8();
    let small = image::imageops::resize(&gray, 9, 8, image::imageops::FilterType::Triangle);
    let mut hash: u64 = 0;
    let mut bit = 0;
    for y in 0..8u32 {
        for x in 0..8u32 {
            let l = small.get_pixel(x, y)[0];
            let r = small.get_pixel(x + 1, y)[0];
            if l < r {
                hash |= 1u64 << bit;
            }
            bit += 1;
        }
    }
    Some(hash)
}

/// 汉明距离（有多少位不同）。
pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// 默认去重阈值：dHash 有 64 位，差异 ≤ 5 位基本可以认为「同一画面」。
pub const DEFAULT_THRESHOLD: u32 = 5;

/// 去重：丢掉与「上一张保留下来的图」过于相似的帧。
///
/// 注意是**与上一张保留的**比较，不是与上一张原始图比较 ——
/// 否则一个缓慢变化的过程（比如动画）会因为每步只差一点而被全部保留。
pub fn dedupe(shots: &[ShotCandidate], threshold: u32, limit: usize) -> Vec<ShotCandidate> {
    let mut kept: Vec<ShotCandidate> = Vec::new();
    let mut last_hash: Option<u64> = None;

    for s in shots {
        let h = s.hash.or_else(|| dhash(&s.path));
        match (h, last_hash) {
            (Some(cur), Some(prev)) if hamming(cur, prev) <= threshold => {
                // 与上一张保留的图几乎一样，跳过
                continue;
            }
            _ => {}
        }
        let mut keep = s.clone();
        keep.hash = h;
        last_hash = h;
        kept.push(keep);
        if limit > 0 && kept.len() >= limit {
            break;
        }
    }

    // 一首都留不下时（比如只有一张图）也至少要给出第一张
    if kept.is_empty() {
        if let Some(first) = shots.first() {
            kept.push(first.clone());
        }
    }
    kept
}

/// 找出 `at_ms` 时刻老师在说什么。
///
/// 先找覆盖该时刻的分段；找不到就取时间上最近的一段
/// （间隔超过 `window_ms` 则返回空，避免把不相干的文字贴上去）。
pub fn caption_at(segments: &[Segment], at_ms: u64, window_ms: u64) -> String {
    if segments.is_empty() {
        return String::new();
    }

    // 1) 覆盖该时刻的分段。
    //    用半开区间 [start, end)：时间轴上相邻两段常常首尾相接
    //    （上一段 end == 下一段 start），闭区间会让边界点被前一段抢走，
    //    结果截在 60 秒的那张图配了第 0~60 秒的讲稿。
    for s in segments {
        if at_ms >= s.start_ms && at_ms < s.end_ms {
            let t = s.text.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
    }

    // 恰好落在最后一段的结束点上时，仍然归最后一段
    if let Some(last) = segments.last() {
        if at_ms == last.end_ms && last.end_ms > last.start_ms {
            let t = last.text.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
    }

    // 2) 之后最近的一段（截图往往落在两句话之间）
    let mut after: Vec<&Segment> = segments.iter().filter(|s| s.start_ms > at_ms).collect();
    after.sort_by_key(|s| s.start_ms);
    if let Some(s) = after.first() {
        if s.start_ms - at_ms <= window_ms {
            let t = s.text.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
    }

    // 3) 之前最近的一段
    let mut before: Vec<&Segment> = segments.iter().filter(|s| s.end_ms < at_ms).collect();
    before.sort_by_key(|s| s.end_ms);
    if let Some(s) = before.last() {
        if at_ms - s.end_ms <= window_ms {
            let t = s.text.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
    }

    String::new()
}

/// 一步到位：收集 → 去重 → 生成可放进文档的截图引用。
///
/// `segments` 为空（比如用了不支持时间戳的云端转写）时，
/// 图注留空但截图仍然保留 —— 有图总比没图强。
pub fn build_refs(
    shot_dir: &Path,
    interval_secs: u64,
    segments: &[Segment],
    max_shots: usize,
) -> Vec<ShotRef> {
    let all = collect(shot_dir, interval_secs);
    if all.is_empty() {
        return Vec::new();
    }
    let limit = if max_shots == 0 { 8 } else { max_shots };
    let kept = dedupe(&all, DEFAULT_THRESHOLD, limit);
    // 图注窗口取截图间隔的一半：超过这个跨度就不要硬拽文字过来了
    let window = (interval_secs * 1000 / 2).max(5_000);

    kept.into_iter()
        .map(|s| {
            let caption = caption_at(segments, s.at_ms, window);
            ShotRef {
                path: s.path,
                at_ms: s.at_ms,
                caption,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: u64, end: u64, text: &str) -> Segment {
        Segment {
            start_ms: start,
            end_ms: end,
            text: text.to_string(),
        }
    }

    #[test]
    fn seq_is_parsed_from_ffmpeg_names() {
        assert_eq!(parse_seq(Path::new("shot00001.jpg")), Some(1));
        assert_eq!(parse_seq(Path::new("shot00042.jpg")), Some(42));
        assert_eq!(parse_seq(Path::new("00007.png")), Some(7));
        assert_eq!(parse_seq(Path::new("shot.jpg")), None);
        assert_eq!(parse_seq(Path::new("shot00000.jpg")), None, "序号从 1 开始");
    }

    #[test]
    fn timestamps_follow_the_capture_interval() {
        let dir = std::env::temp_dir().join("vca_shots_collect");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 1..=3 {
            std::fs::write(dir.join(format!("shot{i:05}.jpg")), b"x").unwrap();
        }
        let c = collect(&dir, 180);
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].at_ms, 0);
        assert_eq!(c[1].at_ms, 180_000);
        assert_eq!(c[2].at_ms, 360_000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn collect_ignores_non_image_files() {
        let dir = std::env::temp_dir().join("vca_shots_ignore");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("shot00001.jpg"), b"x").unwrap();
        std::fs::write(dir.join("readme.txt"), b"x").unwrap();
        std::fs::write(dir.join("notes.md"), b"x").unwrap();
        let c = collect(&dir, 60);
        assert_eq!(c.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn caption_prefers_the_covering_segment() {
        let segs = vec![
            seg(0, 5_000, "上课"),
            seg(5_000, 12_000, "讲顶点公式"),
            seg(12_000, 20_000, "做例题"),
        ];
        assert_eq!(caption_at(&segs, 8_000, 90_000), "讲顶点公式");
    }

    #[test]
    fn caption_falls_back_to_the_next_segment_within_window() {
        let segs = vec![seg(5_100, 9_000, "讲顶点公式")];
        // 截图落在 5000，下一句 5100 开始 —— 应当取到
        assert_eq!(caption_at(&segs, 5_000, 90_000), "讲顶点公式");
        // 窗口很窄时不硬拽
        assert_eq!(caption_at(&segs, 5_000, 50), "");
    }

    #[test]
    fn caption_falls_back_to_the_previous_segment() {
        let segs = vec![seg(1_000, 4_000, "上一句")];
        assert_eq!(caption_at(&segs, 4_500, 90_000), "上一句");
        // 窗口只有 5 秒，86 秒前的文字不该被硬拽过来当图注
        assert_eq!(caption_at(&segs, 90_000, 5_000), "");
    }

    #[test]
    fn caption_picks_the_later_segment_at_a_shared_boundary() {
        // 相邻两段首尾相接（上一段 end == 下一段 start）时，
        // 边界点应归后一段，否则「截在第 60 秒的图」会配上第 0~60 秒的讲稿。
        let segs = vec![seg(0, 60_000, "前半节"), seg(60_000, 120_000, "后半节")];
        assert_eq!(caption_at(&segs, 60_000, 90_000), "后半节");
        assert_eq!(caption_at(&segs, 59_999, 90_000), "前半节");
    }

    #[test]
    fn caption_is_empty_without_segments() {
        assert_eq!(caption_at(&[], 1_000, 90_000), "");
    }

    #[test]
    fn hamming_counts_differing_bits() {
        assert_eq!(hamming(0, 0), 0);
        assert_eq!(hamming(0b1011, 0b1001), 1);
        assert_eq!(hamming(u64::MAX, 0), 64);
    }

    /// 生成一张有内容的测试 JPEG。
    fn make_jpeg(path: &Path, seed: u8) {
        use image::{ImageBuffer, Rgb};
        let mut img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::new(64, 48);
        for (x, y, px) in img.enumerate_pixels_mut() {
            // 用 seed 决定明暗走势，做出一张左右渐变图
            let v = ((x as u16 * (seed as u16 + 2)) % 256) as u8;
            let w = ((y as u16 * 3) % 256) as u8;
            *px = Rgb([v, w, seed]);
        }
        img.save(path).unwrap();
    }

    #[test]
    fn dhash_is_stable_for_the_same_image() {
        let dir = std::env::temp_dir().join("vca_shots_hash");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.jpg");
        make_jpeg(&p, 10);
        let h1 = dhash(&p).expect("应能算出哈希");
        let h2 = dhash(&p).expect("应能算出哈希");
        assert_eq!(h1, h2, "同一张图的 dHash 必须稳定");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dhash_differs_for_different_images() {
        let dir = std::env::temp_dir().join("vca_shots_hash2");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.jpg");
        let b = dir.join("b.jpg");
        make_jpeg(&a, 1);
        make_jpeg(&b, 200);
        let ha = dhash(&a).unwrap();
        let hb = dhash(&b).unwrap();
        assert!(
            hamming(ha, hb) > DEFAULT_THRESHOLD,
            "明显不同的两张图不该被判为重复（距离 {}）",
            hamming(ha, hb)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dedupe_keeps_one_of_two_identical_frames() {
        let dir = std::env::temp_dir().join("vca_shots_dedupe");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 三张内容完全相同的图 + 一张不同的
        for i in 1..=3 {
            make_jpeg(&dir.join(format!("shot{i:05}.jpg")), 42);
        }
        make_jpeg(&dir.join("shot00004.jpg"), 200);

        let all = collect(&dir, 60);
        assert_eq!(all.len(), 4);
        let kept = dedupe(&all, DEFAULT_THRESHOLD, 8);
        // 重复的三张只留一张，第四张不同要保留
        assert_eq!(
            kept.len(),
            2,
            "实际保留 {:?}",
            kept.iter().map(|k| k.seq).collect::<Vec<_>>()
        );
        assert_eq!(kept[0].seq, 1);
        assert_eq!(kept[1].seq, 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dedupe_respects_limit() {
        let dir = std::env::temp_dir().join("vca_shots_limit");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 1..=6 {
            make_jpeg(&dir.join(format!("shot{i:05}.jpg")), (i * 40) as u8);
        }
        let all = collect(&dir, 60);
        let kept = dedupe(&all, DEFAULT_THRESHOLD, 2);
        assert!(kept.len() <= 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dedupe_keeps_at_least_one_when_all_identical() {
        let dir = std::env::temp_dir().join("vca_shots_one");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 1..=4 {
            make_jpeg(&dir.join(format!("shot{i:05}.jpg")), 7);
        }
        let all = collect(&dir, 60);
        let kept = dedupe(&all, DEFAULT_THRESHOLD, 8);
        assert_eq!(kept.len(), 1, "全一样时也要留一张，不能返回空");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_refs_attaches_captions() {
        let dir = std::env::temp_dir().join("vca_shots_refs");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        make_jpeg(&dir.join("shot00001.jpg"), 10);
        make_jpeg(&dir.join("shot00002.jpg"), 220);

        let segs = vec![
            seg(0, 60_000, "第一段讲解"),
            seg(60_000, 120_000, "第二段讲解"),
        ];
        let refs = build_refs(&dir, 60, &segs, 8);
        assert!(!refs.is_empty());
        assert_eq!(refs[0].at_ms, 0);
        assert_eq!(refs[0].caption, "第一段讲解");
        if refs.len() > 1 {
            assert_eq!(refs[1].caption, "第二段讲解");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_refs_on_missing_dir_is_empty_not_panic() {
        let refs = build_refs(Path::new("这个目录不存在"), 60, &[], 8);
        assert!(refs.is_empty());
    }
}
