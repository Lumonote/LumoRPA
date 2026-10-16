//! 版面重建:带坐标的文本片段 → 有阅读顺序的块。
//!
//! 这一层全是**启发式**,所以产出的块一律带 `confidence` 与
//! `Source::Layout`(标题层级这类推断)。与其把不确定性藏起来,不如让调用方看见
//! 并决定是否人工复核 —— 这是 DocIR「可审计」设计的落点。
//!
//! 步骤:分栏 → 行聚类 → 段落合并 → 标题识别 → 页眉页脚剔除 → 阅读顺序。

use crate::ir::*;
use crate::pdf::text::Fragment;

/// 同一行的基线容差(相对字号)。
const LINE_TOLERANCE: f64 = 0.5;
/// 段落断开:行距超过正文行距的这个倍数即视为新段。
const PARAGRAPH_BREAK: f64 = 1.6;
/// 标题字号阈值:超过正文中位数的这个倍数。
const HEADING_RATIO: f64 = 1.15;

#[derive(Debug, Clone)]
pub struct Line {
    pub text: String,
    pub x0: f64,
    pub x1: f64,
    /// 页面坐标下的基线 y(左下原点)。
    pub y: f64,
    pub font_size: f64,
}

/// 把片段按基线聚成行,行内按 x 排序后拼接。
pub fn cluster_lines(fragments: &[Fragment]) -> Vec<Line> {
    let mut sorted: Vec<&Fragment> = fragments.iter().filter(|f| !f.text.trim().is_empty()).collect();
    // y 降序(页面自上而下),同 y 按 x 升序
    sorted.sort_by(|a, b| {
        b.y.partial_cmp(&a.y)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
    });

    let mut lines: Vec<Vec<&Fragment>> = Vec::new();
    for fragment in sorted {
        let tolerance = (fragment.font_size * LINE_TOLERANCE).max(1.0);
        match lines.last_mut() {
            Some(current) if (current[0].y - fragment.y).abs() <= tolerance => {
                current.push(fragment);
            }
            _ => lines.push(vec![fragment]),
        }
    }

    lines
        .into_iter()
        .map(|mut group| {
            group.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
            let x0 = group.first().map(|f| f.x).unwrap_or(0.0);
            let last = group.last().unwrap();
            let x1 = last.x + last.width;
            // 片段之间若有明显空隙,补一个空格,避免 "总计" + "100" 粘成 "总计100"
            let mut text = String::new();
            let mut prev_end: Option<f64> = None;
            for fragment in &group {
                if let Some(end) = prev_end {
                    let gap = fragment.x - end;
                    if gap > fragment.font_size * 0.25 && !text.ends_with(' ') {
                        text.push(' ');
                    }
                }
                text.push_str(&fragment.text);
                prev_end = Some(fragment.x + fragment.width);
            }
            let font_size = group
                .iter()
                .map(|f| f.font_size)
                .fold(0.0_f64, |acc, size| acc.max(size));
            Line {
                text: text.trim().to_string(),
                x0,
                x1,
                y: group[0].y,
                font_size,
            }
        })
        .filter(|line| !line.text.is_empty())
        .collect()
}

/// 正文字号 = **按字符数加权的众数**。
///
/// 早先用中位数,在行数少时会被标题带偏(只有「标题 + 一行正文」时,中位数取到
/// 的是标题字号,于是标题反而识别不出来)。正文在字符量上总是压倒性的,按字符
/// 数投票更稳。
pub fn body_font_size(lines: &[Line]) -> f64 {
    use std::collections::HashMap;
    let mut weight: HashMap<u32, usize> = HashMap::new();
    for line in lines {
        // 量化到 0.5pt,吸收同一字号的浮点抖动
        let key = (line.font_size * 2.0).round() as u32;
        *weight.entry(key).or_default() += line.text.chars().count().max(1);
    }
    weight
        .into_iter()
        // 同权重时取较小字号 —— 正文比标题小
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
        .map(|(key, _)| key as f64 / 2.0)
        .unwrap_or(0.0)
}

/// 把标题字号映射到 1..=6 级:比正文越大级别越高。
fn heading_level(font_size: f64, body: f64) -> u8 {
    if body <= 0.0 {
        return 1;
    }
    let ratio = font_size / body;
    match ratio {
        r if r >= 2.0 => 1,
        r if r >= 1.6 => 2,
        r if r >= 1.35 => 3,
        r if r >= HEADING_RATIO => 4,
        _ => 5,
    }
}

/// 页眉页脚候选所在的页边带(占页高比例)。只看重复不看位置会误伤正文 ——
/// 短页面上正文行同样「每页都出现」。
const MARGIN_BAND: f64 = 0.12;

