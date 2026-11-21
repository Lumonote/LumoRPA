use lumo_storage::*;
use serde_json::json;
fn campaign(repo: &Repo) -> AcquisitionCampaign {
    let c=serde_json::from_value(json!({"id":"c","name":"预约","product":"预约系统","targetAudience":"店主","keywords":["预约"],"platforms":["weibo"],"maxLeads":5,"status":"draft","createdAt":0,"updatedAt":0,"outreach":{"mode":"approved_auto","messageTemplate":"你好 {{name}}","approvedAt":123}})).unwrap();
    repo.acquisition_save_campaign(&c).unwrap()
}
fn prospect(repo: &Repo, c: &AcquisitionCampaign) -> AcquisitionLead {
    let l=serde_json::from_value(json!({"id":"l","campaignId":c.id,"platform":"weibo","platformUserId":"5","displayName":"店主","profileUrl":"https://weibo.com/u/5","sourceUrl":"https://weibo.com/5/test","evidence":"想找预约系统","bio":"","status":"qualified","intentScore":90,"tags":[],"profile":{"scope":acquisition_scope(c),"matched":true},"createdAt":0,"updatedAt":0})).unwrap();
    repo.acquisition_upsert_lead(&l).unwrap()
}
fn msg(id: &str, direction: &str, body: &str) -> AcquisitionMessage {
    AcquisitionMessage {
        id: id.into(),
        lead_id: "l".into(),
        direction: direction.into(),
        status: if direction == "inbound" {
            "received"
        } else {
            "draft"
        }
        .into(),
        body: body.into(),
        error: None,
        created_at: 0,
        updated_at: 0,
    }
}
#[test]
fn approval_and_recovery_prevent_duplicate_first_contact() {
    let r = Repo::open_in_memory().unwrap();
    let c = campaign(&r);
    let l = prospect(&r, &c);
    assert_eq!(c.outreach.approved_at, None);
    let m = r
        .acquisition_add_message(&msg("m", "outbound", &acquisition_render_message(&c, &l)))
        .unwrap();
    assert!(r.acquisition_reserve_send(&m.id, true).is_err());
    assert!(r.acquisition_approve_outreach("c", c.updated_at).unwrap());
    assert!(r
        .acquisition_transition_campaign("c", "draft", "running", None)
        .unwrap());
    assert_eq!(
        r.acquisition_reserve_send("m", true).unwrap().status,
        "sending"
    );
    assert!(r.acquisition_reserve_send("m", true).is_err());
    r.acquisition_recover_interrupted().unwrap();
    assert_eq!(
        r.acquisition_get_message("m").unwrap().unwrap().status,
        "uncertain"
    );
    assert_eq!(
        r.acquisition_add_message(&msg("retry", "outbound", &m.body))
            .unwrap()
            .id,
        "m"
    );
    assert!(r
        .acquisition_transition_message(
            "m",
            "uncertain",
            "failed",
            Some("operator checked platform")
        )
        .unwrap());
    assert_eq!(
        r.acquisition_add_message(&msg("retry", "outbound", &m.body))
            .unwrap()
            .id,
        "retry"
    );
    assert_eq!(
        r.acquisition_get_campaign("c").unwrap().unwrap().status,
        "failed"
    );
    assert!(r.acquisition_reserve_send("m", true).is_err());
}
#[test]
fn new_scope_requires_new_assessment_even_after_reapproval() {
    let r = Repo::open_in_memory().unwrap();
    let mut c = campaign(&r);
    let l = prospect(&r, &c);
    r.acquisition_add_message(&msg("m", "outbound", &acquisition_render_message(&c, &l)))
        .unwrap();
    assert!(r.acquisition_approve_outreach("c", c.updated_at).unwrap());
    c.target_audience = "另一组客户".into();
    let c = r.acquisition_save_campaign(&c).unwrap();
    assert_eq!(c.outreach.approved_at, None);
    assert!(r.acquisition_approve_outreach("c", c.updated_at).unwrap());
    assert!(r.acquisition_reserve_send("m", true).is_err());
}

