//! A visible Chromium window per platform, with an application-owned login profile.
//! Browser code is fixed; page content and message bodies are always JSON data.
use super::acquisition_desktop::{self as desktop, Stage};
use chromiumoxide::{Browser, BrowserConfig, Page};
use futures::StreamExt;
use lumo_core::CancelToken;
use reqwest::Url;
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::LazyLock,
    time::Duration,
};
use tokio::{
    sync::Mutex,
    time::{sleep, timeout},
};

const PAGE: &str = include_str!("acquisition_page.js");
struct Window {
    browser: Browser,
    page: Page,
}
static WINDOWS: LazyLock<Mutex<HashMap<String, Window>>> = LazyLock::new(Mutex::default);

pub(super) async fn shutdown() {
    let windows = std::mem::take(&mut *WINDOWS.lock().await);
    for (_, mut window) in windows {
        let _ = timeout(Duration::from_secs(2), window.browser.close()).await;
    }
}

fn urls(platform: &str, keyword: &str) -> Result<(&'static str, Url), String> {
    let (home, search) = match platform {
        "xiaohongshu" => (
            "https://www.xiaohongshu.com",
            "https://www.xiaohongshu.com/search_result",
        ),
        "douyin" => ("https://www.douyin.com", "https://www.douyin.com/search/"),
        "weibo" => ("https://weibo.com", "https://s.weibo.com/weibo"),
        _ => return Err("未知平台".into()),
    };
    let mut url = Url::parse(search).map_err(|e| e.to_string())?;
    if platform == "douyin" {
        url.path_segments_mut()
            .map_err(|_| "搜索路径无效")?
            .pop_if_empty()
            .push(keyword);
    } else {
        url.query_pairs_mut()
            .append_pair(if platform == "weibo" { "q" } else { "keyword" }, keyword);
    }
    Ok((home, url))
}
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
pub(super) async fn tab(home: &Path, platform: &str) -> Result<Page, String> {
    urls(platform, "")?;
    let mut windows = WINDOWS.lock().await;
    if let Some(window) = windows.get(platform) {
        if window.page.evaluate_expression("1").await.is_ok() {
            return Ok(window.page.clone());
        }
    }
    if let Some(mut old) = windows.remove(platform) {
        let _ = old.browser.close().await;
    }
    let dir = home
        .join("browser-profiles")
        .join(format!("acquisition-{platform}"));
    std::fs::create_dir_all(&dir).map_err(error)?;
    let config = BrowserConfig::builder()
        .user_data_dir(&dir)
        .with_head()
        .window_size(1280, 900)
        .request_timeout(Duration::from_secs(12))
        .launch_timeout(Duration::from_secs(20))
        .build()
        .map_err(error)?;
    let (browser, mut handler) = Browser::launch(config)
        .await
        .map_err(|e| format!("启动 Chromium 失败，请确认已安装 Chrome 或 Chromium：{e}"))?;
    tokio::spawn(async move { while handler.next().await.is_some() {} });
    let page = browser.new_page("about:blank").await.map_err(error)?;
    windows.insert(
        platform.into(),
        Window {
            browser,
            page: page.clone(),
        },
    );
    Ok(page)
}
pub(super) async fn navigate(page: &Page, url: &str) -> Result<(), String> {
    timeout(Duration::from_secs(30), page.goto(url))
        .await
        .map_err(|_| "页面加载超时")?
        .map_err(error)?;
    Ok(())
}
pub(super) async fn login(home: &Path, platform: &str) -> Result<(), String> {
    let page = tab(home, platform).await?;
    navigate(&page, urls(platform, "")?.0).await?;
    page.bring_to_front().await.map_err(error)?;
    Ok(())
}
pub(super) async fn op<T: DeserializeOwned>(
    page: &Page,
    name: &str,
    args: &Value,
) -> Result<T, String> {
    page.evaluate_expression(format!("({PAGE})({}, {})", json!(name), args))
        .await
        .map_err(error)?
        .into_value()
        .map_err(error)
}
#[cfg(target_os = "macos")]
pub(super) async fn browser_pid(platform: &str, page: &Page) -> Result<u32, String> {
    let mut windows = WINDOWS.lock().await;
    let w = windows
        .get_mut(platform)
        .filter(|w| w.page.target_id() == page.target_id())
        .ok_or("平台窗口已关闭")?;
    w.browser
        .get_mut_child()
        .and_then(|c| c.as_mut_inner().id())
        .ok_or("无法确定获客浏览器进程".into())
}
pub(super) fn check(cancel: &CancelToken) -> Result<(), String> {
    if cancel.is_cancelled() {
        Err("任务已停止".into())
    } else {
        Ok(())
    }
}
pub(super) async fn wait(cancel: &CancelToken, duration: Duration) -> Result<(), String> {
    tokio::select! {_ = cancel.cancelled()=>Err("任务已停止".into()), _ = sleep(duration)=>Ok(())}
}
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(super) struct Candidate {
    pub platform_user_id: String,
    pub display_name: String,
    pub profile_url: String,
    pub source_url: String,
    pub evidence: String,
}
#[derive(Deserialize)]
struct Results {
    rows: Vec<Candidate>,
    links: Vec<String>,
    empty: bool,
}
pub(super) async fn search(
    home: &Path,
    platform: &str,
    keyword: &str,
    audience: &str,
    analyst: &super::acquisition_analysis::Analyst,
    desktop_vision: bool,
    cancel: &CancelToken,
) -> Result<Vec<Candidate>, String> {
    let page = tab(home, platform).await?;
    let url = urls(platform, keyword)?.1;
    navigate(&page, url.as_str()).await?;
    let args = json!({"platform":platform});
    let mut results = Vec::new();
    let mut links = Vec::new();
    let mut seen = HashSet::new();
    if desktop_vision {
        wait(cancel, Duration::from_millis(800)).await?;
        let first: Results = op(&page, "discover", &args).await?;
        results.extend(first.rows);
        if first.empty && results.is_empty() && first.links.is_empty() {
            return Ok(vec![]);
        }
        desktop::perform(analyst, &page, &args, Stage::Post, audience, cancel).await?;
        wait(cancel, Duration::from_millis(700)).await?;
        let detail: Results = op(&page, "discover", &args).await?;
        results.extend(detail.rows);
        if results.is_empty() {
            return Err("已打开帖子，但未识别到同时包含作者、原文和链接的线索。".into());
        }
        return Ok(results);
    }
    // Sample multiple viewports then visit a few result pages for author-attributed comments.
    for step in 0..5 {
        wait(cancel, Duration::from_millis(800)).await?;
        let batch: Results = op(&page, "discover", &args).await?;
        if batch.empty && results.is_empty() && batch.rows.is_empty() && batch.links.is_empty() {
            return Ok(vec![]);
        }
        results.extend(batch.rows);
        for link in batch.links {
            if seen.insert(link.clone()) {
                links.push(link);
            }
        }
        if step == 1 && !links.is_empty() {
            if let Some(model) = &analyst.vision_model {
                let png = lumo_actions::vision::screenshot_png(&page)
                    .await
                    .map_err(error)?;
                let preferred = tokio::select! { _=cancel.cancelled()=>return Err("任务已停止".into()), r=analyst.choose_posts(model,&png,&links,audience)=>r? };
                let remaining = std::mem::take(&mut links);
                links = preferred;
                for link in remaining {
                    if !links.contains(&link) {
                        links.push(link);
                    }
                }
            }
        }
        if step >= 1 {
            op::<bool>(&page, "scroll", &args).await?;
        }
    }
    for link in links.into_iter().take(3) {
        check(cancel)?;
        navigate(&page, &link).await?;
        wait(cancel, Duration::from_millis(900)).await?;
        results.extend(op::<Results>(&page, "discover", &args).await?.rows);
        op::<bool>(&page, "scroll", &args).await?;
        wait(cancel, Duration::from_millis(450)).await?;
        results.extend(op::<Results>(&page, "discover", &args).await?.rows);
    }
    let mut identities = HashSet::new();
    results.retain(|row| {
        identities.insert((
            row.platform_user_id.clone(),
            row.source_url.clone(),
            row.evidence.clone(),
        ))
    });
    if results.is_empty() {
        return Err(
            "未识别到同时包含作者、内容链接与原文的搜索结果。请检查浏览器登录状态和平台页面。"
                .into(),
        );
    }
    Ok(results)
}

