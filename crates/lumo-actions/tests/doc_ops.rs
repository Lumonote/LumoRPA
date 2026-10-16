//! `doc.*` 动作族的集成测试:门禁、输出三态、上界与降级路径。
//!
//! 解析正确性由 `lumo-doc` 自己的语料测试覆盖,这里只盯**动作层契约**。

mod common;

use common::{run_with, Capabilities};
use serde_json::json;
use std::io::Write;
use tempfile::TempDir;
use zip::write::SimpleFileOptions;

fn write_docx(dir: &TempDir, name: &str, body: &str) -> String {
    let path = dir.path().join(name);
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <w:body>{body}</w:body>
</w:document>"#
    );
    let file = std::fs::File::create(&path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("word/document.xml", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(document.as_bytes()).unwrap();
    zip.finish().unwrap();
    path.display().to_string()
}

fn para(text: &str) -> String {
    format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
}

fn read_caps(dir: &TempDir) -> Capabilities {
    Capabilities {
        fs_read: vec![format!("{}/**", dir.path().display())],
        ..Default::default()
    }
}

#[tokio::test]
async fn parse_is_denied_without_fs_read_even_for_a_missing_file() {
    // 「先校验能力再碰文件」的既有约定:未授权路径即使不存在也要先失败,
    // 否则错误信息会泄露路径是否存在。
    let err = run_with(
        "doc.parse",
        json!({ "path": "/definitely/not/here.docx" }),
        Capabilities::default(),
    )
    .await
    .expect_err("no fs.read grant must be denied");
    assert!(err.contains("fs.read"), "got: {err}");
}

#[tokio::test]
async fn parse_returns_docir_inline_with_counts() {
    let dir = TempDir::new().unwrap();
    let body = format!(
        "{}<w:tbl><w:tr><w:tc>{}</w:tc><w:tc>{}</w:tc></w:tr></w:tbl>",
        r#"<w:p><w:pPr><w:outlineLvl w:val="0"/></w:pPr><w:r><w:t>报告</w:t></w:r></w:p>"#,
        para("项目"),
        para("金额")
    );
    let path = write_docx(&dir, "a.docx", &body);

    let out = run_with("doc.parse", json!({ "path": path }), read_caps(&dir))
        .await
        .expect("parse should succeed");

    assert_eq!(out["source"]["format"], "docx");
    assert_eq!(out["tables"].as_array().unwrap().len(), 1);
    assert_eq!(out["outline"][0]["title"], "报告");
    // 逐块 provenance 是「可审计」的基础,必须出现在输出里
    assert_eq!(out["blocks"][0]["source"], "text_layer");
    assert_eq!(out["blocks"][0]["confidence"], 1.0);
}

#[tokio::test]
async fn limit_truncates_and_reports_it() {
    let dir = TempDir::new().unwrap();
    let body = (0..10)
        .map(|i| para(&format!("段落{i}")))
        .collect::<String>();
    let path = write_docx(&dir, "many.docx", &body);

    let out = run_with(
        "doc.parse",
        json!({ "path": path, "limit": 3 }),
        read_caps(&dir),
    )
    .await
    .unwrap();

    assert_eq!(out["blocks"].as_array().unwrap().len(), 3);
    assert_eq!(out["diagnostics"]["truncated"], true);
}

#[tokio::test]
async fn zero_limit_is_rejected_before_any_work() {
    let dir = TempDir::new().unwrap();
    let path = write_docx(&dir, "a.docx", &para("x"));
    let err = run_with(
        "doc.parse",
        json!({ "path": path, "limit": 0 }),
        read_caps(&dir),
    )
    .await
    .expect_err("limit 0 must fail");
    assert!(err.contains("limit"), "got: {err}");
}

#[tokio::test]
async fn file_output_needs_fs_write_and_keeps_the_result_small() {
    let dir = TempDir::new().unwrap();
    let path = write_docx(&dir, "a.docx", &para("正文"));
    let out_path = dir.path().join("docir.json");

    // 只有 fs.read 时写盘必须被拒
    let denied = run_with(
        "doc.parse",
        json!({ "path": path, "output": "file", "out_path": out_path }),
        read_caps(&dir),
    )
    .await
    .expect_err("writing without fs.write must be denied");
    assert!(denied.contains("fs.write"), "got: {denied}");

    let caps = Capabilities {
        fs_read: vec![format!("{}/**", dir.path().display())],
        fs_write: vec![format!("{}/**", dir.path().display())],
        ..Default::default()
    };
    let out = run_with(
        "doc.parse",
        json!({ "path": path, "output": "file", "out_path": out_path }),
        caps,
    )
    .await
    .unwrap();

    // step result 里不该再有整份 DocIR —— 大文档正是靠这条避免撑爆上下文
    assert!(
        out.get("blocks").is_none(),
        "file 模式不该内联 blocks: {out}"
    );
    assert_eq!(out["counts"]["blocks"], 1);
    assert!(out_path.exists());
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&out_path).unwrap()).unwrap();
    assert_eq!(written["blocks"][0]["text"], "正文");
}

