//! PDF 解析入口:装配页、文本层、版面与有线表格。
//!
//! 与既有 `pdf.extract_text`(`lopdf::extract_text` 拼一个大字符串)的关系:那条
//! 路径仍在,面向「只要文字」的场景;本模块面向「要结构」——每块带页归属、bbox、
//! 阅读顺序与溯源。
//!
//! **扫描件**:无文本层的页会被识别出来(`text_coverage ≈ 0`)并在 `diagnostics`
//! 里点名,但 OCR 在 M3 才接。此处不静默返回空白页 —— 显式告警,让调用方知道
//! 这些页需要 OCR。

pub mod images;
pub mod layout;
pub mod tables;
pub mod text;

use crate::error::DocError;
use crate::ir::*;
use lopdf::Document as PdfDocument;

/// 单份 PDF 的页数上界,防御构造的超大文档。
const MAX_PAGES: usize = 5_000;

/// 一页解析出的中间产物。页眉页脚需要跨页比对,所以先全量收集再统一成块。
struct PageWork {
    index: usize,
    height: f64,
    lines: Vec<layout::Line>,
    fragments: Vec<text::Fragment>,
    rules: Vec<text::Line>,
}

/// PDF 解析产物。图像字节单独带出 —— 动作层要拿它跑 OCR 或落盘,而引擎层
/// 不碰文件系统也不联网。
pub struct ParsedPdf {
    pub document: Document,
    pub media: Vec<(String, Vec<u8>)>,
}

pub fn parse(
    bytes: &[u8],
    source: SourceInfo,
    limit: usize,
    collect_images: bool,
) -> Result<ParsedPdf, DocError> {
    let pdf = PdfDocument::load_mem(bytes).map_err(|e| match e {
        lopdf::Error::Decryption(_) => DocError::Encrypted,
        other => DocError::container("pdf", other.to_string()),
    })?;
    if pdf.is_encrypted() {
        return Err(DocError::Encrypted);
    }

    let page_ids = pdf.get_pages();
    if page_ids.len() > MAX_PAGES {
        return Err(DocError::LimitExceeded {
            message: format!("pdf has {} pages, exceeding {MAX_PAGES}", page_ids.len()),
        });
    }

    let mut doc = Document::new(source);
    doc.meta.page_count = page_ids.len();
    read_info(&pdf, &mut doc.meta);

    // 先把每页的行算出来 —— 页眉页脚要跨页比对才能识别。
    let mut per_page: Vec<PageWork> = Vec::new();
    let mut scanned_pages = Vec::new();
    let mut image_warnings: Vec<String> = Vec::new();
    let mut media: Vec<(String, Vec<u8>)> = Vec::new();

    for (index, (_, page_id)) in page_ids.iter().enumerate() {
        let (width, height, rotation) = page_geometry(&pdf, *page_id);
        let fonts = pdf.get_page_fonts(*page_id).unwrap_or_default();
        let content = match text::walk_page(&pdf, *page_id, &fonts) {
            Ok(content) => content,
            Err(e) => {
                // 单页解析失败不该毁掉整份文档,记录后继续。
                doc.diagnostics
                    .warn(format!("第 {} 页内容流解析失败,已跳过: {e}", index + 1));
                Default::default()
            }
        };

        let total = content.decoded_chars + content.undecodable;
        let coverage = if total == 0 {
            0.0
        } else {
            content.decoded_chars as f64 / total as f64
        };
        if content.undecodable > 0 {
            doc.diagnostics.warn(format!(
                "第 {} 页有 {} 字节无法解码为 Unicode(字体缺 ToUnicode/编码信息)",
                index + 1,
                content.undecodable
            ));
        }
        // 只有「一个字都解不出」才算没有文本层。用「字符数少于 N」当判据会把
        // 扉页、章节分隔页、短中文页误报成扫描件 —— 有文本却说没有,比漏报更糟。
        if content.decoded_chars == 0 {
            scanned_pages.push(index + 1);
        }

        let lines = layout::cluster_lines(&content.fragments);

        // 图像 XObject:扫描页 OCR 的输入源。整页图一律取字节(OCR 要用),
        // 其余图只在调用方明确要求时才取,避免把无关插图全读进内存。
        let extracted = images::extract_page_images(
            &pdf,
            *page_id,
            &content.placements,
            &mut image_warnings,
            index + 1,
        )?;
        for image in extracted {
            let is_scan = images::is_page_scan(image.placement.as_ref(), width, height);
            let id = format!("i{}", doc.images.len() + 1);
            if is_scan || collect_images {
                media.push((id.clone(), image.bytes));
            }
            let bbox = image.placement.as_ref().map(|p| {
                // 翻转到左上原点
                BBox::new(p.x0, height - p.y1, p.x1, height - p.y0)
            });
            doc.images.push(Image {
                id,
                page: index,
                bbox,
                format: image.format,
                width: image.width,
                height: image.height,
                path: None,
                ocr_text: None,
                is_page_scan: is_scan,
            });
        }

        per_page.push(PageWork {
            index,
            height,
            lines,
            fragments: content.fragments,
            rules: content.lines,
        });
        doc.pages.push(Page {
            index,
            width: Some(width),
            height: Some(height),
            unit: "pt".into(),
            rotation,
            text_coverage: coverage,
            source: Source::TextLayer,
            name: None,
        });
    }

    for warning in image_warnings {
        doc.diagnostics.warn(warning);
    }
    if !scanned_pages.is_empty() {
        // 显式点名,不假装这些页是空白的。有整页扫描图的页是 OCR 的可行目标;
        // 连图都没有的空白页要分开说,免得调用方以为开了 OCR 就能出内容。
        let with_image: Vec<usize> = scanned_pages
            .iter()
            .copied()
            .filter(|page| {
                doc.images
                    .iter()
                    .any(|img| img.page + 1 == *page && img.is_page_scan)
            })
            .collect();
        let join = |pages: &[usize]| {
            pages
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join("、")
        };
        if !with_image.is_empty() {
            doc.diagnostics.warn(format!(
                "第 {} 页没有文本层但有整页图像,可交由 OCR 识别",
                join(&with_image)
            ));
        }
        let blank: Vec<usize> = scanned_pages
            .iter()
            .copied()
            .filter(|p| !with_image.contains(p))
            .collect();
        if !blank.is_empty() {
            doc.diagnostics.warn(format!(
                "第 {} 页既无文本层也无可用图像,OCR 也无从下手",
                join(&blank)
            ));
        }
    }

    let all_lines: Vec<(Vec<layout::Line>, f64)> = per_page
        .iter()
        .map(|work| (work.lines.clone(), work.height))
        .collect();
    let running_heads = layout::running_head_texts(&all_lines);
    let flat: Vec<layout::Line> = all_lines
        .iter()
        .flat_map(|(lines, _)| lines.clone())
        .collect();
    let body = layout::body_font_size(&flat);

    let mut order = 0usize;
    let mut block_seq = 0usize;
    let mut table_seq = 0usize;
    for work in &per_page {
        let (index, height) = (work.index, work.height);
        let found = tables::detect(&work.rules, &work.fragments, index, height, &mut table_seq);
        // 落在表格框内的行不再单独成段,否则同一内容会出现两次
        let table_boxes: Vec<BBox> = found.iter().filter_map(|t| t.bbox).collect();
        let outside: Vec<layout::Line> = work
            .lines
            .iter()
            .filter(|line| {
                let y_top = height - line.y - line.font_size;
                !table_boxes.iter().any(|b| {
                    y_top >= b.y0 - 3.0
                        && y_top <= b.y1 + 3.0
                        && line.x0 >= b.x0 - 3.0
                        && line.x1 <= b.x1 + 3.0
                })
            })
            .cloned()
            .collect();

        let blocks = layout::lines_to_blocks(
            &outside,
            index,
            height,
            body,
            &running_heads,
            order,
            &mut block_seq,
        );
        order += blocks.len();
        doc.blocks.extend(blocks);

        for table in found {
            block_seq += 1;
            let placeholder = Block::text_layer(
                format!("b{block_seq}"),
                BlockType::Table,
                index,
                order,
                String::new(),
            )
            .with_ref(&table.id);
            doc.blocks.push(placeholder);
            order += 1;
            doc.tables.push(table);
        }
    }

    doc.rebuild_outline();
    doc.apply_limit(limit);
    Ok(ParsedPdf {
        document: doc,
        media,
    })
}