#[test]
fn changing_window_control_revokes_outreach_approval_and_survives_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("mode.db");
    let r = Repo::open(&path).unwrap();
    let mut c = campaign(&r);
    assert!(!c.outreach.desktop_vision);
    assert!(r.acquisition_approve_outreach(&c.id, c.updated_at).unwrap());
    c.outreach.desktop_vision = true;
    assert!(r
        .acquisition_save_campaign(&c)
        .unwrap()
        .outreach
        .approved_at
        .is_none());
    drop(r);
    assert!(
        Repo::open(&path)
            .unwrap()
            .acquisition_get_campaign(&c.id)
            .unwrap()
            .unwrap()
            .outreach
            .desktop_vision
    );
}
#[test]
fn identical_incoming_text_is_distinct_by_source_id() {
    let r = Repo::open_in_memory().unwrap();
    let c = campaign(&r);
    prospect(&r, &c);
    for id in ["first", "first", "second"] {
        r.acquisition_add_message(&msg(id, "inbound", "好的"))
            .unwrap();
    }
    assert_eq!(r.acquisition_list_messages("l").unwrap().len(), 2);
    assert_eq!(
        r.acquisition_get_lead("l").unwrap().unwrap().status,
        "replied"
    );
}

#[test]
fn v8_upgrade_keeps_history_and_retries_analysis_after_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("v8.db");
    let old = rusqlite::Connection::open(&path).unwrap();
    old.execute_batch(lumo_storage::schema::V8_DDL).unwrap();
    old.execute_batch(r#"PRAGMA user_version=8;
       INSERT INTO acquisition_campaigns VALUES('c','旧任务','产品','客户','["需求"]','["weibo"]',1,'completed',1,2,NULL,'{}');
       INSERT INTO acquisition_leads VALUES('l','c','weibo','5','店主','https://weibo.com/u/5','https://weibo.com/5/a','初始需求','','replied',80,'[]','{}',1,2);
       INSERT INTO acquisition_messages VALUES('r','l','inbound','要导出表格','received',2,2,NULL,NULL);
       INSERT INTO acquisition_requirements VALUES('old','l','预约','功能','低','初始需求',1);"#).unwrap();
    drop(old);
    let repo = Repo::open(&path).unwrap();
    assert!(repo
        .acquisition_add_source("l", "https://weibo.com/5/b", "还需要开票")
        .unwrap());
    assert!(!repo
        .acquisition_add_source("l", "https://weibo.com/5/b", "还需要开票")
        .unwrap());
    assert_eq!(repo.acquisition_sources("l").unwrap().len(), 2);
    assert_eq!(repo.acquisition_list_leads(Some("c")).unwrap().len(), 1);
    let stale = repo.acquisition_analysis_batch("l").unwrap().unwrap();
    drop(repo); // model failed before any output was committed
    let repo = Repo::open(&path).unwrap();
    assert_eq!(repo.acquisition_pending_leads("c").unwrap(), vec!["l"]);
    let batch = repo.acquisition_analysis_batch("l").unwrap().unwrap();
    assert_eq!(batch.sources, ["初始需求", "要导出表格", "还需要开票"]);
    let mut l = repo.acquisition_get_lead("l").unwrap().unwrap();
    l.profile = json!({"scope":acquisition_scope(&repo.acquisition_get_campaign("c").unwrap().unwrap()),"summary":"新画像"});
    let mut n = repo
        .acquisition_list_requirements(Some("l"))
        .unwrap()
        .remove(0);
    n.summary = "开票".into();
    n.evidence = "还需要开票".into();
    // Conflicting requirement id must roll back both profile and cursor.
    assert!(repo
        .acquisition_finish_analysis(&batch, &l, &[n.clone()])
        .is_err());
    assert_eq!(
        repo.acquisition_get_lead("l").unwrap().unwrap().profile,
        json!({})
    );
    n.id = "new".into();
    repo.acquisition_add_message(&msg("r2", "inbound", "另外还需多人使用"))
        .unwrap();
    assert!(repo.acquisition_finish_analysis(&batch, &l, &[n]).unwrap());
    l.profile["summary"] = json!("过期画像");
    assert!(!repo.acquisition_finish_analysis(&stale, &l, &[]).unwrap());
    assert_eq!(
        repo.acquisition_get_lead("l").unwrap().unwrap().profile["summary"],
        "新画像"
    );
    let next = repo.acquisition_analysis_batch("l").unwrap().unwrap();
    assert!(next.sources.contains(&"另外还需多人使用".to_string()));
    assert!(repo.acquisition_finish_analysis(&next, &l, &[]).unwrap());
    repo.acquisition_add_message(&msg("r2", "inbound", "另外还需多人使用"))
        .unwrap();
    assert!(repo.acquisition_pending_leads("c").unwrap().is_empty());
    assert_eq!(
        repo.acquisition_list_requirements(Some("l")).unwrap().len(),
        2
    );
}