async fn conversation(
    home: &Path,
    lead: &lumo_storage::AcquisitionLead,
    cancel: &CancelToken,
    analyst: Option<&super::acquisition_analysis::Analyst>,
) -> Result<(Page, Value), String> {
    let page = tab(home, &lead.platform).await?;
    navigate(&page, &lead.profile_url).await?;
    let args = json!({"platform":lead.platform,"userId":lead.platform_user_id});
    if let Some(model) = analyst {
        desktop::perform(model, &page, &args, Stage::Chat, "", cancel).await?;
        return Ok((page, args));
    }
    let mut last = String::new();
    for _ in 0..5 {
        wait(cancel, Duration::from_millis(600)).await?;
        match op::<bool>(&page, "openChat", &args).await {
            Ok(true) => {
                wait(cancel, Duration::from_millis(850)).await?;
                return Ok((page, args));
            }
            Ok(false) => {}
            Err(e) => last = e,
        }
    }
    Err(format!("无法打开客户的私信会话：{last}"))
}
#[derive(Deserialize)]
pub(super) struct Incoming {
    pub id: String,
    pub text: String,
}
pub(super) async fn sync(
    home: &Path,
    lead: &lumo_storage::AcquisitionLead,
    cancel: &CancelToken,
    analyst: Option<&super::acquisition_analysis::Analyst>,
) -> Result<Vec<Incoming>, String> {
    let (page, args) = conversation(home, lead, cancel, analyst).await?;
    op(&page, "sync", &args).await
}
pub(super) enum Delivery {
    Sent,
    Uncertain(String),
}
pub(super) async fn send(
    home: &Path,
    lead: &lumo_storage::AcquisitionLead,
    body: &str,
    cancel: &CancelToken,
    analyst: Option<&super::acquisition_analysis::Analyst>,
) -> Result<Delivery, String> {
    let (page, mut args) = conversation(home, lead, cancel, analyst).await?;
    args["body"] = json!(body);
    let before: Vec<String> = op(&page, "prepare", &args).await?;
    args["before"] = json!(before);
    check(cancel)?;
    if let Some(model) = analyst {
        desktop::perform(model, &page, &args, Stage::Draft, body, cancel).await?;
    } else if !op::<bool>(&page, "fill", &args).await? {
        return Err("写入草稿失败，请检查会话输入框。".into());
    }
    check(cancel)?;
    match op::<bool>(&page, "send", &args).await {
        Ok(false) => return Err("收件人、消息全文或发送按钮未通过核对。".into()),
        Err(e) => return Ok(Delivery::Uncertain(e)),
        Ok(true) => {}
    }
    // Once clicked, resolve the receipt even if the next operation was cancelled.
    for _ in 0..8 {
        sleep(Duration::from_millis(500)).await;
        if let Ok(true) = op::<bool>(&page, "receipt", &args).await {
            return Ok(Delivery::Sent);
        }
    }
    Ok(Delivery::Uncertain(
        "已点击发送，尚未识别到带新消息 ID 的出站回执，请核实平台会话。".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use chromiumoxide::cdp::browser_protocol::fetch::{EventRequestPaused, FulfillRequestParams};
    use chromiumoxide::layout::Point;

    // CDP supplies trusted pointer events to exercise the page side of the native
    // handshake without granting the test runner control over a user's desktop.
    async fn gesture_fixture(p: &Page, args: &Value, stage: &str, selector: &str) -> String {
        let [x,y]:[f64;2]=p.evaluate_expression(format!("(()=>{{const r=document.querySelector({}).getBoundingClientRect();return [r.x+r.width/2,r.y+r.height/2]}})()",json!(selector))).await.unwrap().into_value().unwrap();
        p.evaluate_expression("window.addEventListener('mousemove', e=>window.fixturePoint=[e.screenX,e.screenY], {once:true}); true").await.unwrap();
        p.move_mouse(Point { x, y }).await.unwrap();
        let [sx, sy]: [f64; 2] = p
            .evaluate_expression("window.fixturePoint")
            .await
            .unwrap()
            .into_value()
            .unwrap();
        let mut a = args.clone();
        a["token"] = json!("test-capture");
        a["stage"] = json!(stage);
        a["x"] = json!(sx);
        a["y"] = json!(sy);
        assert!(op::<bool>(p, "gestureArm", &a).await.unwrap());
        p.move_mouse(Point { x: x + 1., y }).await.unwrap();
        let target: String = op(p, "gesturePeek", &a).await.unwrap();
        a["expected"] = json!("no approved target");
        assert_eq!(op::<String>(p, "gestureConsume", &a).await.unwrap(), "");
        if !target.is_empty() {
            a["expected"] = json!(target);
            assert_eq!(op::<String>(p, "gestureConsume", &a).await.unwrap(), target);
            assert_eq!(op::<String>(p, "gestureConsume", &a).await.unwrap(), "");
        }
        op::<bool>(p, "gestureClear", &a).await.unwrap();
        target
    }

    // Chrome redirects all hostnames to nowhere; the CDP fixture fulfills every request.
    #[tokio::test]
    #[ignore = "requires a local Chromium binary"]
    async fn acquisition_chromium_recipient_and_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = BrowserConfig::builder()
            .user_data_dir(dir.path())
            .arg("--host-resolver-rules=MAP * ~NOTFOUND")
            .enable_request_intercept()
            .build()
            .unwrap();
        let (mut browser, mut handler) = Browser::launch(cfg).await.unwrap();
        let pump = tokio::spawn(async move { while handler.next().await.is_some() {} });
        let p = browser.new_page("about:blank").await.unwrap();
        let mut events = p.event_listener::<EventRequestPaused>().await.unwrap();
        let intercept = p.clone();
        let responder = tokio::spawn(async move {
            while let Some(e) = events.next().await {
                let doc = r#"<html><meta charset=utf-8><article><a href='/u/5'>店主</a><a href='/5/abc'>原帖</a><div class='content'>需要预约系统</div></article>
                <button onclick="document.querySelector('section').hidden=false">私信</button><section role='dialog' data-peer-id='5' hidden>
                <div data-message-id='r1' data-direction='inbound'><span class='message-text'>需要预约系统</span></div>
                <textarea></textarea><button onclick="const d=document.createElement('div');d.dataset.messageId='s1';d.dataset.direction='outbound';d.textContent=document.querySelector('textarea').value;this.parentNode.append(d)">发送</button></section></html>"#;
                let response = FulfillRequestParams::builder()
                    .body(
                        STANDARD.encode(if e.request.url.contains("xiaohongshu.com") {
                            doc.replace("<article>", "<article class='note-item'>")
                                .replace("/u/5", "/user/profile/5")
                                .replace("/5/abc", "/explore/abc")
                        } else if e.request.url.contains("douyin.com") {
                            doc.replace("<article>", "<article data-e2e='search-card'>")
                                .replace("/u/5", "/user/5")
                                .replace("/5/abc", "/video/12345")
                        } else {
                            doc.into()
                        }),
                    )
                    .request_id(e.request_id.clone())
                    .response_code(200)
                    .build()
                    .unwrap();
                if intercept.execute(response).await.is_err() {
                    break;
                }
            }
        });
        for (platform, search) in [
            ("weibo", "https://s.weibo.com/weibo?q=abc"),
            ("douyin", "https://www.douyin.com/search/abc"),
            (
                "xiaohongshu",
                "https://www.xiaohongshu.com/search_result?keyword=abc",
            ),
        ] {
            let mut args =
                json!({"platform":platform,"userId":"5","body":"想了解你的预约需求。","before":[]});
            p.goto(search).await.unwrap();
            let batch: Results = op(&p, "discover", &args).await.unwrap();
            assert_eq!(batch.rows.len(), 1);
            let row = &batch.rows[0];
            assert_eq!(row.evidence, "需要预约系统");
            assert_eq!(row.platform_user_id, "5");
            assert_eq!(
                gesture_fixture(&p, &args, "post", "article a:nth-of-type(2)").await,
                row.source_url
            );
            p.goto(&row.profile_url).await.unwrap();
            assert_eq!(
                gesture_fixture(&p, &args, "chat", "body > button").await,
                "chat"
            );
            assert!(op::<bool>(&p, "peer", &args).await.unwrap());
            assert!(gesture_fixture(&p, &args, "chat", "section button")
                .await
                .is_empty());
            assert_eq!(
                op::<Vec<Incoming>>(&p, "sync", &args).await.unwrap()[0].id,
                "r1"
            );
            assert!(op::<Vec<String>>(&p, "prepare", &args)
                .await
                .unwrap()
                .is_empty());
            assert!(op::<bool>(&p, "fill", &args).await.unwrap());
            assert!(op::<bool>(&p, "draftMatches", &args).await.unwrap());
            args["userId"] = json!("6");
            assert!(op::<bool>(&p, "send", &args).await.is_err());
            args["userId"] = json!("5");
            assert!(!op::<bool>(&p, "receipt", &args).await.unwrap());
            assert!(op::<bool>(&p, "send", &args).await.unwrap());
            assert!(op::<bool>(&p, "receipt", &args).await.unwrap());
            assert!(!op::<bool>(&p, "send", &args).await.unwrap());
        }
        browser.close().await.unwrap();
        responder.abort();
        pump.abort();
    }

    #[tokio::test]
    #[ignore = "requires a local Chromium binary"]
    async fn acquisition_chromium_workbench_with_existing_customers() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = BrowserConfig::builder()
            .user_data_dir(dir.path())
            .arg("--host-resolver-rules=MAP * ~NOTFOUND")
            .build()
            .unwrap();
        let (mut browser, mut handler) = Browser::launch(cfg).await.unwrap();
        let pump = tokio::spawn(async move { while handler.next().await.is_some() {} });
        let html = include_str!("../../../../scripts/fixtures/acquisition-workbench.html").replace(
            "/* PRODUCTION_ACQUISITION_MODULE */",
            include_str!("../../frontend/src/js/acquisition.js"),
        );
        let page = browser.new_page("about:blank").await.unwrap();
        page.goto(format!(
            "data:text/html;charset=utf-8;base64,{}",
            STANDARD.encode(html)
        ))
        .await
        .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let value: String = page
                    .evaluate_expression("document.body?.dataset.result || ''")
                    .await
                    .unwrap()
                    .into_value()
                    .unwrap();
                if !value.is_empty() {
                    break value;
                }
                sleep(Duration::from_millis(25)).await;
            }
        })
        .await;
        browser.close().await.unwrap();
        pump.abort();
        assert_eq!(result.expect("workbench did not finish"), "pass");
    }
}
