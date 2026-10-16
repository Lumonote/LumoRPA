//! DOCX(OOXML)结构化解析。
//!
//! 与既有的 `docx.read_text`(字符串 `split("</w:p>")` + 抽 `<w:t>`)的本质区别:
//! 那条路径把**表格拍平成段落** —— 因为单元格文字最终也落在 `<w:t>` 里,split
//! 看不出结构。本模块改用 quick-xml 流式解析 + 显式栈式状态机,保留:
//!
//! * 标题层级(`w:outlineLvl`,或 `w:pStyle` 经 styles.xml 名称映射)
//! * 表格及**合并单元格**(`w:gridSpan` = 列跨度;`w:vMerge` = 行跨度)
//! * 列表项与缩进层级(`w:numPr/w:ilvl`)
//! * 图片引用(`a:blip@r:embed` → rels → `word/media/*`)
//! * 页眉页脚(独立 part,默认不进正文阅读流)
//!
//! 单元格状态用**栈**而非单值字段:表格可嵌套(单元格里再放表格),单值字段会让
//! 内层收尾时把外层的 gridSpan/vMerge 一起清掉。
//!
//! 修订标记按「接受修订」处理:`<w:ins>` 内容保留,`<w:del>` 内文本丢弃并告警 ——
//! 静默保留删除线文本会让下游读到本不该存在的内容。

use crate::error::DocError;
use crate::ir::*;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::collections::HashMap;
use std::io::{Cursor, Read};

const DOCUMENT: &str = "word/document.xml";
const STYLES: &str = "word/styles.xml";
const CORE_PROPS: &str = "docProps/core.xml";
const RELS: &str = "word/_rels/document.xml.rels";

/// zip 解压量上限 —— zip bomb 防护。1GB 对合法文档绰绰有余。
const MAX_UNCOMPRESSED: u64 = 1024 * 1024 * 1024;
/// 表格嵌套深度上限,防御构造的深层嵌套。
const MAX_TABLE_DEPTH: usize = 8;

type Archive<'a> = zip::ZipArchive<Cursor<&'a [u8]>>;

pub struct ParsedDocx {
    pub document: Document,
    /// (image id, 原始字节)。仅当 `collect_images` 开启;落盘由动作层在
    /// `fs.write` 门禁下完成 —— 引擎层不碰文件系统。
    pub media: Vec<(String, Vec<u8>)>,
}

pub fn parse(
    bytes: &[u8],
    source: SourceInfo,
    collect_images: bool,
) -> Result<ParsedDocx, DocError> {
    let mut zip =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| DocError::container("docx", e.to_string()))?;

    let mut total: u64 = 0;
    for index in 0..zip.len() {
        if let Ok(file) = zip.by_index(index) {
            total = total.saturating_add(file.size());
        }
    }
    if total > MAX_UNCOMPRESSED {
        return Err(DocError::LimitExceeded {
            message: format!("docx uncompressed size {total} exceeds {MAX_UNCOMPRESSED}"),
        });
    }

    let document_xml =
        read_text(&mut zip, DOCUMENT)?.ok_or_else(|| DocError::malformed(DOCUMENT, "missing part"))?;
    let styles = match read_text(&mut zip, STYLES)? {
        Some(xml) => parse_styles(&xml)?,
        None => HashMap::new(),
    };
    let rels = match read_text(&mut zip, RELS)? {
        Some(xml) => parse_rels(&xml)?,
        None => HashMap::new(),
    };

    let mut doc = Document::new(source);
    doc.meta = match read_text(&mut zip, CORE_PROPS)? {
        Some(xml) => parse_core_props(&xml)?,
        None => DocMeta::default(),
    };

    let mut parser = BodyParser::new(&styles, &rels);
    parser.run(&document_xml)?;
    let harvest = parser.finish();

    doc.blocks = harvest.blocks;
    doc.tables = harvest.tables;
    doc.images = harvest.images;
    for warning in harvest.warnings {
        doc.diagnostics.warn(warning);
    }

    // 页眉页脚是独立 part。顺序追加在正文之后并标类型,`plain_text()` 会跳过它们。
    let part_names: Vec<String> = zip
        .file_names()
        .filter(|name| name.starts_with("word/header") || name.starts_with("word/footer"))
        .map(|name| name.to_string())
        .collect();
    let mut order = doc.blocks.len();
    for name in part_names {
        let kind = if name.starts_with("word/header") {
            BlockType::Header
        } else {
            BlockType::Footer
        };
        let Some(xml) = read_text(&mut zip, &name)? else {
            continue;
        };
        let mut sub = BodyParser::new(&styles, &rels);
        sub.run(&xml)?;
        for mut block in sub.finish().blocks {
            if block.text.trim().is_empty() {
                continue;
            }
            block.block_type = kind;
            block.level = None;
            block.reading_order = order;
            block.id = format!("hf{order}");
            doc.blocks.push(block);
            order += 1;
        }
    }

    // DOCX 无固定分页(取决于渲染器),统一记 1 页,避免伪造不存在的页码。
    doc.meta.page_count = 1;
    doc.pages = vec![Page {
        index: 0,
        width: None,
        height: None,
        unit: "pt".into(),
        rotation: 0,
        text_coverage: 1.0,
        source: Source::TextLayer,
        name: None,
    }];
    doc.rebuild_outline();

    let mut media = Vec::new();
    if collect_images {
        for (image_id, target) in harvest.media_targets {
            let path = format!("word/{}", target.trim_start_matches("./"));
            if let Some(raw) = read_bytes(&mut zip, &path)? {
                media.push((image_id, raw));
            }
        }
    }

    Ok(ParsedDocx {
        document: doc,
        media,
    })
}

