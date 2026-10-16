//! DocIR —— 跨格式统一文档模型。
//!
//! 设计要点(详见 `.omc/plans/doc-parsing-skill.md`):
//!
//! * **逐块 provenance**:每个块带 [`Source`] 与 `confidence`。文本层块置信度
//!   1.0 可直接信任;OCR 块带实测值,低置信可接 `human.approve` 走人工复核。
//!   版面重建本质是启发式,与其隐藏不确定性,不如显式暴露给调用方。
//! * **坐标统一为左上原点 / pt**。PDF 原生是左下原点(解析时翻转),图片按 96
//!   DPI 折算。统一坐标系是下游做高亮、截图取证的前提。
//! * **`reading_order` 显式存储**,不依赖数组下标 —— 多栏版面下 blocks 可能按
//!   位置排序而阅读顺序另有其序。
//! * **表格独立于 blocks**,同时在 blocks 里留 `Table` 占位块保持阅读顺序连续。

use serde::{Deserialize, Serialize};

/// 内容的来源通路。序列化为 snake_case,进入动作输出后可被 flow 模板直接判别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// 文档自带文本层(PDF 内嵌文本 / OOXML / 单元格)——零成本且准确。
    TextLayer,
    /// 本地小 OCR 模型(可选 `ocr-native` 特性,离线)。
    OcrNative,
    /// 远程 OCR 接口(自建/第三方 HTTP 服务)。**内容会离开本机**。
    OcrRemote,
    /// 云端视觉模型。**内容会离开本机**,受 `allow_cloud` 与 `llm` 双重门禁。
    OcrVlm,
    /// 由版面启发式推导(如从字号推断的标题层级)。
    Layout,
}

impl Source {
    /// 该来源是否把文档内容送出了本机 —— 审计与 `diagnostics` 用。
    /// 远程 OCR 与云 VLM 都算,本地小模型不算。
    pub fn leaves_machine(self) -> bool {
        matches!(self, Source::OcrRemote | Source::OcrVlm)
    }

    /// 是否来自 OCR(与文本层/版面推断相区分)。
    pub fn is_ocr(self) -> bool {
        matches!(
            self,
            Source::OcrNative | Source::OcrRemote | Source::OcrVlm
        )
    }
}

/// 块类型。故意保持扁平枚举(而非嵌套树):阅读顺序是线性的,层级由
/// [`Block::level`] 表达,下游遍历成本最低。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockType {
    Heading,
    Paragraph,
    ListItem,
    /// 表格占位块 —— 实体在 [`Document::tables`],此处仅占阅读顺序的位置。
    Table,
    Figure,
    Caption,
    Header,
    Footer,
    Footnote,
}

/// 左上原点的矩形,单位 pt。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BBox {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl BBox {
    pub fn new(x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        Self { x0, y0, x1, y1 }
    }
    pub fn width(&self) -> f64 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> f64 {
        self.y1 - self.y0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub id: String,
    #[serde(rename = "type")]
    pub block_type: BlockType,
    /// 标题层级 1..=9;列表项复用为缩进深度。其余类型为 `None`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<u8>,
    /// 0 基页序。无分页概念的格式(XLSX 按 sheet)用 sheet 序号。
    pub page: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bbox: Option<BBox>,
    pub reading_order: usize,
    pub text: String,
    pub source: Source,
    pub confidence: f64,
    /// 指向 [`Document::tables`] / [`Document::images`] 的实体 id。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_id: Option<String>,
}

impl Block {
    /// 文本层块的便捷构造 —— 置信度恒为 1.0。
    pub fn text_layer(
        id: impl Into<String>,
        block_type: BlockType,
        page: usize,
        reading_order: usize,
        text: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            block_type,
            level: None,
            page,
            bbox: None,
            reading_order,
            text: text.into(),
            source: Source::TextLayer,
            confidence: 1.0,
            ref_id: None,
        }
    }

    pub fn with_level(mut self, level: u8) -> Self {
        self.level = Some(level);
        self
    }

    pub fn with_ref(mut self, ref_id: impl Into<String>) -> Self {
        self.ref_id = Some(ref_id.into());
        self
    }
}

