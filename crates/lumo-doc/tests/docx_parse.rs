//! DOCX 解析的端到端测试:在内存里构造真实 .docx(zip + OOXML)后解析。
//!
//! 重点盯**旧实现做不到的事** —— `docx.read_text` 用 `split("</w:p>")` 抽
//! `<w:t>`,表格会被拍平成段落。这里的断言就是要钉死结构不丢。

use lumo_doc::{BlockType, ParseOptions, TableSource};
use std::io::{Cursor, Write};
use zip::write::SimpleFileOptions;

/// 用给定的 part 组装一个最小可解析的 .docx。
fn build_docx(parts: &[(&str, &str)]) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
        let options = SimpleFileOptions::default();
        for (name, content) in parts {
            zip.start_file(*name, options).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }
    buf
}

fn document(body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"
            xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
  <w:body>{body}</w:body>
</w:document>"#
    )
}

fn para(text: &str) -> String {
    format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
}

fn parse(parts: &[(&str, &str)]) -> lumo_doc::Document {
    let bytes = build_docx(parts);
    lumo_doc::parse_bytes(&bytes, "/tmp/t.docx", &ParseOptions::default())
        .expect("docx should parse")
        .document
}

#[test]
fn headings_come_from_outline_level_and_style_names() {
    let styles = r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
        <w:style w:styleId="af3"><w:name w:val="heading 2"/></w:style>
    </w:styles>"#;
    let body = format!(
        "{}{}{}",
        // 显式 outlineLvl(0 基)→ 1 级标题
        r#"<w:p><w:pPr><w:outlineLvl w:val="0"/></w:pPr><w:r><w:t>第一章</w:t></w:r></w:p>"#,
        // 本地化 styleId(af3)只能靠 styles.xml 的 name 映射认出是 2 级标题
        r#"<w:p><w:pPr><w:pStyle w:val="af3"/></w:pPr><w:r><w:t>第一节</w:t></w:r></w:p>"#,
        para("正文"),
    );
    let doc = parse(&[
        ("word/document.xml", &document(&body)),
        ("word/styles.xml", styles),
    ]);

    let headings: Vec<_> = doc
        .blocks
        .iter()
        .filter(|b| b.block_type == BlockType::Heading)
        .collect();
    assert_eq!(headings.len(), 2, "blocks={:?}", doc.blocks);
    assert_eq!((headings[0].text.as_str(), headings[0].level), ("第一章", Some(1)));
    assert_eq!((headings[1].text.as_str(), headings[1].level), ("第一节", Some(2)));
    // outline 应能直接导航
    assert_eq!(doc.outline.len(), 2);
}

#[test]
fn table_cells_do_not_leak_into_paragraph_blocks() {
    // 这正是旧 split 实现的坑:单元格文字也在 <w:t> 里,会被当成段落。
    let body = format!(
        "{}<w:tbl><w:tr><w:tc>{}</w:tc><w:tc>{}</w:tc></w:tr></w:tbl>",
        para("表格前的段落"),
        para("单元格A"),
        para("单元格B")
    );
    let doc = parse(&[("word/document.xml", &document(&body))]);

    let paragraphs: Vec<&str> = doc
        .blocks
        .iter()
        .filter(|b| b.block_type == BlockType::Paragraph)
        .map(|b| b.text.as_str())
        .collect();
    assert_eq!(paragraphs, vec!["表格前的段落"], "单元格内容不该变成段落块");

    assert_eq!(doc.tables.len(), 1);
    let table = &doc.tables[0];
    assert_eq!(table.source, TableSource::Markup);
    assert_eq!(table.cell_at(0, 0).unwrap().text, "单元格A");
    assert_eq!(table.cell_at(0, 1).unwrap().text, "单元格B");
    // 表格在阅读流里要有占位块指向实体
    let placeholder = doc
        .blocks
        .iter()
        .find(|b| b.block_type == BlockType::Table)
        .expect("表格需要占位块保持阅读顺序");
    assert_eq!(placeholder.ref_id.as_deref(), Some(table.id.as_str()));
}

