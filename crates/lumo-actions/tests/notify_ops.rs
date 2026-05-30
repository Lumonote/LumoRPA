//! Integration coverage for `notify.send` (S-class F-8). `wiremock` captures the
//! outgoing request so we assert provider body shape + signing fields offline.

mod common;
use common::{ok_with, run, Capabilities};
use serde_json::{json, Value};
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn net(host: &str) -> Capabilities {
    Capabilities {
        network: vec![host.to_string()],
        ..Default::default()
    }
}

#[tokio::test]
async fn dingtalk_text_body_shape() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/robot"))
        .and(body_json(
            json!({"msgtype": "text", "text": {"content": "hi"}}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"errcode": 0})))
        .mount(&server)
        .await;

    let out = ok_with(
        "notify.send",
        json!({"provider": "dingtalk", "url": format!("{}/robot", server.uri()), "text": "hi"}),
        net("127.0.0.1"),
    )
    .await;
    assert_eq!(out["ok"], json!(true));
    assert_eq!(out["status"], json!(200));
}

#[tokio::test]
async fn feishu_text_body_shape() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .and(body_json(
            json!({"msg_type": "text", "content": {"text": "hi"}}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"code": 0})))
        .mount(&server)
        .await;

    let out = ok_with(
        "notify.send",
        json!({"provider": "feishu", "url": format!("{}/hook", server.uri()), "text": "hi"}),
        net("127.0.0.1"),
    )
    .await;
    assert_eq!(out["ok"], json!(true));
}

#[tokio::test]
async fn dingtalk_secret_appends_timestamp_and_sign_to_url() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/robot"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"errcode": 0})))
        .mount(&server)
        .await;

    ok_with(
        "notify.send",
        json!({"provider": "dingtalk", "url": format!("{}/robot", server.uri()), "text": "hi", "secret": "S3CRET"}),
        net("127.0.0.1"),
    )
    .await;

    let reqs = server.received_requests().await.unwrap();
    let query = reqs[0].url.query().unwrap_or("");
    assert!(
        query.contains("timestamp="),
        "signed URL has timestamp: {query}"
    );
    assert!(query.contains("sign="), "signed URL has sign: {query}");
}

#[tokio::test]
async fn feishu_secret_adds_timestamp_and_sign_to_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"code": 0})))
        .mount(&server)
        .await;

    ok_with(
        "notify.send",
        json!({"provider": "feishu", "url": format!("{}/hook", server.uri()), "text": "hi", "secret": "S3CRET"}),
        net("127.0.0.1"),
    )
    .await;

    let reqs = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert!(
        body.get("timestamp").is_some(),
        "body has timestamp: {body}"
    );
    assert!(body.get("sign").is_some(), "body has sign: {body}");
}

#[tokio::test]
async fn provider_errcode_nonzero_fails_the_step() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/robot"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"errcode": 310000, "errmsg": "bad token"})),
        )
        .mount(&server)
        .await;

    let err = run(
        "notify.send",
        json!({"provider": "dingtalk", "url": format!("{}/robot", server.uri()), "text": "hi"}),
    )
    .await;
    // No network grant on `run`, so this would be denied — use ok_with path:
    let _ = err;
    let err2 = common::run_with(
        "notify.send",
        json!({"provider": "dingtalk", "url": format!("{}/robot", server.uri()), "text": "hi"}),
        net("127.0.0.1"),
    )
    .await
    .unwrap_err();
    assert!(err2.contains("failed"), "got: {err2}");
}

#[tokio::test]
async fn notify_denied_without_network_grant() {
    let err = run(
        "notify.send",
        json!({"provider": "webhook", "url": "https://example.com/h", "text": "hi"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
}

#[tokio::test]
async fn notify_blocks_redirect_to_ungranted_host() {
    // SSRF 网关:授权 host 302 跳到未授权内网(云元数据),必须在连接前被拒,
    // 与 http.download/upload 的逐跳重定向防护一致(复用 build_gated_client)。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/robot"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", "http://169.254.169.254/latest/meta-data/"),
        )
        .mount(&server)
        .await;

    let err = common::run_with(
        "notify.send",
        json!({"provider": "webhook", "url": format!("{}/robot", server.uri()), "text": "hi"}),
        net("127.0.0.1"),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("redirect") || err.contains("network capability"),
        "got: {err}"
    );
}

#[tokio::test]
async fn dingtalk_transport_error_does_not_leak_signed_url() {
    // 传输层 send 失败(连接拒绝)时,reqwest 的 Error::Display 会带上完整 URL;
    // dingtalk 带 secret 时该 URL 含 ?timestamp=...&sign=<HMAC>(派生自 secret、
    // 1 小时窗口内可重放的有效签名)。该错误串经 vm.rs 持久化进 step 记录并
    // tracing::warn 落日志,违反 design doc「secret 绝不进快照/日志」契约。
    let port = {
        // 绑定后立即释放 → 该端口无监听 → 连接被拒(确定性传输错误,非 redirect)。
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };

    let err = common::run_with(
        "notify.send",
        json!({
            "provider": "dingtalk",
            "url": format!("http://127.0.0.1:{port}/robot"),
            "text": "hi",
            "secret": "S3CRET",
            "timeout_ms": 2000
        }),
        net("127.0.0.1"),
    )
    .await
    .unwrap_err();

    assert!(
        !err.contains("sign=") && !err.contains("timestamp="),
        "transport error must not leak the signed URL query (sign/timestamp derived from secret): {err}"
    );
}

#[tokio::test]
async fn feishu_secret_with_non_object_payload_is_rejected() {
    // secret 存在但 payload 非 JSON object 时,timestamp/sign 无处可插。旧实现静默
    // 丢弃签名并发出未签名请求(飞书侧 401,本地无任何提示)。应显式报错而非静默
    // 安全降级。mock 200 确保「未修复时返回 Ok(发了未签名)」,使 expect_err 干净失败。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"code": 0})))
        .mount(&server)
        .await;

    let res = common::run_with(
        "notify.send",
        json!({
            "provider": "feishu",
            "url": format!("{}/hook", server.uri()),
            "payload": [1, 2, 3],
            "secret": "S3CRET"
        }),
        net("127.0.0.1"),
    )
    .await;

    let err = res.expect_err(
        "feishu + secret + non-object payload must be rejected, not silently sent unsigned",
    );
    assert!(
        err.contains("object") || err.contains("payload"),
        "error should explain the object-payload signing requirement: {err}"
    );
}
