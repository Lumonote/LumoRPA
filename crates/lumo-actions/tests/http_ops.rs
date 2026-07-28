//! Integration coverage for `http.request` (P1-8). A local `wiremock` server
//! stands in for the network so the happy path is hermetic; the deny path needs
//! no server at all.

mod common;
use common::{ok_with, run, Capabilities};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn net(host: &str) -> Capabilities {
    Capabilities {
        network: vec![host.to_string()],
        ..Default::default()
    }
}

#[tokio::test]
async fn request_returns_status_and_parsed_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/hello"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-trace", "abc")
                .set_body_json(json!({"ok": true})),
        )
        .mount(&server)
        .await;

    let out = ok_with(
        "http.request",
        json!({"url": format!("{}/hello", server.uri())}),
        net("127.0.0.1"),
    )
    .await;
    assert_eq!(out["status"], json!(200));
    assert_eq!(
        out["json"],
        json!({"ok": true}),
        "JSON bodies are parsed into `json`"
    );
    assert_eq!(out["headers"]["x-trace"], json!("abc"));
}

#[tokio::test]
async fn request_exposes_raw_text_when_body_is_not_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/plain"))
        .respond_with(ResponseTemplate::new(200).set_body_string("just text"))
        .mount(&server)
        .await;

    let out = ok_with(
        "http.request",
        json!({"url": format!("{}/plain", server.uri())}),
        net("127.0.0.1"),
    )
    .await;
    assert_eq!(out["text"], json!("just text"));
    assert_eq!(
        out["json"],
        json!(null),
        "non-JSON bodies leave `json` null"
    );
}

#[tokio::test]
async fn request_is_denied_without_a_network_grant() {
    // No grant → rejected before any socket is opened, so this stays offline.
    let err = run("http.request", json!({"url": "https://example.com/"}))
        .await
        .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
}

#[tokio::test]
async fn request_applies_bearer_auth_header() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/secure"))
        .and(wiremock::matchers::header(
            "authorization",
            "Bearer tok-123",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;

    let out = ok_with(
        "http.request",
        json!({
            "url": format!("{}/secure", server.uri()),
            "auth": {"kind": "bearer", "token": "tok-123"}
        }),
        net("127.0.0.1"),
    )
    .await;
    assert_eq!(out["status"], json!(200));
}

#[tokio::test]
async fn request_applies_basic_auth_header() {
    let server = MockServer::start().await;
    // base64("alice:s3cret") == "YWxpY2U6czNjcmV0"
    Mock::given(method("GET"))
        .and(path("/basic"))
        .and(wiremock::matchers::header(
            "authorization",
            "Basic YWxpY2U6czNjcmV0",
        ))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let out = ok_with(
        "http.request",
        json!({
            "url": format!("{}/basic", server.uri()),
            "auth": {"kind": "basic", "user": "alice", "pass": "s3cret"}
        }),
        net("127.0.0.1"),
    )
    .await;
    assert_eq!(out["status"], json!(204));
}

#[tokio::test]
async fn request_without_auth_still_works() {
    // Back-compat: omitting `auth` behaves exactly as before.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/open"))
        .respond_with(ResponseTemplate::new(200).set_body_string("hi"))
        .mount(&server)
        .await;
    let out = ok_with(
        "http.request",
        json!({"url": format!("{}/open", server.uri())}),
        net("127.0.0.1"),
    )
    .await;
    assert_eq!(out["status"], json!(200));
    assert_eq!(out["text"], json!("hi"));
}

#[tokio::test]
async fn request_rejects_unknown_auth_kind() {
    // deny_unknown_fields / the tagged enum reject a bogus scheme at deserialize.
    let err = run(
        "http.request",
        json!({"url": "https://example.com/", "auth": {"kind": "digest", "token": "x"}}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("input invalid"), "got: {err}");
}

#[tokio::test]
async fn request_aggregates_repeated_response_headers_into_an_array() {
    // P2-3:多枚 Set-Cookie 不能只剩最后一个。向后兼容形状:单值头仍是
    // 字符串,只有重复头才聚合成数组。
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/cookies"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-single", "one")
                .append_header("set-cookie", "a=1; Path=/")
                .append_header("set-cookie", "b=2; Path=/"),
        )
        .mount(&server)
        .await;

    let out = ok_with(
        "http.request",
        json!({"url": format!("{}/cookies", server.uri())}),
        net("127.0.0.1"),
    )
    .await;
    assert_eq!(out["headers"]["x-single"], json!("one"), "单值头保持字符串");
    assert_eq!(
        out["headers"]["set-cookie"],
        json!(["a=1; Path=/", "b=2; Path=/"]),
        "重复头按到达顺序聚合成数组"
    );
}