#[test]
fn grid_span_and_vertical_merge_become_cell_spans() {
    // 布局:
    //   行0: [ 合并表头(横跨2列) ]
    //   行1: [ 纵向合并起点 ][ B1 ]
    //   行2: [   (延续)     ][ B2 ]
    let body = format!(
        r#"<w:tbl>
            <w:tr><w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr>{}</w:tc></w:tr>
            <w:tr>
              <w:tc><w:tcPr><w:vMerge w:val="restart"/></w:tcPr>{}</w:tc>
              <w:tc>{}</w:tc>
            </w:tr>
            <w:tr>
              <w:tc><w:tcPr><w:vMerge/></w:tcPr><w:p/></w:tc>
              <w:tc>{}</w:tc>
            </w:tr>
        </w:tbl>"#,
        para("合并表头"),
        para("纵向合并"),
        para("B1"),
        para("B2")
    );
    let doc = parse(&[("word/document.xml", &document(&body))]);
    let table = &doc.tables[0];

    assert_eq!(table.rows, 3);
    assert_eq!(table.cols, 2);

    let header = table.cell_at(0, 0).unwrap();
    assert_eq!(header.col_span, 2, "gridSpan 应转成 col_span");
    assert_eq!(header.text, "合并表头");

    let merged = table.cell_at(1, 0).unwrap();
    assert_eq!(merged.row_span, 2, "vMerge restart+continue 应累加成 row_span");
    assert_eq!(merged.text, "纵向合并");

    // 延续格不单独产出,否则下游会重复计数
    assert!(table.cell_at(2, 0).is_none(), "被合并覆盖的从属格不该出现");
    assert_eq!(table.cell_at(2, 1).unwrap().text, "B2");
}

#[test]
fn list_items_keep_their_indent_level() {
    let body = format!(
        "{}{}",
        r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>一级条目</w:t></w:r></w:p>"#,
        r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="1"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>二级条目</w:t></w:r></w:p>"#,
    );
    let doc = parse(&[("word/document.xml", &document(&body))]);
    let items: Vec<_> = doc
        .blocks
        .iter()
        .filter(|b| b.block_type == BlockType::ListItem)
        .collect();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].level, Some(1));
    assert_eq!(items[1].level, Some(2));
}

#[test]
fn tracked_deletions_are_dropped_with_a_warning() {
    let body = format!(
        "<w:p><w:r><w:t>保留</w:t></w:r><w:del><w:r><w:delText>被删除</w:delText></w:r></w:del></w:p>{}",
        para("下一段")
    );
    let doc = parse(&[("word/document.xml", &document(&body))]);
    let text = doc.plain_text();
    assert!(text.contains("保留"));
    assert!(!text.contains("被删除"), "删除线内容不该出现在正文里: {text}");
    assert!(
        doc.diagnostics.warnings.iter().any(|w| w.contains("修订")),
        "丢弃内容必须留痕: {:?}",
        doc.diagnostics.warnings
    );
}

#[test]
fn headers_and_footers_are_typed_and_excluded_from_body_text() {
    let doc = parse(&[
        ("word/document.xml", &document(&para("正文内容"))),
        ("word/header1.xml", &document(&para("这是页眉"))),
        ("word/footer1.xml", &document(&para("第 1 页"))),
    ]);

    assert!(doc.blocks.iter().any(|b| b.block_type == BlockType::Header));
    assert!(doc.blocks.iter().any(|b| b.block_type == BlockType::Footer));
    let text = doc.plain_text();
    assert!(text.contains("正文内容"));
    assert!(!text.contains("这是页眉"), "页眉不该混进正文: {text}");
}

#[test]
fn preserved_spaces_survive_between_runs() {
    // trim_text(true) 会把这里的空格吃掉,导致「第 1 条」粘成「第1条」
    let body = r#"<w:p><w:r><w:t>第</w:t></w:r><w:r><w:t xml:space="preserve"> 1 </w:t></w:r><w:r><w:t>条</w:t></w:r></w:p>"#;
    let doc = parse(&[("word/document.xml", &document(body))]);
    assert_eq!(doc.blocks[0].text, "第 1 条");
}

#[test]
fn core_properties_populate_metadata() {
    let core = r#"<?xml version="1.0"?>
    <cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
                       xmlns:dc="http://purl.org/dc/elements/1.1/"
                       xmlns:dcterms="http://purl.org/dc/terms/">
      <dc:title>季度报告</dc:title>
      <dc:creator>财务部</dc:creator>
    </cp:coreProperties>"#;
    let doc = parse(&[
        ("word/document.xml", &document(&para("x"))),
        ("docProps/core.xml", core),
    ]);
    assert_eq!(doc.meta.title.as_deref(), Some("季度报告"));
    assert_eq!(doc.meta.author.as_deref(), Some("财务部"));
}

#[test]
fn markdown_export_round_trips_structure() {
    let body = format!(
        "{}<w:tbl><w:tr><w:tc>{}</w:tc><w:tc>{}</w:tc></w:tr></w:tbl>",
        r#"<w:p><w:pPr><w:outlineLvl w:val="0"/></w:pPr><w:r><w:t>报告</w:t></w:r></w:p>"#,
        para("项目"),
        para("金额")
    );
    let doc = parse(&[("word/document.xml", &document(&body))]);
    let md = lumo_doc::to_markdown(&doc);
    assert!(md.contains("# 报告"), "md={md}");
    assert!(md.contains("| 项目 | 金额 |"), "md={md}");
}