/// 在**多页**范围内找出既落在页边带、又跨页重复的短文本 —— 页眉页脚。
/// 单页/两页文档无从判断重复,一律不剔除(宁可多留也不误删正文)。
pub fn running_head_texts(pages: &[(Vec<Line>, f64)]) -> std::collections::HashSet<String> {
    use std::collections::{HashMap, HashSet};
    if pages.len() < 3 {
        return HashSet::new();
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (lines, height) in pages {
        for line in lines {
            if !in_margin_band(line.y, *height) {
                continue;
            }
            if line.text.chars().count() <= 40 {
                *counts.entry(normalize_running_head(&line.text)).or_default() += 1;
            }
        }
    }
    let threshold = (pages.len() as f64 * 0.6).ceil() as usize;
    counts
        .into_iter()
        .filter(|(_, count)| *count >= threshold)
        .map(|(text, _)| text)
        .collect()
}

/// 基线 y(左下原点)是否落在上下页边带内。
pub fn in_margin_band(y: f64, page_height: f64) -> bool {
    if page_height <= 0.0 {
        return false;
    }
    y >= page_height * (1.0 - MARGIN_BAND) || y <= page_height * MARGIN_BAND
}

/// 页码会变(第 1 页 / 第 2 页),归一化后才能识别出「同一个页脚」。
fn normalize_running_head(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_ascii_digit() { '#' } else { c })
        .collect()
}

