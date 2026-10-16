//! OCR 的**中立契约**与结果合并 —— 不含任何引擎实现。
//!
//! 引擎(本地小模型 / 远程接口 / 云 VLM)都需要 IO(动态库、网络、AI provider),
//! 所以实现放在动作层 `lumo-actions/src/ocr/`;本模块只定义两边共享的数据形状
//! 与「把 OCR 结果并回 DocIR」的纯逻辑,这样合并规则可以脱离网络单测。
//!
//! 合并的两条硬规则:
//! 1. **OCR 块一律带真实来源与置信度**,绝不冒充 `TextLayer`。下游据此决定是否
//!    人工复核。
//! 2. **有文本层的页不被 OCR 覆盖**。OCR 只补空白,不推翻已有的准确文本。

use crate::ir::*;

/// OCR 识别出的一行。`bbox` 用左上原点、与 DocIR 一致的坐标系。
#[derive(Debug, Clone)]
pub struct OcrLine {
    pub text: String,
    pub bbox: Option<BBox>,
    pub confidence: f64,
}

/// 一页(或一张图)的识别结果。
#[derive(Debug, Clone)]
pub struct OcrPageResult {
    pub page: usize,
    pub lines: Vec<OcrLine>,
    pub source: Source,
}

impl OcrPageResult {
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// 把 OCR 结果并回 DocIR。返回实际写入的块数。
///
/// 只处理**没有文本层**的页(`text_coverage` 低于阈值);已有文本的页原样保留 ——
/// OCR 的准确率不可能高过文档自带的文本层。
pub fn apply_ocr(doc: &mut Document, results: &[OcrPageResult], coverage_threshold: f64) -> usize {
    let mut written = 0;
    let mut order = doc.blocks.iter().map(|b| b.reading_order + 1).max().unwrap_or(0);
    let mut seq = doc.blocks.len();

    for result in results {
        let has_text_layer = doc
            .pages
            .iter()
            .find(|p| p.index == result.page)
            .map(|p| p.text_coverage > coverage_threshold)
            .unwrap_or(false);
        if has_text_layer {
            doc.diagnostics.warn(format!(
                "第 {} 页已有文本层,跳过 OCR 结果以免覆盖更准确的内容",
                result.page + 1
            ));
            continue;
        }

        for line in &result.lines {
            if line.text.trim().is_empty() {
                continue;
            }
            seq += 1;
            let mut block = Block::text_layer(
                format!("ocr{seq}"),
                BlockType::Paragraph,
                result.page,
                order,
                line.text.trim(),
            );
            // 关键:如实标注来源与置信度,不冒充文本层
            block.source = result.source;
            block.confidence = line.confidence;
            block.bbox = line.bbox;
            doc.blocks.push(block);
            order += 1;
            written += 1;
        }

        // 该页现在有内容了,更新页来源便于下游识别
        if let Some(page) = doc.pages.iter_mut().find(|p| p.index == result.page) {
            page.source = result.source;
        }
        // 审计:走过网络的页要单独记,便于事后核查哪些内容出过本机
        if result.source.leaves_machine() {
            doc.diagnostics.cloud_ocr_pages.push(result.page + 1);
        } else {
            doc.diagnostics.ocr_pages.push(result.page + 1);
        }
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_with_pages(coverages: &[f64]) -> Document {
        let mut doc = Document::new(SourceInfo {
            path: "/tmp/a.pdf".into(),
            format: "pdf".into(),
            bytes: 0,
            sha256: None,
        });
        doc.pages = coverages
            .iter()
            .enumerate()
            .map(|(index, coverage)| Page {
                index,
                width: Some(595.0),
                height: Some(842.0),
                unit: "pt".into(),
                rotation: 0,
                text_coverage: *coverage,
                source: Source::TextLayer,
                name: None,
            })
            .collect();
        doc
    }

    fn result(page: usize, text: &str, source: Source) -> OcrPageResult {
        OcrPageResult {
            page,
            lines: vec![OcrLine {
                text: text.into(),
                bbox: Some(BBox::new(10.0, 10.0, 100.0, 30.0)),
                confidence: 0.93,
            }],
            source,
        }
    }

    #[test]
    fn ocr_blocks_declare_their_real_source_and_confidence() {
        let mut doc = doc_with_pages(&[0.0]);
        let written = apply_ocr(&mut doc, &[result(0, "扫描出来的文字", Source::OcrNative)], 0.05);
        assert_eq!(written, 1);
        let block = &doc.blocks[0];
        assert_eq!(block.source, Source::OcrNative);
        assert!((block.confidence - 0.93).abs() < 1e-9);
        assert!(block.bbox.is_some());
        // 本地识别不算内容外发
        assert_eq!(doc.diagnostics.ocr_pages, vec![1]);
        assert!(doc.diagnostics.cloud_ocr_pages.is_empty());
    }

    #[test]
    fn pages_that_already_have_text_are_never_overwritten() {
        // OCR 准确率不可能高过文本层 —— 有文本的页必须原样保留
        let mut doc = doc_with_pages(&[0.99]);
        let written = apply_ocr(&mut doc, &[result(0, "OCR 猜的内容", Source::OcrNative)], 0.05);
        assert_eq!(written, 0);
        assert!(doc.blocks.is_empty());
        assert!(doc
            .diagnostics
            .warnings
            .iter()
            .any(|w| w.contains("跳过 OCR")));
    }

    #[test]
    fn remote_and_vlm_paths_are_recorded_as_content_leaving_the_machine() {
        for source in [Source::OcrRemote, Source::OcrVlm] {
            let mut doc = doc_with_pages(&[0.0]);
            apply_ocr(&mut doc, &[result(0, "合同金额", source)], 0.05);
            assert_eq!(
                doc.diagnostics.cloud_ocr_pages,
                vec![1],
                "{source:?} 必须计入外发审计"
            );
            assert!(doc.diagnostics.ocr_pages.is_empty());
        }
    }

    #[test]
    fn empty_lines_are_dropped_rather_than_creating_blank_blocks() {
        let mut doc = doc_with_pages(&[0.0]);
        let mut r = result(0, "", Source::OcrNative);
        r.lines.push(OcrLine {
            text: "   ".into(),
            bbox: None,
            confidence: 0.5,
        });
        assert_eq!(apply_ocr(&mut doc, &[r], 0.05), 0);
        assert!(doc.blocks.is_empty());
    }

    #[test]
    fn ocr_blocks_continue_the_existing_reading_order() {
        let mut doc = doc_with_pages(&[0.0]);
        doc.blocks.push(Block::text_layer("b1", BlockType::Heading, 0, 7, "已有标题"));
        apply_ocr(&mut doc, &[result(0, "补充内容", Source::OcrNative)], 0.05);
        let ocr_block = doc.blocks.last().unwrap();
        assert_eq!(ocr_block.reading_order, 8, "阅读顺序应接在已有块之后");
    }
}
