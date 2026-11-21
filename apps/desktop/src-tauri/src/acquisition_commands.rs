use super::{
    acquisition_analysis::{self as analysis, Analyst},
    acquisition_computer as computer, app_home, AppHandle,
};
use lumo_core::CancelToken;
use lumo_storage::{
    acquisition_render_message, acquisition_scope, error::SendBlock, AcquisitionActivity,
    AcquisitionCampaign as Campaign, AcquisitionLead as Lead, AcquisitionMessage as Message, Repo,
    StorageError,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
    time::Duration,
};
use tauri::Emitter;
use ulid::Ulid;

type Res<T> = Result<T, String>;
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn id() -> String {
    Ulid::new().to_string()
}
fn db(app: &AppHandle) -> Res<Repo> {
    Repo::open(app_home(app)?.join("lumo.db")).map_err(err)
}
pub(super) fn recover(app: &AppHandle) -> Res<()> {
    db(app)?.acquisition_recover_interrupted().map_err(err)
}
fn campaign(r: &Repo, id: &str) -> Res<Campaign> {
    r.acquisition_get_campaign(id)
        .map_err(err)?
        .ok_or("找不到此任务".into())
}
fn lead(r: &Repo, id: &str) -> Res<Lead> {
    r.acquisition_get_lead(id)
        .map_err(err)?
        .ok_or("找不到此客户".into())
}
fn message(r: &Repo, id: &str) -> Res<Message> {
    r.acquisition_get_message(id)
        .map_err(err)?
        .ok_or("找不到此消息".into())
}
fn notify(app: &AppHandle, cid: &str) {
    let _ = app.emit("lumo://acquisition-updated", json!({"campaignId":cid}));
}
fn activity(r: &Repo, cid: &str, lid: Option<&str>, kind: &str, detail: &str) -> Res<()> {
    r.acquisition_add_activity(&AcquisitionActivity {
        id: id(),
        campaign_id: Some(cid.into()),
        lead_id: lid.map(Into::into),
        kind: kind.into(),
        detail: detail.chars().take(4000).collect(),
        created_at: 0,
    })
    .map_err(err)
}
fn detail(r: &Repo, id: &str) -> Res<Value> {
    Ok(
        json!({"lead":lead(r,id)?,"sources":r.acquisition_sources(id).map_err(err)?,"messages":r.acquisition_list_messages(id).map_err(err)?,"requirements":r.acquisition_list_requirements(Some(id)).map_err(err)?}),
    )
}

static ACTIVE: LazyLock<Mutex<Option<(String, CancelToken)>>> = LazyLock::new(Mutex::default);
struct Lease {
    cancel: CancelToken,
}
impl Lease {
    fn claim(owner: &str) -> Res<Self> {
        let mut slot = ACTIVE.lock().map_err(err)?;
        if slot.is_some() {
            return Err("已有获客操作正在使用浏览器，请等待或停止当前任务。".into());
        }
        let cancel = CancelToken::new();
        *slot = Some((owner.into(), cancel.clone()));
        Ok(Self { cancel })
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut slot) = ACTIVE.lock() {
            *slot = None;
        }
    }
}