fn read_text(zip: &mut Archive<'_>, name: &str) -> Result<Option<String>, DocError> {
    Ok(read_bytes(zip, name)?.map(|raw| String::from_utf8_lossy(&raw).into_owned()))
}

fn read_bytes(zip: &mut Archive<'_>, name: &str) -> Result<Option<Vec<u8>>, DocError> {
    let mut file = match zip.by_name(name) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(DocError::container("docx", e.to_string())),
    };
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|e| DocError::malformed(name, e.to_string()))?;
    Ok(Some(buf))
}

/// styles.xml → `styleId -> 标题层级`。本地化模板里 styleId 可能是 `1` 或
/// `af3`,但 `<w:name w:val="heading 1"/>` 是稳定的,所以以 name 为准。
fn parse_styles(xml: &str) -> Result<HashMap<String, u8>, DocError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut map = HashMap::new();
    let mut current_id: Option<String> = None;
    loop {
        match reader
            .read_event()
            .map_err(|e| DocError::malformed(STYLES, e.to_string()))?
        {
            Event::Start(e) if e.name().as_ref() == b"w:style" => current_id = attr(&e, "w:styleId"),
            Event::Start(e) | Event::Empty(e) if e.name().as_ref() == b"w:name" => {
                if let (Some(id), Some(name)) = (current_id.as_ref(), attr(&e, "w:val")) {
                    if let Some(level) = heading_level_from_name(&name) {
                        map.insert(id.clone(), level);
                    }
                }
            }
            Event::End(e) if e.name().as_ref() == b"w:style" => current_id = None,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(map)
}

/// `"heading 1"` / `"Heading 1"` / `"标题 1"` → `Some(1)`
fn heading_level_from_name(name: &str) -> Option<u8> {
    let trimmed = name.trim();
    let lower = trimmed.to_ascii_lowercase();
    let rest = match lower.strip_prefix("heading") {
        Some(rest) => rest.to_string(),
        None => trimmed.strip_prefix("标题")?.to_string(),
    };
    rest.trim().parse::<u8>().ok().filter(|n| (1..=9).contains(n))
}

fn parse_rels(xml: &str) -> Result<HashMap<String, String>, DocError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut map = HashMap::new();
    loop {
        match reader
            .read_event()
            .map_err(|e| DocError::malformed(RELS, e.to_string()))?
        {
            Event::Start(e) | Event::Empty(e) if e.name().as_ref() == b"Relationship" => {
                if let (Some(id), Some(target)) = (attr(&e, "Id"), attr(&e, "Target")) {
                    map.insert(id, target);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(map)
}

fn parse_core_props(xml: &str) -> Result<DocMeta, DocError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut meta = DocMeta::default();
    let mut field: Option<String> = None;
    loop {
        match reader
            .read_event()
            .map_err(|e| DocError::malformed(CORE_PROPS, e.to_string()))?
        {
            Event::Start(e) => field = Some(String::from_utf8_lossy(e.name().as_ref()).into_owned()),
            Event::Text(e) => {
                let value = e
                    .unescape()
                    .map_err(|err| DocError::malformed(CORE_PROPS, err.to_string()))?
                    .into_owned();
                match field.as_deref() {
                    Some("dc:title") => meta.title = Some(value),
                    Some("dc:creator") => meta.author = Some(value),
                    Some("dcterms:created") => meta.created = Some(value),
                    Some("dcterms:modified") => meta.modified = Some(value),
                    _ => {}
                }
            }
            Event::End(_) => field = None,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(meta)
}

fn attr(e: &BytesStart, name: &str) -> Option<String> {
    e.try_get_attribute(name)
        .ok()
        .flatten()
        .and_then(|a| a.unescape_value().ok())
        .map(|v| v.into_owned())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VMerge {
    None,
    Restart,
    Continue,
}

#[derive(Debug)]
struct RawCell {
    grid_span: usize,
    v_merge: VMerge,
    text: String,
}

/// 单元格的收集状态。放在栈上,嵌套表格才不会互相覆盖。
#[derive(Debug)]
struct CellFrame {
    paragraphs: Vec<String>,
    grid_span: usize,
    v_merge: VMerge,
}

#[derive(Debug)]
struct TableFrame {
    rows: Vec<Vec<RawCell>>,
    current_row: Vec<RawCell>,
    header_rows: usize,
    row_is_header: bool,
}

struct Harvest {
    blocks: Vec<Block>,
    tables: Vec<Table>,
    images: Vec<Image>,
    warnings: Vec<String>,
    media_targets: Vec<(String, String)>,
}

struct BodyParser<'a> {
    styles: &'a HashMap<String, u8>,
    rels: &'a HashMap<String, String>,
    blocks: Vec<Block>,
    tables: Vec<Table>,
    images: Vec<Image>,
    warnings: Vec<String>,
    media_targets: Vec<(String, String)>,
    order: usize,
    para_text: String,
    para_style: Option<String>,
    para_outline: Option<u8>,
    para_list_level: Option<u8>,
    in_deleted_run: bool,
    in_field_code: bool,
    reported_revision: bool,
    table_stack: Vec<TableFrame>,
    cell_stack: Vec<CellFrame>,
}

impl<'a> BodyParser<'a> {
    fn new(styles: &'a HashMap<String, u8>, rels: &'a HashMap<String, String>) -> Self {
        Self {
            styles,
            rels,
            blocks: Vec::new(),
            tables: Vec::new(),
            images: Vec::new(),
            warnings: Vec::new(),
            media_targets: Vec::new(),
            order: 0,
            para_text: String::new(),
            para_style: None,
            para_outline: None,
            para_list_level: None,
            in_deleted_run: false,
            in_field_code: false,
            reported_revision: false,
            table_stack: Vec::new(),
            cell_stack: Vec::new(),
        }
    }

    fn finish(self) -> Harvest {
        Harvest {
            blocks: self.blocks,
            tables: self.tables,
            images: self.images,
            warnings: self.warnings,
            media_targets: self.media_targets,
        }
    }

    fn run(&mut self, xml: &str) -> Result<(), DocError> {
        let mut reader = Reader::from_str(xml);
        // trim_text(false):正文空格有意义(`<w:t xml:space="preserve"> </w:t>`),
        // 裁掉会把「第 1 条」粘成「第1条」。
        reader.config_mut().trim_text(false);
        loop {
            match reader
                .read_event()
                .map_err(|e| DocError::malformed(DOCUMENT, e.to_string()))?
            {
                Event::Start(e) | Event::Empty(e) => self.on_open(&e)?,
                Event::Text(e) => {
                    if self.in_deleted_run || self.in_field_code {
                        continue;
                    }
                    let text = e
                        .unescape()
                        .map_err(|err| DocError::malformed(DOCUMENT, err.to_string()))?;
                    self.para_text.push_str(&text);
                }
                Event::End(e) => self.on_close(e.name().as_ref()),
                Event::Eof => break,
                _ => {}
            }
        }
        Ok(())
    }

    fn on_open(&mut self, e: &BytesStart) -> Result<(), DocError> {
        match e.name().as_ref() {
            b"w:tbl" => {
                if self.table_stack.len() >= MAX_TABLE_DEPTH {
                    return Err(DocError::LimitExceeded {
                        message: format!("table nesting exceeds {MAX_TABLE_DEPTH}"),
                    });
                }
                self.table_stack.push(TableFrame {
                    rows: Vec::new(),
                    current_row: Vec::new(),
                    header_rows: 0,
                    row_is_header: false,
                });
            }
            b"w:tr" => {
                if let Some(frame) = self.table_stack.last_mut() {
                    frame.current_row = Vec::new();
                    frame.row_is_header = false;
                }
            }
            b"w:tblHeader" => {
                if let Some(frame) = self.table_stack.last_mut() {
                    frame.row_is_header = true;
                }
            }
            b"w:tc" => self.cell_stack.push(CellFrame {
                paragraphs: Vec::new(),
                grid_span: 1,
                v_merge: VMerge::None,
            }),
            b"w:gridSpan" => {
                if let (Some(frame), Some(v)) = (
                    self.cell_stack.last_mut(),
                    attr(e, "w:val").and_then(|v| v.parse::<usize>().ok()),
                ) {
                    frame.grid_span = v.max(1);
                }
            }
            b"w:vMerge" => {
                // `w:val="restart"` = 合并起点;省略 val = 延续上一行。
                let merge = match attr(e, "w:val").as_deref() {
                    Some("restart") => VMerge::Restart,
                    _ => VMerge::Continue,
                };
                if let Some(frame) = self.cell_stack.last_mut() {
                    frame.v_merge = merge;
                }
            }
            b"w:pStyle" => self.para_style = attr(e, "w:val"),
            b"w:outlineLvl" => {
                self.para_outline = attr(e, "w:val")
                    .and_then(|v| v.parse::<u8>().ok())
                    .map(|v| v.saturating_add(1).min(9));
            }
            b"w:ilvl" => self.para_list_level = attr(e, "w:val").and_then(|v| v.parse::<u8>().ok()),
            // numPr 出现即表示是列表项;ilvl 可能缺省(= 0 级)。ilvl 若先到达
            // 就已经写进 para_list_level,这里用 get_or_insert 不覆盖它。
            b"w:numPr" => {
                self.para_list_level.get_or_insert(0);
            }
            b"w:tab" => self.para_text.push('\t'),
            b"w:br" | b"w:cr" => self.para_text.push('\n'),
            b"w:del" => {
                self.in_deleted_run = true;
                if !self.reported_revision {
                    self.reported_revision = true;
                    self.warnings
                        .push("文档含修订标记:已按「接受修订」处理(删除内容被丢弃)".to_string());
                }
            }
            b"w:instrText" => self.in_field_code = true,
            b"a:blip" => {
                if let Some(target) = attr(e, "r:embed").and_then(|id| self.rels.get(&id).cloned()) {
                    let id = format!("i{}", self.images.len() + 1);
                    let format = target.rsplit('.').next().unwrap_or("bin").to_ascii_lowercase();
                    self.images.push(Image {
                        id: id.clone(),
                        page: 0,
                        bbox: None,
                        format,
                        width: None,
                        height: None,
                        path: None,
                        ocr_text: None,
                        is_page_scan: false,
                    });
                    self.media_targets.push((id, target));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn on_close(&mut self, name: &[u8]) {
        match name {
            b"w:p" => self.flush_paragraph(),
            b"w:tc" => {
                if let Some(frame) = self.cell_stack.pop() {
                    let cell = RawCell {
                        grid_span: frame.grid_span,
                        v_merge: frame.v_merge,
                        text: frame.paragraphs.join("\n"),
                    };
                    if let Some(table) = self.table_stack.last_mut() {
                        table.current_row.push(cell);
                    }
                }
            }
            b"w:tr" => {
                if let Some(frame) = self.table_stack.last_mut() {
                    let row = std::mem::take(&mut frame.current_row);
                    if frame.row_is_header && frame.rows.len() == frame.header_rows {
                        frame.header_rows += 1;
                    }
                    frame.rows.push(row);
                }
            }
            b"w:tbl" => self.flush_table(),
            b"w:del" => self.in_deleted_run = false,
            b"w:instrText" => self.in_field_code = false,
            _ => {}
        }
    }

    /// 段落收尾:在单元格内则并入该单元格,否则产出一个块。
    fn flush_paragraph(&mut self) {
        let text = std::mem::take(&mut self.para_text);
        let style = self.para_style.take();
        let outline = self.para_outline.take();
        let list_level = self.para_list_level.take();

        if let Some(frame) = self.cell_stack.last_mut() {
            if !text.trim().is_empty() {
                frame.paragraphs.push(text.trim().to_string());
            }
            return;
        }
        if text.trim().is_empty() {
            return;
        }

        // 标题判定优先级:显式 outlineLvl > styles.xml 名称映射 > styleId 自身像
        // `Heading2` 这种约定名。
        let heading = outline
            .or_else(|| style.as_ref().and_then(|id| self.styles.get(id).copied()))
            .or_else(|| style.as_deref().and_then(heading_level_from_name));

        let (block_type, level) = match (heading, list_level) {
            (Some(level), _) => (BlockType::Heading, Some(level)),
            (None, Some(depth)) => (BlockType::ListItem, Some(depth.saturating_add(1).min(9))),
            (None, None) => (BlockType::Paragraph, None),
        };

        let mut block = Block::text_layer(
            format!("b{}", self.order + 1),
            block_type,
            0,
            self.order,
            text.trim(),
        );
        block.level = level;
        self.blocks.push(block);
        self.order += 1;
    }

    /// 表格收尾:把 RawCell 网格解析成带 row_span/col_span 的锚格集合。
    /// 被合并覆盖的从属格不产出,只把跨度累加到锚格 —— 下游不会重复计数。
    fn flush_table(&mut self) {
        let Some(frame) = self.table_stack.pop() else {
            return;
        };
        let id = format!("t{}", self.tables.len() + 1);
        let mut cells: Vec<Cell> = Vec::new();
        // grid 列 → 该列最近一个 vMerge 锚格在 `cells` 里的下标
        let mut anchor_at: HashMap<usize, usize> = HashMap::new();
        let mut cols = 0usize;

        for (row_index, row) in frame.rows.iter().enumerate() {
            let mut col = 0usize;
            for raw in row {
                match raw.v_merge {
                    VMerge::Continue => match anchor_at.get(&col) {
                        Some(&index) => cells[index].row_span += 1,
                        None => {
                            // 没有起点 = 结构异常。降级为普通格并告警,不丢内容。
                            self.warnings.push(format!(
                                "表格 {id} 第 {} 行第 {} 列的 vMerge 没有起点,已按普通单元格处理",
                                row_index + 1,
                                col + 1
                            ));
                            cells.push(new_cell(row_index, col, raw));
                        }
                    },
                    VMerge::Restart => {
                        cells.push(new_cell(row_index, col, raw));
                        anchor_at.insert(col, cells.len() - 1);
                    }
                    VMerge::None => {
                        cells.push(new_cell(row_index, col, raw));
                        anchor_at.remove(&col);
                    }
                }
                col += raw.grid_span;
            }
            cols = cols.max(col);
        }

        let table = Table {
            id: id.clone(),
            page: 0,
            bbox: None,
            rows: frame.rows.len(),
            cols,
            header_rows: frame.header_rows,
            cells,
            source: TableSource::Markup,
            confidence: 1.0,
            name: None,
        };

        // 嵌套表格:文本并进父单元格,同时仍作为独立表格产出(结构不丢)。
        if let Some(parent) = self.cell_stack.last_mut() {
            parent.paragraphs.push(table_preview(&table));
            self.warnings
                .push(format!("表格 {id} 是嵌套表格,已同时作为独立表格产出"));
        } else {
            let block = Block::text_layer(
                format!("b{}", self.order + 1),
                BlockType::Table,
                0,
                self.order,
                String::new(),
            )
            .with_ref(&id);
            self.blocks.push(block);
            self.order += 1;
        }
        self.tables.push(table);
    }
}

fn new_cell(row: usize, col: usize, raw: &RawCell) -> Cell {
    Cell {
        row,
        col,
        row_span: 1,
        col_span: raw.grid_span,
        text: raw.text.clone(),
        formula: None,
        value_type: None,
        bbox: None,
    }
}

fn table_preview(table: &Table) -> String {
    table
        .cells
        .iter()
        .map(|c| c.text.as_str())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