#[tokio::test]
async fn oauth2_token_parses_access_token_from_client_credentials_grant() {
    use wiremock::matchers::{body_string_contains, header};
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        // Default `client_auth: basic` → creds in the Authorization header;
        // base64("cid:secret") == "Y2lkOnNlY3JldA==".
        .and(header("authorization", "Basic Y2lkOnNlY3JldA=="))
        .and(body_string_contains("grant_type=client_credentials"))
        .and(body_string_contains("scope=read"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "at-xyz",
            "token_type": "Bearer",
            "expires_in": 3600,
            "scope": "read"
        })))
        .mount(&server)
        .await;

    let out = ok_with(
        "http.oauth2_token",
        json!({
            "token_url": format!("{}/oauth/token", server.uri()),
            "client_id": "cid",
            "client_secret": "secret",
            "scope": "read"
        }),
        net("127.0.0.1"),
    )
    .await;
    assert_eq!(out["access_token"], json!("at-xyz"));
    assert_eq!(out["token_type"], json!("Bearer"));
    assert_eq!(out["expires_in"], json!(3600));
    assert_eq!(out["scope"], json!("read"));
    // 输出是白名单字段:不带 `raw`(可能含 refresh_token 等长寿命凭据,而步骤
    // 输出会持久化进运行历史)。
    assert_eq!(out.get("raw"), None, "raw must not be exposed/persisted");
}

#[tokio::test]
async fn oauth2_token_accepts_string_expires_in() {
    // 部分 provider 把 expires_in 返回成字符串 —— 归一化成数字。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "tok",
            "expires_in": "7200"
        })))
        .mount(&server)
        .await;

    let out = ok_with(
        "http.oauth2_token",
        json!({
            "token_url": format!("{}/oauth/token", server.uri()),
            "client_id": "cid",
            "client_secret": "sek"
        }),
        net("127.0.0.1"),
    )
    .await;
    assert_eq!(out["expires_in"], json!(7200));
}

#[tokio::test]
async fn oauth2_token_body_auth_puts_credentials_in_form() {
    use wiremock::matchers::body_string_contains;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(body_string_contains("client_id=cid"))
        .and(body_string_contains("client_secret=sek"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"access_token": "tok"})))
        .mount(&server)
        .await;

    let out = ok_with(
        "http.oauth2_token",
        json!({
            "token_url": format!("{}/oauth/token", server.uri()),
            "client_id": "cid",
            "client_secret": "sek",
            "client_auth": "body"
        }),
        net("127.0.0.1"),
    )
    .await;
    assert_eq!(out["access_token"], json!("tok"));
}

#[tokio::test]
async fn paginate_next_url_aggregates_items_across_pages() {
    let server = MockServer::start().await;
    // Page 1 points to /p?page=2 via a body field; page 2 has no `next`.
    Mock::given(method("GET"))
        .and(path("/p"))
        .and(wiremock::matchers::query_param_is_missing("page"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {"items": [1, 2]},
            "next": format!("{}/p?page=2", server.uri())
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/p"))
        .and(wiremock::matchers::query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {"items": [3, 4, 5]}
        })))
        .mount(&server)
        .await;

    let out = ok_with(
        "http.paginate",
        json!({
            "url": format!("{}/p", server.uri()),
            "items_path": "/data/items",
            "pagination": {"style": "next_url", "next_path": "/next"}
        }),
        net("127.0.0.1"),
    )
    .await;
    assert_eq!(out["items"], json!([1, 2, 3, 4, 5]));
    assert_eq!(out["pages"], json!(2));
    assert_eq!(out["truncated"], json!(false));
}