#[tauri::command]
pub(super) fn acquisition_snapshot(app: AppHandle, campaign_id: Option<String>) -> Res<Value> {
    let r = db(&app)?;
    let campaigns = r.acquisition_list_campaigns().map_err(err)?;
    let selected = campaign_id
        .as_deref()
        .or_else(|| campaigns.first().map(|c| c.id.as_str()));
    let leads = r.acquisition_list_leads(selected).map_err(err)?;
    let pending = selected
        .map(|id| r.acquisition_pending_leads(id))
        .transpose()
        .map_err(err)?
        .unwrap_or_default();
    let ids: HashSet<_> = leads.iter().map(|l| l.id.as_str()).collect();
    let needs: Vec<_> = r
        .acquisition_list_requirements(None)
        .map_err(err)?
        .into_iter()
        .filter(|n| ids.contains(n.lead_id.as_str()))
        .collect();
    let ready = Analyst::load(&app_home(&app)?);
    let config_message = match &ready {
        Ok(a) if a.vision_model.is_some() => {
            format!(
                "视觉模型：{}。桌面视觉模式会将获客窗口截图发送至该模型。",
                a.vision_model.as_deref().unwrap_or_default()
            )
        }
        Ok(_) => "已配置文本分析，匹配客户所用的原文将发送至所选模型。".to_owned(),
        Err(e) => e.clone(),
    };
    let active = ACTIVE
        .lock()
        .map_err(err)?
        .as_ref()
        .map(|(key, _)| key.clone());
    Ok(
        json!({"campaigns":campaigns,"leads":leads,"pendingAnalysisIds":pending,"requirements":needs,"activity":r.acquisition_list_activity(selected,150).map_err(err)?,"runningCampaignIds":active.into_iter().collect::<Vec<_>>(),"computer":{"desktopSupported":cfg!(target_os="macos"),"ready":ready.is_ok(),"message":config_message}}),
    )
}
#[tauri::command]
pub(super) fn acquisition_save_campaign(app: AppHandle, mut campaign: Campaign) -> Res<Campaign> {
    if campaign.id.is_empty() {
        campaign.id = id();
    }
    db(&app)?.acquisition_save_campaign(&campaign).map_err(err)
}
#[tauri::command]
pub(super) fn acquisition_approve_outreach(
    app: AppHandle,
    campaign_id: String,
    expected_updated_at: i64,
) -> Res<Campaign> {
    let r = db(&app)?;
    if !r
        .acquisition_approve_outreach(&campaign_id, expected_updated_at)
        .map_err(err)?
    {
        return Err("任务已更改，请刷新后重新核对范围。".into());
    }
    activity(
        &r,
        &campaign_id,
        None,
        "approval",
        "已批准当前话术、客户范围和发送限额",
    )?;
    campaign(&r, &campaign_id)
}
#[tauri::command]
pub(super) fn acquisition_lead_detail(app: AppHandle, lead_id: String) -> Res<Value> {
    detail(&db(&app)?, &lead_id)
}
#[tauri::command]
pub(super) fn acquisition_set_lead_status(
    app: AppHandle,
    lead_id: String,
    status: String,
) -> Res<Lead> {
    let r = db(&app)?;
    let l = lead(&r, &lead_id)?;
    r.acquisition_update_lead_status(&lead_id, &status)
        .map_err(err)?;
    if status == "ignored" {
        if let Some((_, token)) = ACTIVE.lock().map_err(err)?.as_ref() {
            token.cancel();
        }
    }
    notify(&app, &l.campaign_id);
    lead(&r, &lead_id)
}
fn store_message(r: &Repo, mid: String, lid: &str, direction: &str, body: &str) -> Res<Message> {
    r.acquisition_add_message(&new_message(mid, lid, direction, body))
        .map_err(err)
}
fn new_message(mid: String, lid: &str, direction: &str, body: &str) -> Message {
    Message {
        id: mid,
        lead_id: lid.into(),
        direction: direction.into(),
        body: body.trim().into(),
        status: if direction == "inbound" {
            "received"
        } else {
            "draft"
        }
        .into(),
        created_at: 0,
        updated_at: 0,
        error: None,
    }
}
#[tauri::command]
pub(super) fn acquisition_draft_message(
    app: AppHandle,
    lead_id: String,
    body: String,
) -> Res<Message> {
    store_message(&db(&app)?, id(), &lead_id, "outbound", &body)
}
#[tauri::command]
pub(super) async fn acquisition_add_note(
    app: AppHandle,
    lead_id: String,
    body: String,
) -> Res<Value> {
    let r = db(&app)?;
    store_message(&r, id(), &lead_id, "inbound", &body)?;
    let l = lead(&r, &lead_id)?;
    notify(&app, &l.campaign_id);
    let c = campaign(&r, &l.campaign_id)?;
    let issue = analysis::analyze(&app_home(&app)?, &r, l, &c).await.err();
    let mut result = detail(&r, &lead_id)?;
    result["analysisError"] = json!(issue);
    notify(&app, &c.id);
    Ok(result)
}
#[tauri::command]
pub(super) async fn acquisition_analyze_lead(app: AppHandle, lead_id: String) -> Res<Value> {
    let r = db(&app)?;
    let l = lead(&r, &lead_id)?;
    let c = campaign(&r, &l.campaign_id)?;
    if l.status == "ignored" {
        return Err("已忽略的客户不参与分析。".into());
    }
    r.acquisition_restart_analysis(&lead_id).map_err(err)?;
    analysis::analyze(&app_home(&app)?, &r, l, &c).await?;
    notify(&app, &c.id);
    detail(&r, &lead_id)
}
#[tauri::command]
pub(super) async fn acquisition_browser_open(app: AppHandle, platform: String) -> Res<()> {
    let _lease = Lease::claim("login")?;
    computer::login(&app_home(&app)?, &platform).await
}
async fn deliver_reserved(
    home: &Path,
    r: &Repo,
    l: &Lead,
    m: &Message,
    cancel: &CancelToken,
    desktop_vision: bool,
) -> Res<Message> {
    let (state, note) = match computer::send(home, l, &m.body, cancel, desktop_vision).await {
        Ok(computer::Delivery::Sent) => ("sent", None),
        Ok(computer::Delivery::Uncertain(s)) => ("uncertain", Some(s)),
        Err(e) => ("failed", Some(e)),
    };
    let note = note.map(|s| s.chars().take(4000).collect::<String>());
    if !r
        .acquisition_transition_message(&m.id, "sending", state, note.as_deref())
        .map_err(err)?
    {
        return Err("消息状态已变化，请核实会话。".into());
    }
    activity(
        r,
        &l.campaign_id,
        Some(&l.id),
        "delivery",
        &format!(
            "{} · {} · {}",
            l.display_name,
            state,
            note.as_deref().unwrap_or("已读取出站回执")
        ),
    )?;
    message(r, &m.id)
}
#[tauri::command]
pub(super) async fn acquisition_send_message(app: AppHandle, message_id: String) -> Res<Message> {
    let r = db(&app)?;
    let m = message(&r, &message_id)?;
    let l = lead(&r, &m.lead_id)?;
    let lease = Lease::claim(&l.campaign_id)?;
    let home = app_home(&app)?;
    computer::check(&lease.cancel)?;
    let desktop_vision = campaign(&r, &l.campaign_id)?.outreach.desktop_vision;
    if desktop_vision {
        super::acquisition_desktop::load(&home)?;
    }
    let m = r.acquisition_reserve_send(&m.id, false).map_err(err)?;
    let result = deliver_reserved(&home, &r, &l, &m, &lease.cancel, desktop_vision).await;
    drop(lease);
    notify(&app, &l.campaign_id);
    result
}
#[tauri::command]
pub(super) fn acquisition_resolve_message(
    app: AppHandle,
    message_id: String,
    sent: bool,
) -> Res<Message> {
    let r = db(&app)?;
    let m = message(&r, &message_id)?;
    let l = lead(&r, &m.lead_id)?;
    if !r
        .acquisition_transition_message(
            &m.id,
            "uncertain",
            if sent { "sent" } else { "failed" },
            Some("操作员已核对平台会话"),
        )
        .map_err(err)?
    {
        return Err("该消息没有待核实结果".into());
    }
    activity(
        &r,
        &l.campaign_id,
        Some(&l.id),
        "reconciled",
        if sent {
            "人工核实已发送"
        } else {
            "人工核实未发送"
        },
    )?;
    notify(&app, &l.campaign_id);
    message(&r, &m.id)
}
#[tauri::command]
pub(super) fn acquisition_cancel(campaign_id: String) -> Res<()> {
    let slot = ACTIVE.lock().map_err(err)?;
    let (_, cancel) = slot
        .as_ref()
        .filter(|(id, _)| id == &campaign_id)
        .ok_or("任务没有运行")?;
    cancel.cancel();
    Ok(())
}
struct Worker {
    app: AppHandle,
    home: PathBuf,
    r: Repo,
    c: Campaign,
    cancel: CancelToken,
}
// A login challenge or ambiguous receipt pauses that platform for the
// remainder of this run, including subsequent stages of one-click acquisition.
#[derive(Default)]
struct PlatformRun(BTreeMap<String, String>);
impl PlatformRun {
    fn available(&self, platform: &str) -> bool {
        !self.0.contains_key(platform)
    }
    fn finish(self) -> Res<()> {
        if self.0.is_empty() {
            return Ok(());
        }
        Err(format!(
            "以下平台未完成，已保留本次其他结果：{}",
            self.0
                .into_iter()
                .map(|(p, e)| format!("{}：{e}", platform_name(&p)))
                .collect::<Vec<_>>()
                .join("；")
        ))
    }
}
fn platform_name(platform: &str) -> &str {
    match platform {
        "xiaohongshu" => "小红书",
        "douyin" => "抖音",
        "weibo" => "微博",
        other => other,
    }
}
fn alternate_platforms<T>(
    mut queues: VecDeque<(&str, std::vec::IntoIter<T>)>,
) -> impl Iterator<Item = (&str, T)> {
    std::iter::from_fn(move || {
        while let Some((platform, mut rows)) = queues.pop_front() {
            if let Some(row) = rows.next() {
                if rows.len() > 0 {
                    queues.push_back((platform, rows));
                }
                return Some((platform, row));
            }
        }
        None
    })
}
impl Worker {
    fn log(&self, kind: &str, detail: &str) -> Res<()> {
        activity(&self.r, &self.c.id, None, kind, detail)?;
        notify(&self.app, &self.c.id);
        Ok(())
    }
    fn pause_platform(&self, run: &mut PlatformRun, platform: &str, issue: String) -> Res<()> {
        computer::check(&self.cancel)?;
        self.log(
            "platform_paused",
            &format!("{} 本轮暂停：{issue}", platform_name(platform)),
        )?;
        run.0.insert(platform.into(), issue);
        Ok(())
    }
    async fn discover(&self, run: &mut PlatformRun) -> Res<()> {
        let model = Analyst::load(&self.home)?;
        let old = self
            .r
            .acquisition_list_leads(Some(&self.c.id))
            .map_err(err)?;
        let mut seen: HashMap<_, _> = old
            .iter()
            .map(|l| {
                (
                    (l.platform.clone(), l.platform_user_id.clone()),
                    l.id.clone(),
                )
            })
            .collect();
        for keyword in &self.c.keywords {
            let mut batches = VecDeque::new();
            for platform in &self.c.platforms {
                computer::check(&self.cancel)?;
                if !run.available(platform) {
                    continue;
                }
                self.log(
                    "search",
                    &format!("搜索 {} · {keyword}", platform_name(platform)),
                )?;
                let rows = match computer::search(
                    &self.home,
                    platform,
                    keyword,
                    &self.c.target_audience,
                    &model,
                    self.c.outreach.desktop_vision,
                    &self.cancel,
                )
                .await
                {
                    Ok(rows) => rows,
                    Err(issue) => {
                        self.pause_platform(run, platform, issue)?;
                        continue;
                    }
                };
                batches.push_back((platform.as_str(), rows.into_iter()));
            }
            for (platform, row) in alternate_platforms(batches) {
                computer::check(&self.cancel)?;
                let key = (platform.to_owned(), row.platform_user_id.clone());
                if let Some(lid) = seen.get(&key) {
                    self.r
                        .acquisition_add_source(lid, &row.source_url, &row.evidence)
                        .map_err(err)?;
                    continue;
                }
                if seen.len() >= self.c.max_leads as usize {
                    continue;
                }
                let result = tokio::select! { _=self.cancel.cancelled()=>return Err("任务已停止".into()), r=model.assess(&self.c,std::slice::from_ref(&row.evidence))=>r? };
                if !result.matched {
                    continue;
                }
                let mut l = Lead {
                    id: id(),
                    campaign_id: self.c.id.clone(),
                    platform: platform.to_owned(),
                    platform_user_id: row.platform_user_id.clone(),
                    display_name: row.display_name,
                    profile_url: row.profile_url,
                    source_url: row.source_url,
                    evidence: row.evidence,
                    bio: String::new(),
                    status: "qualified".into(),
                    intent_score: 0,
                    tags: vec![],
                    profile: json!({}),
                    created_at: 0,
                    updated_at: 0,
                };
                result.apply(&mut l, &self.c);
                let l = self
                    .r
                    .acquisition_store_assessed_lead(&l, &result.needs(&l.id))
                    .map_err(err)?;
                seen.insert(key, l.id);
                notify(&self.app, &self.c.id);
            }
        }
        Ok(())
    }
    async fn outreach(&self, run: &mut PlatformRun) -> Res<()> {
        let auto = self.c.outreach.mode == "approved_auto";
        let mut count = 0;
        let mut attempts = 0;
        if auto && self.c.outreach.approved_at.is_none() {
            return Err("请先批准当前话术与范围。".into());
        }
        'leads: for l in self
            .r
            .acquisition_list_leads(Some(&self.c.id))
            .map_err(err)?
        {
            computer::check(&self.cancel)?;
            if attempts >= self.c.outreach.max_messages_per_run {
                break;
            }
            if !run.available(&l.platform)
                || !matches!(l.status.as_str(), "new" | "qualified")
                || !self.c.platforms.contains(&l.platform)
                || l.intent_score < self.c.outreach.min_intent_score
                || l.profile.get("scope") != Some(&acquisition_scope(&self.c))
                || l.profile["matched"] != true
            {
                continue;
            }
            let draft = new_message(
                id(),
                &l.id,
                "outbound",
                &acquisition_render_message(&self.c, &l),
            );
            if auto {
                let m = loop {
                    computer::check(&self.cancel)?;
                    match self.r.acquisition_reserve_first_contact(&draft) {
                        Ok(m) => break m,
                        Err(StorageError::SendBlocked(SendBlock::DailyLimit)) => {
                            self.log("quota", "今日发送配额已用完，其余客户留待下次联系。")?;
                            break 'leads;
                        }
                        Err(StorageError::SendBlocked(SendBlock::Interval(ms))) => {
                            self.log(
                                "interval",
                                &format!("距离下一次发送还需 {} 秒", ms.div_ceil(1000)),
                            )?;
                            computer::wait(&self.cancel, Duration::from_millis(ms)).await?;
                        }
                        Err(StorageError::SendBlocked(SendBlock::Recipient(reason))) => {
                            activity(
                                &self.r,
                                &self.c.id,
                                Some(&l.id),
                                "skipped",
                                &format!("{} · {reason}", l.display_name),
                            )?;
                            continue 'leads;
                        }
                        Err(e) => return Err(err(e)),
                    }
                };
                attempts += 1;
                let result = deliver_reserved(
                    &self.home,
                    &self.r,
                    &l,
                    &m,
                    &self.cancel,
                    self.c.outreach.desktop_vision,
                )
                .await?;
                if result.status != "sent" {
                    self.pause_platform(
                        run,
                        &l.platform,
                        result.error.unwrap_or("请核实发送结果".into()),
                    )?;
                    continue;
                }
            } else {
                let m = self.r.acquisition_add_message(&draft).map_err(err)?;
                if !matches!(m.status.as_str(), "draft" | "approved") {
                    continue;
                }
            }
            count += 1;
            notify(&self.app, &self.c.id);
            if count >= self.c.outreach.max_messages_per_run {
                break;
            }
        }
        self.log(
            "outreach",
            &format!("本次{} {count} 条", if auto { "发送" } else { "准备草稿" }),
        )
    }
    async fn sync(&self, run: &mut PlatformRun) -> Res<()> {
        for l in self
            .r
            .acquisition_list_leads(Some(&self.c.id))
            .map_err(err)?
        {
            computer::check(&self.cancel)?;
            if !run.available(&l.platform)
                || !matches!(l.status.as_str(), "contacted" | "replied")
                || !self.c.platforms.contains(&l.platform)
            {
                continue;
            }
            let incoming =
                match computer::sync(&self.home, &l, &self.cancel, self.c.outreach.desktop_vision)
                    .await
                {
                    Ok(rows) => rows,
                    Err(issue) => {
                        self.pause_platform(run, &l.platform, issue)?;
                        continue;
                    }
                };
            for incoming in incoming {
                let mid = format!(
                    "im-{:x}",
                    Sha256::digest(format!("{}\0{}", l.id, incoming.id).as_bytes())
                );
                store_message(&self.r, mid, &l.id, "inbound", &incoming.text)?;
            }
            notify(&self.app, &self.c.id);
        }
        Ok(())
    }
    async fn analyze_pending(&self) -> Res<()> {
        let pending = self.r.acquisition_pending_leads(&self.c.id).map_err(err)?;
        if pending.is_empty() {
            return Ok(());
        }
        Analyst::load(&self.home)?;
        let mut failures = 0;
        for lid in pending {
            computer::check(&self.cancel)?;
            let l = lead(&self.r, &lid)?;
            let result = tokio::select! { _=self.cancel.cancelled()=>return Err("任务已停止".into()), r=analysis::analyze(&self.home,&self.r,l,&self.c)=>r };
            if let Err(e) = result {
                failures += 1;
                activity(&self.r, &self.c.id, Some(&lid), "analysis_error", &e)?;
            }
            notify(&self.app, &self.c.id);
        }
        if failures > 0 {
            return Err(format!("有 {failures} 位客户的分析未完成，详见执行记录。"));
        }
        Ok(())
    }
    async fn run(&self, mode: &str) -> Res<()> {
        let mut platforms = PlatformRun::default();
        if matches!(mode, "acquire" | "discover") {
            self.discover(&mut platforms).await?;
            self.analyze_pending().await?;
        }
        if matches!(mode, "acquire" | "outreach") {
            self.outreach(&mut platforms).await?;
        }
        if matches!(mode, "acquire" | "sync") {
            let synced = self.sync(&mut platforms).await;
            computer::check(&self.cancel)?;
            let analyzed = self.analyze_pending().await;
            synced.and(analyzed)?;
        } else if mode == "analyze_pending" {
            self.analyze_pending().await?;
        }
        platforms.finish()
    }
}
#[tauri::command]
pub(super) fn acquisition_start(app: AppHandle, campaign_id: String, mode: String) -> Res<Value> {
    if !matches!(
        mode.as_str(),
        "acquire" | "discover" | "outreach" | "sync" | "analyze_pending"
    ) {
        return Err("未知获客操作".into());
    }
    let r = db(&app)?;
    let c = campaign(&r, &campaign_id)?;
    let home = app_home(&app)?;
    if c.outreach.desktop_vision
        && (matches!(mode.as_str(), "acquire" | "discover" | "sync")
            || (mode == "outreach" && c.outreach.mode == "approved_auto"))
    {
        super::acquisition_desktop::load(&home)?;
    } else if matches!(mode.as_str(), "acquire" | "discover") {
        Analyst::load(&home)?;
    }
    if matches!(mode.as_str(), "acquire" | "outreach")
        && c.outreach.mode == "approved_auto"
        && c.outreach.approved_at.is_none()
    {
        return Err("请先批准当前话术与范围。".into());
    }
    let lease = Lease::claim(&campaign_id)?;
    if !r
        .acquisition_transition_campaign(&c.id, &c.status, "running", None)
        .map_err(err)?
    {
        return Err("任务状态已变化".into());
    }
    let w = Worker {
        app,
        home,
        r,
        c,
        cancel: lease.cancel.clone(),
    };
    tauri::async_runtime::spawn(async move {
        let result = async {
            w.log("start", &format!("启动 {mode}"))?;
            w.run(&mode).await
        }
        .await;
        let state = if w.cancel.is_cancelled() {
            "cancelled"
        } else if result.is_ok() {
            "completed"
        } else {
            "failed"
        };
        let issue = result
            .err()
            .map(|s| s.chars().take(4000).collect::<String>());
        if let Err(e) =
            w.r.acquisition_transition_campaign(&w.c.id, "running", state, issue.as_deref())
        {
            tracing::error!("acquisition finish: {e}");
        }
        let _ = activity(
            &w.r,
            &w.c.id,
            None,
            state,
            issue.as_deref().unwrap_or("任务完成"),
        );
        drop(lease);
        notify(&w.app, &w.c.id);
    });
    Ok(json!({"campaignId":campaign_id}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multi_platform_discovery_shares_slots_and_skips_only_paused_platforms() {
        let mut run = PlatformRun::default();
        run.0.insert("douyin".into(), "需要登录".into());
        let selected = [
            ("xiaohongshu", vec![1, 2, 3, 4]),
            ("douyin", vec![5]),
            ("weibo", vec![6, 7]),
        ];
        let queues = selected
            .into_iter()
            .filter(|(p, _)| run.available(p))
            .map(|(p, rows)| (p, rows.into_iter()))
            .collect();
        assert_eq!(
            alternate_platforms(queues)
                .map(|(_, value)| value)
                .collect::<Vec<_>>(),
            vec![1, 6, 2, 7, 3, 4]
        );
        assert!(run.finish().unwrap_err().contains("抖音：需要登录"));
    }
}
