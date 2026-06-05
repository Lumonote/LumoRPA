//! Integration coverage for the `docx.*` action family (批次B).
//!
//! docx is a LOCAL family (no network) so these are *real* behavioral tests:
//! build a minimal valid .docx (zip of OOXML) in a tempdir, extract its text,
//! fill `{{placeholders}}`, and assert the round-trip. Gating + validation are
//! exercised too. The .docx fixture is assembled with the `zip` dev-dependency
//! (no Word needed).

mod common;
use common::{fs_caps, ok_with, run, run_with};
use serde_json::json;
use std::io::Write;
use std::path::Path;

/// Write a minimal but structurally valid .docx whose body is `document_xml`
/// (the `<w:body>` inner content). Includes the `[Content_Types].xml` Word needs
/// so the file is a real OOXML package, not just a bag of bytes.
fn write_docx(path: &Path, body_inner: &str) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts: zip::write::FileOptions<()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    zip.start_file("[Content_Types].xml", opts).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#,
    )
    .unwrap();

    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body>{body_inner}</w:body>
</w:document>"#
    );
    zip.start_file("word/document.xml", opts).unwrap();
    zip.write_all(document.as_bytes()).unwrap();
    zip.finish().unwrap();
}

/// One paragraph holding `text` in a single run.
fn para(text: &str) -> String {
    format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
}

#[tokio::test]
async fn read_text_extracts_paragraphs_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("doc.docx");
    let caps = fs_caps(dir.path());
    write_docx(
        &path,
        &format!("{}{}{}", para("First line"), para("Second line"), para("第三行")),
    );

    let out = ok_with("docx.read_text", json!({ "path": path }), caps).await;
    assert_eq!(
        out["paragraphs"],
        json!(["First line", "Second line", "第三行"])
    );
    assert_eq!(out["text"], json!("First line\nSecond line\n第三行"));
}

#[tokio::test]
async fn read_text_concatenates_split_runs_within_a_paragraph() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("split.docx");
    let caps = fs_caps(dir.path());
    // A paragraph whose text is split across two runs (as Word often does).
    write_docx(
        &path,
        "<w:p><w:r><w:t>Hello </w:t></w:r><w:r><w:t>World</w:t></w:r></w:p>",
    );
    let out = ok_with("docx.read_text", json!({ "path": path }), caps).await;
    assert_eq!(out["paragraphs"], json!(["Hello World"]));
}

#[tokio::test]
async fn replace_placeholders_fills_template_then_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let template = dir.path().join("template.docx");
    let out_path = dir.path().join("filled.docx");
    let caps = fs_caps(dir.path());
    write_docx(
        &template,
        &format!(
            "{}{}",
            para("Dear {{name}},"),
            para("Your invoice total is {{amount}}.")
        ),
    );

    let res = ok_with(
        "docx.replace_placeholders",
        json!({
            "template": template,
            "out": out_path,
            "values": { "name": "Alice", "amount": 1280 }
        }),
        caps.clone(),
    )
    .await;
    assert_eq!(res["replaced"], json!(2), "both placeholders matched");
    assert!(out_path.exists(), "output .docx written");

    // Read the filled doc back: placeholders are gone, values are present.
    let read = ok_with("docx.read_text", json!({ "path": out_path }), caps).await;
    let text = read["text"].as_str().unwrap();
    assert!(text.contains("Dear Alice,"), "got: {text}");
    assert!(
        text.contains("Your invoice total is 1280."),
        "non-string value stringified, got: {text}"
    );
    assert!(!text.contains("{{"), "no leftover placeholders, got: {text}");
}

#[tokio::test]
async fn replace_placeholders_escapes_xml_special_chars() {
    let dir = tempfile::tempdir().unwrap();
    let template = dir.path().join("t.docx");
    let out_path = dir.path().join("o.docx");
    let caps = fs_caps(dir.path());
    write_docx(&template, &para("Owner: {{who}}"));

    ok_with(
        "docx.replace_placeholders",
        json!({
            "template": template,
            "out": out_path,
            "values": { "who": "Tom & <Jerry>" }
        }),
        caps.clone(),
    )
    .await;
    // The value contains XML-special chars; if not escaped the package would be
    // malformed and read_text would fail/mangle. Round-trip proves escaping.
    let read = ok_with("docx.read_text", json!({ "path": out_path }), caps).await;
    assert_eq!(read["paragraphs"], json!(["Owner: Tom & <Jerry>"]));
}

#[tokio::test]
async fn read_text_denies_ungranted_path() {
    let err = run("docx.read_text", json!({ "path": "/nope/secret.docx" }))
        .await
        .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(err.contains("fs.read"), "got: {err}");
}

#[tokio::test]
async fn replace_placeholders_denies_ungranted_output() {
    // Template readable, output dir not writable → fs.write gate rejects.
    let dir = tempfile::tempdir().unwrap();
    let template = dir.path().join("t.docx");
    write_docx(&template, &para("x"));
    // Grant read only over the tempdir, no write.
    let caps = common::Capabilities {
        fs_read: vec![format!("{}/**", dir.path().display())],
        ..Default::default()
    };
    let err = run_with(
        "docx.replace_placeholders",
        json!({ "template": template, "out": "/nope/out.docx", "values": {} }),
        caps,
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(err.contains("fs.write"), "got: {err}");
}

#[tokio::test]
async fn read_text_rejects_missing_path() {
    let err = run("docx.read_text", json!({})).await.unwrap_err();
    assert!(err.contains("input invalid"), "got: {err}");
}
