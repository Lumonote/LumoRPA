use super::*;
use lumo_ai::{AiRouter, ChatRequest, ChatResponse, LlmProvider, ProviderError, ProviderId};
use std::sync::Arc;

const PLATFORMS: [&str; 3] = ["xiaohongshu", "douyin", "weibo"];
const POST: &str = "门店需要预约系统";
const REPLY: &str = "需要到店前一天提醒";

#[tokio::test]
async fn manual_reply_retries_survive_reopen_without_collapsing_distinct_replies() {
    const FIRST: &str = "11111111111111111111111111111111";
    const SECOND: &str = "22222222222222222222222222222222";
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("manual.db");
    let w = setup(Repo::open(&path).unwrap(), "notes", "draft");
    w.run("discover").await.unwrap();
    let leads = w.r.acquisition_list_leads(Some("notes")).unwrap();
    let lid = &leads[0].id;
    store_manual_note(&w.r, lid, REPLY, Some(FIRST)).unwrap();
    drop(w);

    let r = Repo::open(&path).unwrap();
    store_manual_note(&r, lid, REPLY, Some(FIRST)).unwrap();
    assert!(store_manual_note(&r, lid, "不同的内容", Some(FIRST)).is_err());
    assert!(store_manual_note(&r, lid, REPLY, Some("invalid")).is_err());
    assert_eq!(r.acquisition_list_messages(lid).unwrap().len(), 1);
    store_manual_note(&r, lid, REPLY, Some(SECOND)).unwrap();
    store_manual_note(&r, &leads[1].id, REPLY, Some(FIRST)).unwrap();
    assert_eq!(r.acquisition_list_messages(lid).unwrap().len(), 2);
    assert_eq!(r.acquisition_list_messages(&leads[1].id).unwrap().len(), 1);
}

#[derive(Clone, Default)]
struct LocalModel {
    fail_on: Option<&'static str>,
    requests: Arc<Mutex<Vec<Vec<String>>>>,
}

#[async_trait::async_trait]
impl LlmProvider for LocalModel {
    fn name(&self) -> &str {
        "fixture"
    }
    fn id(&self) -> ProviderId {
        ProviderId::OpenaiCompat
    }
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let input: Value = serde_json::from_str(&request.messages[0].content).unwrap();
        let sources: Vec<String> = serde_json::from_value(input["sources"].clone()).unwrap();
        assert!(sources.iter().any(|source| source == POST));
        self.requests.lock().unwrap().push(sources.clone());
        if self
            .fail_on
            .is_some_and(|needle| sources.iter().any(|source| source == needle))
        {
            return Err(ProviderError::Other("fixture model unavailable".into()));
        }
        let evidence = sources.last().unwrap();
        let output = json!({"matched":true,"intentScore":90,"matchEvidence":POST,"summary":evidence,
            "tags":["预约"],"nextAction":"","requirements":[
                {"summary":evidence,"category":"预约","urgency":"中","evidence":evidence},
                {"summary":"无原文支持的推测","category":"价格","urgency":"高","evidence":"虚构引文"}]});
        Ok(ChatResponse {
            content: output.to_string(),
            model: request.model,
            provider: self.name().into(),
            input_tokens: 0,
            output_tokens: 0,
        })
    }
}

#[derive(Default)]
struct LocalPlatform {
    model: LocalModel,
    searches: Mutex<Vec<String>>,
    deliveries: Mutex<Vec<(String, String)>>,
    unavailable: Option<&'static str>,
    ambiguous_receipt: bool,
}

#[async_trait::async_trait]
impl PlatformIo for LocalPlatform {
    fn analyst(&self) -> Res<Analyst> {
        Ok(Analyst {
            router: Arc::new(
                AiRouter::builder()
                    .with(self.model.clone())
                    .active("fixture")
                    .build(),
            ),
            model: "fixture/test".into(),
            vision_model: None,
            vision_issue: None,
        })
    }
    async fn search(
        &self,
        _: &Campaign,
        p: &str,
        keyword: &str,
        _: &Analyst,
        _: &CancelToken,
    ) -> Res<Vec<computer::Candidate>> {
        assert_eq!(keyword, "预约");
        self.searches.lock().unwrap().push(p.into());
        if self.unavailable == Some(p) {
            return Err("需要登录".into());
        }
        // The same external ID on different platforms denotes distinct accounts.
        Ok(vec![computer::Candidate {
            platform_user_id: "5".into(),
            display_name: format!("{p} 店主"),
            profile_url: format!("https://www.{p}.com/user/5"),
            source_url: format!("https://www.{p}.com/post/11"),
            evidence: POST.into(),
        }])
    }
    async fn send(
        &self,
        l: &Lead,
        body: &str,
        _: &CancelToken,
        _: bool,
    ) -> Res<computer::Delivery> {
        self.deliveries
            .lock()
            .unwrap()
            .push((l.id.clone(), body.into()));
        Ok(if self.ambiguous_receipt {
            computer::Delivery::Uncertain("无回执".into())
        } else {
            computer::Delivery::Sent
        })
    }
    async fn sync(&self, _: &Lead, _: &CancelToken, _: bool) -> Res<Vec<computer::Incoming>> {
        Ok(vec![computer::Incoming {
            id: "reply-1".into(),
            text: REPLY.into(),
        }])
    }
}

