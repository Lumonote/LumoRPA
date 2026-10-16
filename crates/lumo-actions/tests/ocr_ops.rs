//! OCR 三档引擎的集成测试。
//!
//! 远程档用**真实的本地 HTTP 服务**验证全链路(而不是打桩 to_lines),这样
//! 网络门禁、请求构造、响应解析、错误透传都被真正跑到。

mod common;

use common::{run_with, Capabilities};
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;
use tempfile::TempDir;

/// 起一个只应答一次的极简 HTTP 服务,返回 (base_url, 收到的请求体)。
fn spawn_ocr_server(status: &'static str, body: &'static str) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        // 读请求头,拿到 Content-Length 后读完 body
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut length = 0usize;
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                break;
            }
            if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = value.trim().parse().unwrap_or(0);
            }
        }
        let mut payload = vec![0u8; length];
        use std::io::Read;
        let _ = reader.read_exact(&mut payload);
        let _ = tx.send(String::from_utf8_lossy(&payload).into_owned());

        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    });

    (format!("http://127.0.0.1:{port}/ocr"), rx)
}

fn png(dir: &TempDir) -> String {
    let path = dir.path().join("scan.png");
    std::fs::write(&path, [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]).unwrap();
    path.display().to_string()
}

fn caps(dir: &TempDir, hosts: &[&str]) -> Capabilities {
    Capabilities {
        fs_read: vec![format!("{}/**", dir.path().display())],
        network: hosts.iter().map(|h| h.to_string()).collect(),
        ..Default::default()
    }
}

#[tokio::test]
async fn remote_engine_round_trips_through_a_real_endpoint() {
    let (url, received) = spawn_ocr_server(
        "200 OK",
        r#"{"lines":[{"text":"合同总额 100000","confidence":0.96},{"text":"签订日期 2026-08-01","confidence":0.91}]}"#,
    );
    let dir = TempDir::new().unwrap();

    let out = run_with(
        "doc.ocr",
        json!({
            "image": png(&dir),
            "ocr": { "engine": "remote", "allow_cloud": true, "endpoint": url },
        }),
        caps(&dir, &["127.0.0.1"]),
    )
    .await
    .expect("remote OCR should succeed");

    assert!(out["text"].as_str().unwrap().contains("合同总额 100000"));
    assert_eq!(out["lines"].as_array().unwrap().len(), 2);
    assert_eq!(out["engine"], "ocrremote");
    // 远程识别属于内容外发,必须如实标注
    assert_eq!(out["left_machine"], true);

    // 请求体应带 base64 图像与语言列表
    let body = received.recv().unwrap();
    let sent: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(sent["image"].as_str().unwrap().len() > 4, "图像应 base64 编码后发送");
    assert!(sent["lang"].is_array());
}

#[tokio::test]
async fn remote_engine_is_blocked_without_allow_cloud_even_with_network_grant() {
    // 这是最关键的一条:光有 network 能力不足以让文档内容出网
    let dir = TempDir::new().unwrap();
    let err = run_with(
        "doc.ocr",
        json!({
            "image": png(&dir),
            "ocr": { "engine": "remote", "endpoint": "http://127.0.0.1:9/ocr" },
        }),
        caps(&dir, &["127.0.0.1"]),
    )
    .await
    .expect_err("allow_cloud 未开时必须拒绝");
    assert!(err.contains("allow_cloud"), "got: {err}");
}

#[tokio::test]
async fn remote_engine_still_honours_the_network_capability() {
    // allow_cloud 开了,但 host 未授权 —— 两道闸是「与」不是「或」
    let (url, _rx) = spawn_ocr_server("200 OK", r#"{"text":"x"}"#);
    let dir = TempDir::new().unwrap();
    let err = run_with(
        "doc.ocr",
        json!({
            "image": png(&dir),
            "ocr": { "engine": "remote", "allow_cloud": true, "endpoint": url },
        }),
        // 只给 fs.read,不给 network
        Capabilities {
            fs_read: vec![format!("{}/**", dir.path().display())],
            ..Default::default()
        },
    )
    .await
    .expect_err("未授权 host 必须被拒");
    assert!(err.contains("network"), "got: {err}");
}

#[tokio::test]
async fn remote_errors_are_surfaced_not_swallowed() {
    let (url, _rx) = spawn_ocr_server("500 Internal Server Error", r#"{"error":"model overloaded"}"#);
    let dir = TempDir::new().unwrap();
    let err = run_with(
        "doc.ocr",
        json!({
            "image": png(&dir),
            "ocr": { "engine": "remote", "allow_cloud": true, "endpoint": url },
        }),
        caps(&dir, &["127.0.0.1"]),
    )
    .await
    .expect_err("5xx 必须报错而不是返回空文本");
    assert!(err.contains("500"), "got: {err}");
}

#[tokio::test]
async fn native_engine_reports_why_it_is_unavailable() {
    // 本机没装模型:必须明确报错并指出替代路径,而不是静默返回空
    let dir = TempDir::new().unwrap();
    let err = run_with(
        "doc.ocr",
        json!({ "image": png(&dir), "ocr": { "engine": "native" } }),
        caps(&dir, &[]),
    )
    .await
    .expect_err("本地模型未安装时应报错");
    assert!(err.contains("本地 OCR 不可用"), "got: {err}");
}

#[tokio::test]
async fn ocr_denies_ungranted_image_paths_before_touching_them() {
    let err = run_with(
        "doc.ocr",
        json!({ "image": "/nowhere/secret.png", "ocr": { "engine": "native" } }),
        Capabilities::default(),
    )
    .await
    .expect_err("未授权路径必须先失败");
    assert!(err.contains("fs.read"), "got: {err}");
}

#[tokio::test]
async fn doc_parse_reports_when_a_scan_needs_ocr_but_no_engine_is_available() {
    // 有整页扫描图、又没有可用引擎:必须在 diagnostics 里说清楚,
    // 而不是安静地返回一份没有正文的 DocIR。
    use lopdf::content::{Content, Operation};
    use lopdf::{dictionary, Document as Pdf, Object, Stream};

    let dir = TempDir::new().unwrap();
    let path = dir.path().join("scan.pdf");
    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let image_id = pdf.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image",
            "Width" => 1700, "Height" => 2200, "Filter" => "DCTDecode",
        },
        b"\xFF\xD8\xFF\xE0fake\xFF\xD9".to_vec(),
    ));
    let resources_id = pdf.add_object(dictionary! {
        "XObject" => dictionary! { "Im0" => image_id },
    });
    let content_id = pdf.add_object(Stream::new(
        dictionary! {},
        Content {
            operations: vec![
                Operation::new("q", vec![]),
                Operation::new(
                    "cm",
                    vec![595.into(), 0.into(), 0.into(), 842.into(), 0.into(), 0.into()],
                ),
                Operation::new("Do", vec![Object::Name(b"Im0".to_vec())]),
                Operation::new("Q", vec![]),
            ],
        }
        .encode()
        .unwrap(),
    ));
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
        caps(&dir, &[]),
    )
    .await
    .expect("parse should still succeed");

    let warnings = out["diagnostics"]["warnings"].as_array().unwrap();
    let joined: String = warnings.iter().map(|w| w.as_str().unwrap()).collect();
    assert!(
        joined.contains("引擎不可用"),
        "需要 OCR 却无引擎时必须告警: {joined}"
    );
}