#[test]
fn automatic_reservations_distinguish_identity_interval_budget_and_platform() {
    use lumo_storage::error::SendBlock;
    let r = Repo::open_in_memory().unwrap();
    let mut c = campaign(&r);
    c.platforms = vec!["weibo".into(), "douyin".into(), "xiaohongshu".into()];
    c.outreach.max_messages_per_run = 2;
    c.outreach.max_messages_per_day = 2;
    let c = r.acquisition_save_campaign(&c).unwrap();
    r.acquisition_approve_outreach(&c.id, c.updated_at).unwrap();
    let l = prospect(&r, &c);
    let body = acquisition_render_message(&c, &l);
    let first = msg("first", "outbound", &body);
    r.acquisition_reserve_first_contact(&first).unwrap();
    r.acquisition_transition_message("first", "sending", "sent", None)
        .unwrap();
    let mut c2 = c.clone();
    c2.id = "other".into();
    let c2 = r.acquisition_save_campaign(&c2).unwrap();
    r.acquisition_approve_outreach(&c2.id, c2.updated_at)
        .unwrap();
    let mut duplicate = l.clone();
    duplicate.campaign_id = c2.id.clone();
    duplicate.id = "another".into();
    r.acquisition_upsert_lead(&duplicate).unwrap();
    let mut draft = msg("later", "outbound", &body);
    draft.lead_id = duplicate.id;
    assert!(matches!(
        r.acquisition_reserve_first_contact(&draft),
        Err(StorageError::SendBlocked(SendBlock::Recipient(_)))
    ));
    assert!(r.acquisition_get_message("later").unwrap().is_none());
    for (platform, host) in [
        ("douyin", "www.douyin.com"),
        ("xiaohongshu", "www.xiaohongshu.com"),
    ] {
        let mut other = l.clone();
        other.id = platform.into();
        other.platform = platform.into();
        other.profile_url = format!("https://{host}/user/5");
        other.source_url = format!("https://{host}/post/1");
        r.acquisition_upsert_lead(&other).unwrap();
    }
    draft.lead_id = "douyin".into();
    assert!(
        matches!(r.acquisition_reserve_first_contact(&draft),Err(StorageError::SendBlocked(SendBlock::Interval(ms))) if ms>0 && ms<=60000)
    );
    assert!(r.acquisition_get_message("later").unwrap().is_none());
    // Deterministically elapse the interval, keeping both attempts in today's allowance.
    r.with_raw(|c| {
        c.execute(
            "UPDATE acquisition_messages SET attempted_at=attempted_at-60000 WHERE id='first'",
            [],
        )
    })
    .unwrap();
    r.acquisition_reserve_first_contact(&draft).unwrap();
    draft.lead_id = "xiaohongshu".into();
    draft.id = "last".into();
    // Pin both attempts to today even if this test runs immediately after UTC midnight.
    r.with_raw(|c|c.execute("UPDATE acquisition_messages SET attempted_at=(SELECT attempted_at FROM acquisition_messages WHERE id='later') WHERE id='first'",[])).unwrap();
    assert!(matches!(
        r.acquisition_reserve_first_contact(&draft),
        Err(StorageError::SendBlocked(SendBlock::DailyLimit))
    ));
    assert!(r.acquisition_get_message("last").unwrap().is_none());
}