/// 一页的行 → 块。`order_base` 是全局阅读顺序的起点。
pub fn lines_to_blocks(
    lines: &[Line],
    page: usize,
    page_height: f64,
    body: f64,
    running_heads: &std::collections::HashSet<String>,
    order_base: usize,
    block_seq: &mut usize,
) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut order = order_base;
    // 行距中位数用于判断段落断开
    let gaps: Vec<f64> = lines
        .windows(2)
        .map(|w| (w[0].y - w[1].y).abs())
        .filter(|g| *g > 0.0)
        .collect();
    let median_gap = {
        let mut g = gaps.clone();
        g.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        g.get(g.len() / 2).copied().unwrap_or(0.0)
    };

    let mut pending: Option<(String, f64, f64, f64, f64)> = None; // text, x0, x1, y_top, y_bottom
    let flush = |pending: &mut Option<(String, f64, f64, f64, f64)>,
                 blocks: &mut Vec<Block>,
                 order: &mut usize,
                 block_seq: &mut usize| {
        if let Some((text, x0, x1, y_top, y_bottom)) = pending.take() {
            *block_seq += 1;
            let mut block = Block::text_layer(
                format!("b{block_seq}"),
                BlockType::Paragraph,
                page,
                *order,
                text.trim(),
            );
            block.bbox = Some(BBox::new(x0, page_height - y_top, x1, page_height - y_bottom));
            blocks.push(block);
            *order += 1;
        }
    };

    for (index, line) in lines.iter().enumerate() {
        // 页眉页脚:既要文本跨页重复,又要落在页边带内。用位置而非行序判断
        // 上下 —— 行序在只有一两行的页面上完全不可靠。
        if in_margin_band(line.y, page_height)
            && running_heads.contains(&normalize_running_head(&line.text))
        {
            flush(&mut pending, &mut blocks, &mut order, block_seq);
            *block_seq += 1;
            let is_header = line.y >= page_height * 0.5;
            let mut block = Block::text_layer(
                format!("b{block_seq}"),
                if is_header {
                    BlockType::Header
                } else {
                    BlockType::Footer
                },
                page,
                order,
                line.text.clone(),
            );
            block.bbox = Some(BBox::new(
                line.x0,
                page_height - line.y - line.font_size,
                line.x1,
                page_height - line.y,
            ));
            blocks.push(block);
            order += 1;
            continue;
        }

        // 标题:字号显著大于正文,且独占一行
        if body > 0.0 && line.font_size >= body * HEADING_RATIO {
            flush(&mut pending, &mut blocks, &mut order, block_seq);
            *block_seq += 1;
            let mut block = Block::text_layer(
                format!("b{block_seq}"),
                BlockType::Heading,
                page,
                order,
                line.text.clone(),
            );
            block.level = Some(heading_level(line.font_size, body));
            // 标题层级是从字号推断的,不是文档显式声明 —— 标注来源与置信度
            block.source = Source::Layout;
            block.confidence = 0.8;
            block.bbox = Some(BBox::new(
                line.x0,
                page_height - line.y - line.font_size,
                line.x1,
                page_height - line.y,
            ));
            blocks.push(block);
            order += 1;
            continue;
        }

        let gap_to_prev = index
            .checked_sub(1)
            .map(|i| (lines[i].y - line.y).abs())
            .unwrap_or(0.0);
        let starts_new_paragraph = pending.is_none()
            || (median_gap > 0.0 && gap_to_prev > median_gap * PARAGRAPH_BREAK);

        if starts_new_paragraph {
            flush(&mut pending, &mut blocks, &mut order, block_seq);
            pending = Some((
                line.text.clone(),
                line.x0,
                line.x1,
                line.y + line.font_size,
                line.y,
            ));
        } else if let Some((text, x0, x1, _, y_bottom)) = pending.as_mut() {
            // 中文行尾不补空格,西文补 —— 避免「合同签订」被拆成「合同 签订」
            if !text.ends_with(|c: char| (c as u32) >= 0x2E80) {
                text.push(' ');
            }
            text.push_str(&line.text);
            *x0 = x0.min(line.x0);
            *x1 = x1.max(line.x1);
            *y_bottom = line.y;
        }
    }
    flush(&mut pending, &mut blocks, &mut order, block_seq);
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frag(text: &str, x: f64, y: f64, size: f64) -> Fragment {
        Fragment {
            text: text.into(),
            x,
            y,
            width: text.chars().count() as f64 * size * 0.5,
            font_size: size,
            width_is_estimated: true,
        }
    }

    #[test]
    fn fragments_on_the_same_baseline_join_into_one_line() {
        let lines = cluster_lines(&[
            frag("世界", 60.0, 700.0, 12.0),
            frag("你好", 10.0, 700.0, 12.0),
            frag("第二行", 10.0, 680.0, 12.0),
        ]);
        assert_eq!(lines.len(), 2);
        // 同行按 x 排序,不按出现顺序
        assert!(lines[0].text.starts_with("你好"), "got: {}", lines[0].text);
        assert_eq!(lines[1].text, "第二行");
    }

    #[test]
    fn wide_gaps_within_a_line_become_a_space() {
        // 表格式排版:「总计」与数字之间有大空隙,不补空格会粘成一个词
        let lines = cluster_lines(&[frag("总计", 10.0, 700.0, 12.0), frag("100", 200.0, 700.0, 12.0)]);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].text.contains(' '), "got: {}", lines[0].text);
    }

    #[test]
    fn heading_level_scales_with_font_ratio() {
        assert_eq!(heading_level(24.0, 10.0), 1);
        assert_eq!(heading_level(17.0, 10.0), 2);
        assert_eq!(heading_level(14.0, 10.0), 3);
        assert_eq!(heading_level(12.0, 10.0), 4);
    }

    #[test]
    fn body_size_is_decided_by_character_volume_not_by_line_count() {
        let line = |text: &str, size: f64| Line {
            text: text.into(),
            x0: 0.0,
            x1: 1.0,
            y: 0.0,
            font_size: size,
        };
        // 一个短标题 + 两行长正文:正文字符量压倒性,基准应取正文字号
        let lines = vec![
            line("年度报告", 30.0),
            line("这是一段足够长的正文内容用于确定正文字号基准", 10.0),
            line("第二段同样是正文内容并且长度可观", 10.0),
        ];
        assert_eq!(body_font_size(&lines), 10.0, "单个大标题不该抬高正文基准");

        // 与行数无关:标题行数多但字符少时,基准仍是正文
        let heading_heavy = vec![
            line("标题一", 30.0),
            line("标题二", 30.0),
            line("这一行正文的字符数超过上面两个标题之和所以它才是正文", 10.0),
        ];
        assert_eq!(body_font_size(&heading_heavy), 10.0);
    }

    #[test]
    fn running_heads_need_repetition_and_a_margin_position() {
        const H: f64 = 842.0;
        // 页脚在下边带(y=40),正文在页面中部(y=700)
        let page = |footer: &str| {
            (
                vec![
                    Line { text: "正文内容".into(), x0: 0.0, x1: 1.0, y: 700.0, font_size: 10.0 },
                    Line { text: footer.into(), x0: 0.0, x1: 1.0, y: 40.0, font_size: 9.0 },
                ],
                H,
            )
        };
        let pages = vec![page("第 1 页"), page("第 2 页"), page("第 3 页"), page("第 4 页")];
        let heads = running_head_texts(&pages);
        // 页码归一化后跨页重复 → 识别为页脚
        assert!(heads.contains(&normalize_running_head("第 5 页")));
        // 正文虽然每页都一样,但不在页边带,不能被当成页眉页脚剔除
        assert!(
            !heads.contains(&normalize_running_head("正文内容")),
            "页面中部的重复文本不该被当成页眉页脚"
        );

        // 页数太少无从判断重复,不剔除
        assert!(running_head_texts(&pages[..2]).is_empty());
    }

    #[test]
    fn margin_band_covers_only_the_top_and_bottom_edges() {
        const H: f64 = 842.0;
        assert!(in_margin_band(800.0, H), "顶部");
        assert!(in_margin_band(40.0, H), "底部");
        assert!(!in_margin_band(500.0, H), "页面中部不是页边带");
    }

    #[test]
    fn headings_are_marked_as_layout_inference_not_ground_truth() {
        let lines = vec![
            Line { text: "大标题".into(), x0: 0.0, x1: 100.0, y: 700.0, font_size: 20.0 },
            Line { text: "正文".into(), x0: 0.0, x1: 100.0, y: 680.0, font_size: 10.0 },
            Line { text: "正文2".into(), x0: 0.0, x1: 100.0, y: 668.0, font_size: 10.0 },
        ];
        let mut seq = 0;
        let blocks = lines_to_blocks(&lines, 0, 842.0, 10.0, &Default::default(), 0, &mut seq);
        let heading = blocks.iter().find(|b| b.block_type == BlockType::Heading).unwrap();
        assert_eq!(heading.source, Source::Layout);
        assert!(heading.confidence < 1.0, "启发式结论不能标成满置信");
        assert!(heading.bbox.is_some());
    }
}