fn page_geometry(pdf: &PdfDocument, page_id: (u32, u16)) -> (f64, f64, i32) {
    // MediaBox 可继承自父节点,lopdf 的 get_page_* 不代劳,这里自己向上找。
    let mut media_box = None;
    let mut rotation = 0i32;
    let mut current = Some(page_id);
    let mut depth = 0;
    while let Some(id) = current {
        depth += 1;
        if depth > 32 {
            break; // 防御环形 Parent 引用
        }
        let Ok(dict) = pdf.get_dictionary(id) else {
            break;
        };
        if media_box.is_none() {
            if let Ok(array) = dict.get(b"MediaBox").and_then(|o| o.as_array()) {
                // MediaBox 常写成整数([0 0 595 842]),as_f32 不接受 Integer,
                // 两种都要认 —— 否则会静默退回 A4 缺省值。
                let values: Vec<f64> = array.iter().filter_map(object_as_f64).collect();
                if values.len() == 4 {
                    media_box = Some((values[2] - values[0], values[3] - values[1]));
                }
            }
        }
        if rotation == 0 {
            if let Ok(value) = dict.get(b"Rotate").and_then(|o| o.as_i64()) {
                rotation = value as i32;
            }
        }
        current = dict.get(b"Parent").ok().and_then(|o| o.as_reference().ok());
    }
    // 缺省 A4(595.3 × 841.9pt)
    let (width, height) = media_box.unwrap_or((595.3, 841.9));
    (width, height, rotation)
}

fn object_as_f64(object: &lopdf::Object) -> Option<f64> {
    match object {
        lopdf::Object::Integer(i) => Some(*i as f64),
        lopdf::Object::Real(r) => Some(*r as f64),
        _ => None,
    }
}

fn read_info(pdf: &PdfDocument, meta: &mut DocMeta) {
    let Some(info_id) = pdf
        .trailer
        .get(b"Info")
        .ok()
        .and_then(|o| o.as_reference().ok())
    else {
        return;
    };
    let Ok(dict) = pdf.get_dictionary(info_id) else {
        return;
    };
    let text = |key: &[u8]| {
        dict.get(key)
            .ok()
            .and_then(|o| o.as_str().ok())
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .filter(|s| !s.is_empty())
    };
    meta.title = text(b"Title");
    meta.author = text(b"Author");
    meta.producer = text(b"Producer");
    meta.created = text(b"CreationDate");
    meta.modified = text(b"ModDate");
}