fn setup(repo: Repo, cid: &str, mode: &str) -> Worker<LocalPlatform> {
    let c = repo
        .acquisition_save_campaign(&Campaign {
            id: cid.into(),
            name: cid.into(),
            product: "门店助手".into(),
            target_audience: "店主".into(),
            platforms: PLATFORMS.map(String::from).into(),
            keywords: vec!["预约".into()],
            max_leads: 3,
            status: "draft".into(),
            created_at: 0,
            updated_at: 0,
            last_error: None,
            outreach: lumo_storage::AcquisitionOutreachPolicy {
                mode: mode.into(),
                message_template: "{{name}}，需要了解{{product}}吗？".into(),
                max_messages_per_run: 1,
                max_messages_per_day: 1,
                ..Default::default()
            },
        })
        .unwrap();
    Worker {
        io: LocalPlatform::default(),
        changed: Box::new(|_| {}),
        r: repo,
        c,
        cancel: CancelToken::new(),
    }
}

fn approve(w: &mut Worker<LocalPlatform>) {
    assert!(w
        .r
        .acquisition_approve_outreach(&w.c.id, w.c.updated_at)
        .unwrap());
    w.c = campaign(&w.r, &w.c.id).unwrap();
}

#[tokio::test]
async fn three_platforms_approval_receipt_reply_and_scoped_demands_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lumo.db");
    let r = Repo::open(&path).unwrap();
    let mut w = setup(r.clone(), "first", "approved_auto");
    w.run("discover").await.unwrap();
    assert_eq!(*w.io.searches.lock().unwrap(), PLATFORMS);
    assert!(w.run("outreach").await.unwrap_err().contains("批准"));
    assert!(w.io.deliveries.lock().unwrap().is_empty());
    approve(&mut w);
    w.run("acquire").await.unwrap();
    w.run("acquire").await.unwrap(); // includes a repeated reply and an exhausted quota
    let leads = r.acquisition_list_leads(Some("first")).unwrap();
    assert_eq!(leads.len(), 3);
    for p in PLATFORMS {
        assert_eq!(leads.iter().filter(|l| l.platform == p).count(), 1);
    }
    let l = leads.iter().find(|l| l.status == "replied").unwrap();
    assert_eq!(
        *w.io.deliveries.lock().unwrap(),
        [(
            l.id.clone(),
            format!("{}，需要了解门店助手吗？", l.display_name)
        )]
    );
    let messages = r.acquisition_list_messages(&l.id).unwrap();
    assert_eq!(messages.len(), 2);
    assert!(messages.iter().any(|m| m.status == "sent"));
    assert!(messages
        .iter()
        .any(|m| m.status == "received" && m.body == REPLY));
    assert_eq!(l.profile["summary"], REPLY);
    assert!(r.acquisition_pending_leads("first").unwrap().is_empty());
    assert!(r
        .acquisition_list_activity(Some("first"), 100)
        .unwrap()
        .iter()
        .any(|a| a.kind == "quota"));
    let other = setup(r.clone(), "second", "draft");
    other.run("discover").await.unwrap();
    drop((other, w, r));
    let reopened = Repo::open(path).unwrap();
    let needs = reopened.acquisition_campaign_requirements("first").unwrap();
    assert_eq!(needs.len(), 4);
    assert!(needs
        .iter()
        .all(|n| leads.iter().any(|l| l.id == n.lead_id)));
    assert_eq!(
        needs
            .iter()
            .filter(|n| n.lead_id == l.id && n.evidence == REPLY)
            .count(),
        1
    );
    assert_eq!(
        reopened
            .acquisition_campaign_requirements("second")
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn existing_drafts_do_not_starve_other_leads() {
    let dir = tempfile::tempdir().unwrap();
    let w = setup(
        Repo::open(dir.path().join("lumo.db")).unwrap(),
        "drafts",
        "draft",
    );
    w.run("discover").await.unwrap();
    let leads = w.r.acquisition_list_leads(Some("drafts")).unwrap();
    for expected in [1, 2, 3, 3] {
        w.run("outreach").await.unwrap();
        let messages: Vec<_> = leads
            .iter()
            .flat_map(|l| w.r.acquisition_list_messages(&l.id).unwrap())
            .collect();
        assert_eq!(messages.len(), expected);
        assert!(messages.iter().all(|m| m.status == "draft"));
    }
    assert!(w.io.deliveries.lock().unwrap().is_empty());
}

#[tokio::test]
async fn failed_analysis_keeps_replies_and_resumes_from_last_committed_batch() {
    const FIRST: &str = "需要预约确认";
    const LAST: &str = "需要批量导出";
    const OTHER: &str = "需要到店统计";
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lumo.db");
    let mut w = setup(Repo::open(&path).unwrap(), "retry", "draft");
    w.run("discover").await.unwrap();
    let mut leads = w.r.acquisition_list_leads(Some("retry")).unwrap();
    leads.sort_by(|a, b| a.id.cmp(&b.id));
    let (affected, peer) = (&leads[0], &leads[1]);
    for body in [
        FIRST,
        "需要通知",
        "需要改期",
        "需要取消",
        "需要查询",
        "需要打印",
        LAST,
    ] {
        store_message(&w.r, id(), &affected.id, "inbound", body).unwrap();
    }
    store_message(&w.r, id(), &peer.id, "inbound", OTHER).unwrap();
    w.io.model.fail_on = Some(LAST);
    assert!(w.run("analyze_pending").await.unwrap_err().contains("1 位"));
    assert_eq!(
        w.r.acquisition_pending_leads("retry").unwrap().as_slice(),
        std::slice::from_ref(&affected.id)
    );
    assert_eq!(
        lead(&w.r, &affected.id).unwrap().profile["summary"],
        "需要打印"
    );
    assert_eq!(lead(&w.r, &peer.id).unwrap().profile["summary"], OTHER);
    assert!(w
        .r
        .acquisition_list_activity(Some("retry"), 100)
        .unwrap()
        .iter()
        .any(|a| a.kind == "analysis_error" && a.lead_id.as_deref() == Some(&affected.id)));

    let c = w.c.clone();
    drop(w);
    let reopened = Repo::open(&path).unwrap();
    let io = LocalPlatform::default();
    let analyst = io.analyst().unwrap();
    analysis::analyze_manually(
        &analyst,
        &reopened,
        lead(&reopened, &affected.id).unwrap(),
        &c,
    )
    .await
    .unwrap();
    assert_eq!(
        *io.model.requests.lock().unwrap(),
        [vec![POST, "需要打印", LAST]]
    );
    assert_eq!(
        lead(&reopened, &affected.id).unwrap().profile["summary"],
        LAST
    );
    assert!(reopened
        .acquisition_pending_leads("retry")
        .unwrap()
        .is_empty());
    assert_eq!(
        reopened
            .acquisition_list_messages(&affected.id)
            .unwrap()
            .len(),
        7
    );

    io.model.requests.lock().unwrap().clear();
    analysis::analyze_manually(
        &analyst,
        &reopened,
        lead(&reopened, &affected.id).unwrap(),
        &c,
    )
    .await
    .unwrap();
    assert!(io.model.requests.lock().unwrap()[0]
        .iter()
        .any(|entry| entry == FIRST));
    assert!(reopened
        .acquisition_pending_leads("retry")
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn failed_platform_preserves_peers_and_ambiguous_sends_are_not_retried() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = setup(
        Repo::open(dir.path().join("lumo.db")).unwrap(),
        "partial",
        "approved_auto",
    );
    w.io.unavailable = Some("douyin");
    w.io.ambiguous_receipt = true;
    approve(&mut w);
    let issue = w.run("acquire").await.unwrap_err();
    assert!(
        issue.contains("抖音：需要登录") && issue.contains("无回执"),
        "{issue}"
    );
    let leads = w.r.acquisition_list_leads(Some("partial")).unwrap();
    assert_eq!(leads.len(), 2);
    assert!(leads.iter().all(|l| l.platform != "douyin"));
    let lid = w.io.deliveries.lock().unwrap()[0].0.clone();
    assert_eq!(
        w.r.acquisition_list_messages(&lid).unwrap()[0].status,
        "uncertain"
    );
    // Remove the other eligible recipient so that only the uncertain identity is tested.
    let other = leads.iter().find(|l| l.id != lid).unwrap();
    w.r.acquisition_update_lead_status(&other.id, "ignored")
        .unwrap();
    w.run("outreach").await.unwrap();
    assert_eq!(w.io.deliveries.lock().unwrap().len(), 1);
    assert!(w
        .r
        .acquisition_list_activity(Some("partial"), 100)
        .unwrap()
        .iter()
        .any(|a| a.kind == "skipped"));
    w.cancel.cancel();
    assert!(w.run("discover").await.is_err());
    assert_eq!(w.io.searches.lock().unwrap().len(), 3);
}