#[tokio::test]
async fn file_output_without_out_path_fails_fast() {
    let dir = TempDir::new().unwrap();
    let path = write_docx(&dir, "a.docx", &para("x"));
    let err = run_with(
        "doc.parse",
        json!({ "path": path, "output": "file" }),
        read_caps(&dir),
    )
    .await
    .expect_err("output: file without out_path must fail");
    assert!(err.contains("out_path"), "got: {err}");
}

#[tokio::test]
async fn image_extraction_requires_an_explicit_directory() {
    let dir = TempDir::new().unwrap();
    let path = write_docx(&dir, "a.docx", &para("x"));
    let err = run_with(
        "doc.parse",
        json!({ "path": path, "images": { "extract": true } }),
        read_caps(&dir),
    )
    .await
    .expect_err("extract without dir must fail");
    assert!(err.contains("images.dir"), "got: {err}");
}

#[tokio::test]
async fn pdf_now_parses_into_docir_with_page_attribution() {
    use lopdf::content::{Content, Operation};
    use lopdf::{dictionary, Document as Pdf, Object, Stream};

    let dir = TempDir::new().unwrap();
    let path = dir.path().join("a.pdf");

    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let font_id = pdf.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });
    let resources_id = pdf.add_object(dictionary! { "Font" => dictionary! { "F1" => font_id } });
    let content = Content {
        operations: vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec!["F1".into(), 12.into()]),
            Operation::new("Td", vec![72.into(), 700.into()]),
            Operation::new("Tj", vec![Object::string_literal("invoice total 100")]),
            Operation::new("ET", vec![]),
        ],
    };
    let content_id = pdf.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
    let page_id = pdf.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content_id,
        "Resources" => resources_id,
        "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
    });
    pdf.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1,
        }),
    );
    let catalog_id = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog_id);
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();
    std::fs::write(&path, &bytes).unwrap();

    let out = run_with(
        "doc.parse",
        json!({ "path": path.display().to_string() }),
        read_caps(&dir),
    )
    .await
    .expect("PDF should parse after M2");

    assert_eq!(out["source"]["format"], "pdf");
    assert_eq!(out["pages"][0]["width"], 595.0);
    // 这正是旧 pdf.extract_text 给不了的:页归属 + bbox
    let block = &out["blocks"][0];
    assert_eq!(block["page"], 0);
    assert!(block["bbox"].is_object(), "PDF 块必须带 bbox: {block}");
    assert!(block["text"].as_str().unwrap().contains("invoice total"));
}

#[tokio::test]
async fn images_still_point_at_the_alternative_path() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("scan.png");
    std::fs::write(&path, [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]).unwrap();

    let err = run_with(
        "doc.parse",
        json!({ "path": path.display().to_string() }),
        read_caps(&dir),
    )
    .await
    .expect_err("image OCR is M3, must fail explicitly");
    // 不静默返回空结果 —— 错误里要写清楚现在能用什么
    assert!(err.contains("image.ocr"), "got: {err}");
}

#[tokio::test]
async fn a_corrupt_pdf_fails_loudly_instead_of_returning_an_empty_document() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("broken.pdf");
    std::fs::write(&path, b"%PDF-1.7\nnot actually a pdf").unwrap();

    let err = run_with(
        "doc.parse",
        json!({ "path": path.display().to_string() }),
        read_caps(&dir),
    )
    .await
    .expect_err("corrupt PDF must not silently yield an empty doc");
    assert!(err.contains("doc.parse"), "got: {err}");
}

#[tokio::test]
async fn to_markdown_from_upstream_docir_needs_no_capability() {
    let dir = TempDir::new().unwrap();
    let path = write_docx(
        &dir,
        "a.docx",
        &format!(
            "{}{}",
            r#"<w:p><w:pPr><w:outlineLvl w:val="0"/></w:pPr><w:r><w:t>标题</w:t></w:r></w:p>"#,
            para("正文")
        ),
    );
    let doc = run_with("doc.parse", json!({ "path": path }), read_caps(&dir))
        .await
        .unwrap();

    // 纯数据变换:不该要任何 grant
    let out = run_with(
        "doc.to_markdown",
        json!({ "doc": doc }),
        Capabilities::default(),
    )
    .await
    .expect("upstream DocIR needs no capability");
    let markdown = out["markdown"].as_str().unwrap();
    assert!(markdown.contains("# 标题"), "md={markdown}");
    assert!(markdown.contains("正文"));
}

#[tokio::test]
async fn to_markdown_from_path_is_gated_and_rejects_ambiguous_input() {
    let dir = TempDir::new().unwrap();
    let path = write_docx(&dir, "a.docx", &para("正文"));

    let denied = run_with(
        "doc.to_markdown",
        json!({ "path": path }),
        Capabilities::default(),
    )
    .await
    .expect_err("path form must be gated");
    assert!(denied.contains("fs.read"), "got: {denied}");

    let both = run_with(
        "doc.to_markdown",
        json!({ "path": path, "doc": {} }),
        read_caps(&dir),
    )
    .await
    .expect_err("doc + path together is ambiguous");
    assert!(both.contains("只能给一个"), "got: {both}");

    let neither = run_with("doc.to_markdown", json!({}), Capabilities::default())
        .await
        .expect_err("neither doc nor path must fail");
    assert!(neither.contains("path"), "got: {neither}");
}