#[tokio::test]
async fn paginate_page_number_stops_on_empty_page() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/list"))
        .and(wiremock::matchers::query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([10, 11])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/list"))
        .and(wiremock::matchers::query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([12])))
        .mount(&server)
        .await;
    // Page 3 is empty → stop here (no truncation, the stream ended naturally).
    Mock::given(method("GET"))
        .and(path("/list"))
        .and(wiremock::matchers::query_param("page", "3"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    let out = ok_with(
        "http.paginate",
        json!({
            "url": format!("{}/list", server.uri()),
            "pagination": {"style": "page_number"}
        }),
        net("127.0.0.1"),
    )
    .await;
    assert_eq!(out["items"], json!([10, 11, 12]));
    assert_eq!(out["pages"], json!(3));
    assert_eq!(out["truncated"], json!(false));
}

#[tokio::test]
async fn paginate_errors_on_non_2xx_page() {
    // HIGH-2 回归:错误页(5xx)必须显式报错,绝不能被当作「分页自然结束」
    // 而静默返回部分数据。
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/p"))
        .and(wiremock::matchers::query_param_is_missing("page"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [1, 2],
            "next": format!("{}/p?page=2", server.uri())
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/p"))
        .and(wiremock::matchers::query_param("page", "2"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({"error": "boom"})))
        .mount(&server)
        .await;

    let err = common::run_with(
        "http.paginate",
        json!({
            "url": format!("{}/p", server.uri()),
            "items_path": "/items",
            "pagination": {"style": "next_url", "next_path": "/next"}
        }),
        net("127.0.0.1"),
    )
    .await
    .unwrap_err();
    assert!(err.contains("HTTP 500"), "got: {err}");
    assert!(err.contains("page 2"), "got: {err}");
}

#[tokio::test]
async fn paginate_next_url_attaches_initial_query_only_to_first_page() {
    // next 链接通常已自带完整 query;若把初始 query 再叠上去会产生重复参数。
    // 第二页的 mock 显式要求「没有 limit 参数」—— 若重复附加则匹配不到,
    // wiremock 兜底 404,在状态码检查下整个动作报错,测试即失败。
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/q"))
        .and(wiremock::matchers::query_param("limit", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [1],
            "next": format!("{}/q2?cursor=abc", server.uri())
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/q2"))
        .and(wiremock::matchers::query_param("cursor", "abc"))
        .and(wiremock::matchers::query_param_is_missing("limit"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"items": [2]})))
        .mount(&server)
        .await;

    let out = ok_with(
        "http.paginate",
        json!({
            "url": format!("{}/q", server.uri()),
            "query": {"limit": "2"},
            "items_path": "/items",
            "pagination": {"style": "next_url", "next_path": "/next"}
        }),
        net("127.0.0.1"),
    )
    .await;
    assert_eq!(out["items"], json!([1, 2]));
    assert_eq!(out["pages"], json!(2));
}

#[tokio::test]
async fn paginate_caps_pages_and_reports_truncated() {
    let server = MockServer::start().await;
    // Every page yields one item and always links to itself → unbounded but for
    // max_pages. The cap must stop the loop AND set truncated: true.
    Mock::given(method("GET"))
        .and(path("/loop"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [1],
            "next": format!("{}/loop", server.uri())
        })))
        .mount(&server)
        .await;

    let out = ok_with(
        "http.paginate",
        json!({
            "url": format!("{}/loop", server.uri()),
            "items_path": "/items",
            "pagination": {"style": "next_url", "next_path": "/next"},
            "max_pages": 2
        }),
        net("127.0.0.1"),
    )
    .await;
    assert_eq!(out["pages"], json!(2), "stopped at the cap");
    assert_eq!(out["items"], json!([1, 1]));
    assert_eq!(out["truncated"], json!(true), "cap hit ⇒ truncated");
}

#[tokio::test]
async fn paginate_caps_total_items_and_reports_truncated() {
    // P2-2:max_bytes 只是每页上限,聚合总量需 max_items 兜底。命中后停止
    // 翻页,与 max_pages 命中同语义 —— truncated: true 而不报错(数据在完整
    // 页边界上,可用),与单页超 max_bytes 报错区分开。
    let server = MockServer::start().await;
    // 每页 3 条且永远链到自己 —— 不设总量闸就会一直翻到 max_pages。
    Mock::given(method("GET"))
        .and(path("/big"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [1, 2, 3],
            "next": format!("{}/big", server.uri())
        })))
        .mount(&server)
        .await;

    let out = ok_with(
        "http.paginate",
        json!({
            "url": format!("{}/big", server.uri()),
            "items_path": "/items",
            "pagination": {"style": "next_url", "next_path": "/next"},
            "max_items": 4
        }),
        net("127.0.0.1"),
    )
    .await;
    // 不切割单页:第 2 页整页计入(共 6 条 ≥ 4),随后停止翻页。
    assert_eq!(out["pages"], json!(2), "第 2 页后命中总量闸即停");
    assert_eq!(out["items"], json!([1, 2, 3, 1, 2, 3]));
    assert_eq!(out["truncated"], json!(true), "max_items 命中 ⇒ truncated");
}

// ─── per-step proxy / mTLS(现有增强)─────────────────────────────────────────