/// 单元格。`row_span`/`col_span` 恒 >= 1;被合并覆盖的从属格**不出现**在
/// [`Table::cells`] 里(只保留左上角的锚格),避免下游重复计数。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cell {
    pub row: usize,
    pub col: usize,
    pub row_span: usize,
    pub col_span: usize,
    pub text: String,
    /// 原始公式(如 `=SUM(A1:A9)`)。审计场景常需要公式本身而非计算值。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    /// 值类型:`string` / `number` / `bool` / `date` / `error` / `empty`。
    /// 显式带出是为了避免下游把 Excel 日期序列号(如 45292)当普通数字。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bbox: Option<BBox>,
}

/// 表格结构的判定依据 —— 直接决定 `confidence` 的可信程度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TableSource {
    /// OOXML / 单元格模型给出的确定结构。
    Markup,
    /// PDF 内容流的表格线推断。
    RulingLines,
    /// 空白沟壑投影推断 —— 置信度低,建议人工复核。
    Whitespace,
    /// OCR 版面模型给出。
    Ocr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Table {
    pub id: String,
    pub page: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bbox: Option<BBox>,
    pub rows: usize,
    pub cols: usize,
    /// 表头行数(0 = 无表头)。
    pub header_rows: usize,
    pub cells: Vec<Cell>,
    pub source: TableSource,
    pub confidence: f64,
    /// 来源名(XLSX 的 sheet 名等)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Table {
    /// 按行列取锚格文本。被合并覆盖的从属位置返回 `None`。
    pub fn cell_at(&self, row: usize, col: usize) -> Option<&Cell> {
        self.cells.iter().find(|c| c.row == row && c.col == col)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Image {
    pub id: String,
    pub page: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bbox: Option<BBox>,
    pub format: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// 落盘路径 —— 仅当调用方开启 `images.extract`(需 `fs.write`)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// OCR 文本(若走过 OCR)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ocr_text: Option<String>,
    /// 是否判定为「整页扫描图」—— PDF 扫描件路由的判据。
    pub is_page_scan: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page {
    pub index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    pub unit: String,
    pub rotation: i32,
    /// 文本层字符覆盖率 0.0..=1.0 —— OCR 路由判据,也写进输出供调试。
    pub text_coverage: f64,
    pub source: Source,
    /// 该页名(XLSX 的 sheet 名)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutlineItem {
    pub title: String,
    pub level: u8,
    pub page: usize,
    pub block_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer: Option<String>,
    pub page_count: usize,
    pub encrypted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceInfo {
    pub path: String,
    pub format: String,
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// 解析过程的可观测性输出。**不静默丢信息**:不支持的编码、走过 OCR 的页、
/// 截断都在这里显式暴露。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Diagnostics {
    pub warnings: Vec<String>,
    /// 走过本地 OCR 的页序。
    pub ocr_pages: Vec<usize>,
    /// 走过云端 VLM 的页序 —— 这些页的内容离开过本机,供事后审计。
    pub cloud_ocr_pages: Vec<usize>,
    pub truncated: bool,
    pub elapsed_ms: u64,
}

impl Diagnostics {
    pub fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub source: SourceInfo,
    pub meta: DocMeta,
    pub pages: Vec<Page>,
    pub blocks: Vec<Block>,
    pub tables: Vec<Table>,
    pub images: Vec<Image>,
    pub outline: Vec<OutlineItem>,
    pub diagnostics: Diagnostics,
}

impl Document {
    pub fn new(source: SourceInfo) -> Self {
        Self {
            source,
            meta: DocMeta::default(),
            pages: Vec::new(),
            blocks: Vec::new(),
            tables: Vec::new(),
            images: Vec::new(),
            outline: Vec::new(),
            diagnostics: Diagnostics::default(),
        }
    }

    /// 从 heading 块重建导航目录。解析器填完 blocks 后调用。
    pub fn rebuild_outline(&mut self) {
        self.outline = self
            .blocks
            .iter()
            .filter(|b| b.block_type == BlockType::Heading)
            .map(|b| OutlineItem {
                title: b.text.clone(),
                level: b.level.unwrap_or(1),
                page: b.page,
                block_id: b.id.clone(),
            })
            .collect();
    }

    /// 按 `limit` 截断块数并置 `truncated`。契约要求集合动作有上界 —— 一份
    /// 200 页扫描件的 DocIR 可达数十 MB,全量进 step result 会撑爆模板上下文
    /// 与运行存储。
    pub fn apply_limit(&mut self, limit: usize) {
        if self.blocks.len() > limit {
            self.blocks.truncate(limit);
            self.diagnostics.truncated = true;
            self.diagnostics
                .warn(format!("blocks truncated to limit {limit}"));
            // 目录随之收敛到保留下来的块,避免指向已被裁掉的 block_id。
            let kept: std::collections::HashSet<&str> =
                self.blocks.iter().map(|b| b.id.as_str()).collect();
            self.outline.retain(|o| kept.contains(o.block_id.as_str()));
        }
    }

    /// 全文纯文本(按阅读顺序,页眉页脚除外)。
    pub fn plain_text(&self) -> String {
        let mut ordered: Vec<&Block> = self
            .blocks
            .iter()
            .filter(|b| !matches!(b.block_type, BlockType::Header | BlockType::Footer))
            .collect();
        ordered.sort_by_key(|b| b.reading_order);
        ordered
            .iter()
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Document {
        Document::new(SourceInfo {
            path: "/tmp/x.docx".into(),
            format: "docx".into(),
            bytes: 10,
            sha256: None,
        })
    }

    #[test]
    fn outline_is_rebuilt_from_heading_blocks_only() {
        let mut d = doc();
        d.blocks = vec![
            Block::text_layer("b1", BlockType::Heading, 0, 0, "第一章").with_level(1),
            Block::text_layer("b2", BlockType::Paragraph, 0, 1, "正文"),
            Block::text_layer("b3", BlockType::Heading, 1, 2, "第二章").with_level(1),
        ];
        d.rebuild_outline();
        assert_eq!(d.outline.len(), 2);
        assert_eq!(d.outline[1].title, "第二章");
        assert_eq!(d.outline[1].block_id, "b3");
    }

    #[test]
    fn limit_truncates_blocks_and_prunes_dangling_outline_entries() {
        let mut d = doc();
        d.blocks = (0..5)
            .map(|i| {
                Block::text_layer(format!("b{i}"), BlockType::Heading, 0, i, format!("H{i}"))
                    .with_level(1)
            })
            .collect();
        d.rebuild_outline();
        d.apply_limit(2);
        assert_eq!(d.blocks.len(), 2);
        assert!(d.diagnostics.truncated);
        // 目录不能指向被裁掉的块
        assert_eq!(d.outline.len(), 2);
        assert!(d.outline.iter().all(|o| o.block_id != "b4"));
    }

    #[test]
    fn plain_text_follows_reading_order_and_skips_running_heads() {
        let mut d = doc();
        d.blocks = vec![
            Block::text_layer("b1", BlockType::Paragraph, 0, 1, "第二句"),
            Block::text_layer("b2", BlockType::Header, 0, 99, "页眉"),
            Block::text_layer("b3", BlockType::Paragraph, 0, 0, "第一句"),
        ];
        assert_eq!(d.plain_text(), "第一句\n第二句");
    }

    #[test]
    fn only_the_vlm_path_is_reported_as_leaving_the_machine() {
        assert!(Source::OcrVlm.leaves_machine());
        assert!(!Source::OcrNative.leaves_machine());
        assert!(!Source::TextLayer.leaves_machine());
    }

    #[test]
    fn optional_fields_stay_out_of_the_json_when_absent() {
        let block = Block::text_layer("b1", BlockType::Paragraph, 0, 0, "x");
        let json = serde_json::to_string(&block).unwrap();
        assert!(!json.contains("level"), "None 字段不应出现在输出里: {json}");
        assert!(json.contains("\"source\":\"text_layer\""));
    }
}