/// network + fs.read(`dir/**`)双授权 —— mtls 测试要读 tempdir 下的 PEM。
fn net_fs(host: &str, dir: &std::path::Path) -> Capabilities {
    Capabilities {
        network: vec![host.to_string()],
        fs_read: vec![format!("{}/**", dir.display())],
        ..Default::default()
    }
}

#[tokio::test]
async fn request_routes_through_per_step_proxy() {
    // wiremock 充当 HTTP 正向代理:http 目标经代理时请求行用绝对 URI 打到代理,
    // hyper 侧 path 匹配不受影响。目标端口 1 无人监听 —— 若没走代理必然连接
    // 失败,拿到 200 即证明流量确实经过了代理。
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/via-proxy"))
        .respond_with(ResponseTemplate::new(200).set_body_string("proxied"))
        .mount(&server)
        .await;

    let out = ok_with(
        "http.request",
        json!({"url": "http://127.0.0.1:1/via-proxy", "proxy": server.uri()}),
        net("127.0.0.1"),
    )
    .await;
    assert_eq!(out["status"], json!(200));
    assert_eq!(out["text"], json!("proxied"));
}

#[tokio::test]
async fn request_rejects_ungranted_proxy_host() {
    // 代理 URL 本身过 SSRF 门禁:目标 host 已授权,但代理 host 未授权 ⇒ 拒绝,
    // 且在任何 socket 建立之前就挡下(离线可测)—— 门禁不因走代理放松。
    let err = common::run_with(
        "http.request",
        json!({"url": "http://127.0.0.1/x", "proxy": "http://10.99.99.99:3128"}),
        net("127.0.0.1"),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
}

#[tokio::test]
async fn request_rejects_invalid_proxy_url() {
    // 代理 host 已授权但 scheme 不被支持(非 http/https/socks5)⇒ 在客户端
    // 构造处显式报错,绝不静默直连。
    let err = common::run_with(
        "http.request",
        json!({"url": "http://127.0.0.1/x", "proxy": "ftp://127.0.0.1:21"}),
        net("127.0.0.1"),
    )
    .await
    .unwrap_err();
    assert!(err.contains("proxy"), "got: {err}");
}

#[tokio::test]
async fn bound_http_resource_decl_proxy_is_gated_too() {
    // decl 路径与 per-step 路径同一条 SSRF 红线:`http` resource 声明的 proxy
    // 也是网络目的地,未授权的代理 host 在共享客户端建起来之前就被拒。
    let err = common::run_bound(
        "run-decl-proxy-gate",
        &[("api", "kind: http\nproxy: http://10.88.88.88:3128\n")],
        Some("api"),
        "http.request",
        json!({"url": "http://127.0.0.1/x"}),
        net("127.0.0.1"),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
}

#[tokio::test]
async fn request_mtls_is_denied_without_fs_read_grant() {
    // 证书/私钥是本地敏感文件:无 fs.read 授权 ⇒ capability denied,文件内容
    // 一个字节都不读(走门禁的拒绝路径)。
    let err = common::run_with(
        "http.request",
        json!({
            "url": "http://127.0.0.1/x",
            "mtls": {"cert_path": "/etc/lumo-cert.pem", "key_path": "/etc/lumo-key.pem"}
        }),
        net("127.0.0.1"),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
}

#[tokio::test]
async fn request_mtls_errors_on_missing_cert_file() {
    // 路径已授权但文件不存在 ⇒ 读取处显式报错(带路径),不静默降级成无证书请求。
    let dir = tempfile::tempdir().unwrap();
    let err = common::run_with(
        "http.request",
        json!({
            "url": "http://127.0.0.1/x",
            "mtls": {
                "cert_path": dir.path().join("absent-cert.pem"),
                "key_path": dir.path().join("absent-key.pem")
            }
        }),
        net_fs("127.0.0.1", dir.path()),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("mtls") && err.contains("read cert"),
        "got: {err}"
    );
}

#[tokio::test]
async fn request_mtls_errors_on_invalid_pem() {
    // 文件在、能读,但不是合法 PEM ⇒ Identity::from_pem 处显式报错。
    let dir = tempfile::tempdir().unwrap();
    let cert = dir.path().join("junk-cert.pem");
    let key = dir.path().join("junk-key.pem");
    std::fs::write(&cert, "this is not a certificate").unwrap();
    std::fs::write(&key, "this is not a key").unwrap();
    let err = common::run_with(
        "http.request",
        json!({
            "url": "http://127.0.0.1/x",
            "mtls": {"cert_path": cert, "key_path": key}
        }),
        net_fs("127.0.0.1", dir.path()),
    )
    .await
    .unwrap_err();
    assert!(err.contains("invalid PEM"), "got: {err}");
}
