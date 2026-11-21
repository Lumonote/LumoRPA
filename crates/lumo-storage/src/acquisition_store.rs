//! Acquisition persistence. Discovery is append-only; explicit profile/status
//! updates preserve reviewed data. Message reservation is the send boundary.
use crate::error::SendBlock;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{
    AcquisitionActivity, AcquisitionCampaign, AcquisitionLead, AcquisitionMessage,
    AcquisitionOutreachPolicy, AcquisitionRequirement, Repo, StorageError,
};

const CAMPAIGN_SELECT: &str = "SELECT id,name,product,target_audience,keywords,platforms,max_leads,status,created_at,updated_at,last_error,outreach FROM acquisition_campaigns";
const LEAD_SELECT: &str = "SELECT id,campaign_id,platform,platform_user_id,display_name,profile_url,source_url,evidence,bio,status,intent_score,tags,profile,created_at,updated_at FROM acquisition_leads";
const MESSAGE_SELECT: &str =
    "SELECT id,lead_id,direction,body,status,created_at,updated_at,error FROM acquisition_messages";
const REQUIREMENT_SELECT: &str =
    "SELECT id,lead_id,summary,category,urgency,evidence,created_at FROM acquisition_requirements";

/// Render only the three supported placeholders in a single pass, so untrusted
/// profile text cannot inject a second template expansion.
pub fn acquisition_render_message(
    campaign: &AcquisitionCampaign,
    lead: &AcquisitionLead,
) -> String {
    let mut remaining = campaign.outreach.message_template.as_str();
    let mut result = String::new();
    while let Some(start) = remaining.find("{{") {
        result.push_str(&remaining[..start]);
        remaining = &remaining[start..];
        let Some(end) = remaining.find("}}") else {
            break;
        };
        let token = &remaining[..end + 2];
        result.push_str(match token {
            "{{name}}" => &lead.display_name,
            "{{product}}" => &campaign.product,
            "{{audience}}" => &campaign.target_audience,
            _ => token,
        });
        remaining = &remaining[end + 2..];
    }
    result.push_str(remaining);
    result.trim().to_string()
}

impl Repo {
    /// New campaigns always start as drafts without outbound authorization.
    /// Editing scope invalidates authorization. Lifecycle changes use CAS.
    pub fn acquisition_save_campaign(
        &self,
        input: &AcquisitionCampaign,
    ) -> Result<AcquisitionCampaign, StorageError> {
        validate_campaign(input)?;
        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        let previous = get_campaign(&tx, &input.id)?;
        let now = Utc::now().timestamp_millis();
        let mut campaign = input.clone();
        campaign.outreach.approved_at = None;
        campaign.last_error = None;
        if let Some(previous) = previous {
            if previous.status == "running" {
                return invalid("cannot edit a running campaign");
            }
            campaign.created_at = previous.created_at;
            campaign.updated_at = now.max(previous.updated_at.saturating_add(1));
            campaign.status = previous.status.clone();
            campaign.last_error = previous.last_error.clone();
            let mut prior_policy = previous.outreach.clone();
            prior_policy.approved_at = None;
            if acquisition_scope(&previous) != acquisition_scope(&campaign) {
                tx.execute("UPDATE acquisition_leads SET profile=json_remove(profile,'$.scope'),intent_score=0 WHERE campaign_id=?", [&campaign.id])?;
                tx.execute("UPDATE acquisition_analysis_progress SET consumed=0,revision=revision+1 WHERE lead_id IN(SELECT id FROM acquisition_leads WHERE campaign_id=?)", [&campaign.id])?;
            }
            if previous.product == campaign.product
                && previous.target_audience == campaign.target_audience
                && previous.keywords == campaign.keywords
                && previous.platforms == campaign.platforms
                && previous.max_leads == campaign.max_leads
                && prior_policy == campaign.outreach
            {
                campaign.outreach.approved_at = previous.outreach.approved_at;
            }
        } else {
            campaign.created_at = now;
            campaign.updated_at = now;
            campaign.status = "draft".into();
        }
        tx.execute(
            "INSERT INTO acquisition_campaigns(id,name,product,target_audience,keywords,platforms,max_leads,status,created_at,updated_at,last_error,outreach) VALUES (?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET name=excluded.name,product=excluded.product,target_audience=excluded.target_audience,keywords=excluded.keywords,platforms=excluded.platforms,max_leads=excluded.max_leads,updated_at=excluded.updated_at,outreach=excluded.outreach",
            params![campaign.id,campaign.name,campaign.product,campaign.target_audience,serde_json::to_string(&campaign.keywords)?,serde_json::to_string(&campaign.platforms)?,campaign.max_leads,campaign.status,campaign.created_at,campaign.updated_at,campaign.last_error,serde_json::to_string(&campaign.outreach)?],
        )?;
        tx.commit()?;
        Ok(campaign)
    }

    pub fn acquisition_get_campaign(
        &self,
        id: &str,
    ) -> Result<Option<AcquisitionCampaign>, StorageError> {
        get_campaign(&self.inner.lock(), id)
    }

    pub fn acquisition_list_campaigns(&self) -> Result<Vec<AcquisitionCampaign>, StorageError> {
        let conn = self.inner.lock();
        let mut stmt = conn.prepare(&format!("{CAMPAIGN_SELECT} ORDER BY updated_at DESC,id"))?;
        let rows = stmt.query_map([], campaign_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn acquisition_transition_campaign(
        &self,
        id: &str,
        expected_status: &str,
        status: &str,
        error: Option<&str>,
    ) -> Result<bool, StorageError> {
        validate_campaign_status(expected_status)?;
        validate_campaign_status(status)?;
        optional_text(error, "campaign error", 8000)?;
        let allowed = matches!(
            (expected_status, status),
            ("draft" | "completed" | "failed" | "cancelled", "running")
                | ("running", "completed" | "failed" | "cancelled")
        );
        if !allowed {
            return invalid("invalid campaign status transition");
        }
        Ok(self.inner.lock().execute("UPDATE acquisition_campaigns SET status=?,last_error=?,updated_at=MAX(updated_at+1,?) WHERE id=? AND status=?",
            params![status,error,Utc::now().timestamp_millis(),id,expected_status])? == 1)
    }

    /// The caller supplies the exact version the user reviewed.
    pub fn acquisition_approve_outreach(
        &self,
        id: &str,
        expected_updated_at: i64,
    ) -> Result<bool, StorageError> {
        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        let Some(mut campaign) = get_campaign(&tx, id)? else {
            return Ok(false);
        };
        if campaign.updated_at != expected_updated_at {
            return Ok(false);
        }
        if campaign.status == "running" {
            return invalid("cannot approve a running campaign");
        }
        if campaign.outreach.mode != "approved_auto" {
            return invalid("campaign is not configured for automatic outreach");
        }
        text(
            &campaign.outreach.message_template,
            "message template",
            3000,
            true,
        )?;
        let now = Utc::now().timestamp_millis();
        campaign.outreach.approved_at = Some(now);
        tx.execute(
            "UPDATE acquisition_campaigns SET outreach=?,updated_at=? WHERE id=?",
            params![
                serde_json::to_string(&campaign.outreach)?,
                now.max(expected_updated_at.saturating_add(1)),
                id
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Rediscovery returns the existing record, retaining reviewed profile,
    /// status, source and evidence. Use explicit updates for curated fields.
    pub fn acquisition_upsert_lead(
        &self,
        lead: &AcquisitionLead,
    ) -> Result<AcquisitionLead, StorageError> {
        self.acquisition_store_assessed_lead(lead, &[])
    }

    pub fn acquisition_store_assessed_lead(
        &self,
        lead: &AcquisitionLead,
        needs: &[AcquisitionRequirement],
    ) -> Result<AcquisitionLead, StorageError> {
        validate_lead(lead)?;
        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        let campaign = get_campaign(&tx, &lead.campaign_id)?
            .ok_or_else(|| StorageError::NotFound("campaign".into()))?;
        if !campaign.platforms.contains(&lead.platform) {
            return invalid("lead platform is outside campaign scope");
        }
        let existing = tx
            .query_row(
                &format!("{LEAD_SELECT} WHERE campaign_id=? AND platform=? AND platform_user_id=?"),
                params![lead.campaign_id, lead.platform, lead.platform_user_id],
                lead_row,
            )
            .optional()?;
        if let Some(existing) = existing {
            return Ok(existing);
        }
        let count: u32 = tx.query_row(
            "SELECT COUNT(*) FROM acquisition_leads WHERE campaign_id=?",
            [&lead.campaign_id],
            |row| row.get(0),
        )?;
        if count >= campaign.max_leads {
            return invalid("campaign lead limit reached");
        }
        let mut stored = lead.clone();
        stored.created_at = Utc::now().timestamp_millis();
        stored.updated_at = stored.created_at;
        if identity_ignored(&tx, lead)? {
            stored.status = "ignored".into();
        }
        tx.execute("INSERT INTO acquisition_leads(id,campaign_id,platform,platform_user_id,display_name,profile_url,source_url,evidence,bio,status,intent_score,tags,profile,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![stored.id,stored.campaign_id,stored.platform,stored.platform_user_id,stored.display_name,stored.profile_url,stored.source_url,stored.evidence,stored.bio,stored.status,stored.intent_score,serde_json::to_string(&stored.tags)?,serde_json::to_string(&stored.profile)?,stored.created_at,stored.updated_at])?;
        if stored.profile.get("scope") == Some(&acquisition_scope(&campaign)) {
            save_needs(&tx, &stored.id, needs)?;
            tx.execute("UPDATE acquisition_analysis_progress SET consumed=(SELECT MAX(seq) FROM acquisition_inputs WHERE lead_id=?1) WHERE lead_id=?1", [&stored.id])?;
        }
        tx.commit()?;
        Ok(stored)
    }

    pub fn acquisition_get_lead(&self, id: &str) -> Result<Option<AcquisitionLead>, StorageError> {
        get_lead(&self.inner.lock(), id)
    }

    pub fn acquisition_list_leads(
        &self,
        campaign_id: Option<&str>,
    ) -> Result<Vec<AcquisitionLead>, StorageError> {
        let conn = self.inner.lock();
        let mut stmt = conn.prepare(&format!("{LEAD_SELECT} WHERE (?1 IS NULL OR campaign_id=?1) ORDER BY intent_score DESC,updated_at DESC,id"))?;
        let rows = stmt.query_map([campaign_id], lead_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Ignoring an identity applies across campaigns and revokes unsent drafts.
    pub fn acquisition_update_lead_status(
        &self,
        id: &str,
        status: &str,
    ) -> Result<bool, StorageError> {
        validate_lead_status(status)?;
        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        let Some(lead) = get_lead(&tx, id)? else {
            return Ok(false);
        };
        let now = Utc::now().timestamp_millis();
        if status == "ignored" {
            tx.execute("UPDATE acquisition_leads SET status='ignored',updated_at=MAX(updated_at+1,?) WHERE platform=? AND platform_user_id=?",params![now,lead.platform,lead.platform_user_id])?;
            tx.execute("UPDATE acquisition_messages SET status='failed',error='recipient ignored',updated_at=MAX(updated_at+1,?) WHERE status IN ('draft','approved') AND lead_id IN (SELECT id FROM acquisition_leads WHERE platform=? AND platform_user_id=?)",params![now,lead.platform,lead.platform_user_id])?;
        } else {
            tx.execute(
                "UPDATE acquisition_leads SET status=?,updated_at=MAX(updated_at+1,?) WHERE id=?",
                params![status, now, id],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    pub fn acquisition_update_lead_profile(
        &self,
        id: &str,
        profile: &Value,
        tags: &[String],
        intent_score: u8,
    ) -> Result<bool, StorageError> {
        validate_profile(profile, tags, intent_score)?;
        Ok(self.inner.lock().execute("UPDATE acquisition_leads SET profile=?,tags=?,intent_score=?,updated_at=MAX(updated_at+1,?) WHERE id=?",
            params![serde_json::to_string(profile)?,serde_json::to_string(tags)?,intent_score,Utc::now().timestamp_millis(),id])? == 1)
    }

    /// Inbound observations enter as received; outbound messages enter as
    /// drafts. A confirmed failure permits a new draft, retaining the old attempt.
    pub fn acquisition_add_message(
        &self,
        input: &AcquisitionMessage,
    ) -> Result<AcquisitionMessage, StorageError> {
        let mut conn = self.inner.lock();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let m = Self::insert_message(&tx, input)?;
        tx.commit()?;
        Ok(m)
    }

    fn insert_message(
        tx: &Connection,
        input: &AcquisitionMessage,
    ) -> Result<AcquisitionMessage, StorageError> {
        text(&input.id, "message id", 200, true)?;
        text(&input.body, "message body", 8000, true)?;
        if !matches!(
            (input.direction.as_str(), input.status.as_str()),
            ("inbound", "received") | ("outbound", "draft")
        ) {
            return invalid(
                "new outbound messages must be drafts; inbound messages must be received",
            );
        }
        let lead =
            get_lead(tx, &input.lead_id)?.ok_or_else(|| StorageError::NotFound("lead".into()))?;
        if input.direction == "outbound" && identity_ignored(tx, &lead)? {
            return Err(SendBlock::Recipient("客户已忽略").into());
        }
        let previous = if input.direction == "inbound" {
            get_message(tx, &input.id)?
        } else {
            tx.query_row(
                &format!("{MESSAGE_SELECT} WHERE lead_id=? AND direction='outbound' AND body=? AND status!='failed'"),
                params![input.lead_id, input.body],
                message_row,
            )
            .optional()?
        };
        if let Some(previous) = previous {
            return Ok(previous);
        }
        let mut stored = input.clone();
        stored.created_at = Utc::now().timestamp_millis();
        stored.updated_at = stored.created_at;
        stored.error = None;
        tx.execute("INSERT INTO acquisition_messages(id,lead_id,direction,body,status,created_at,updated_at,error) VALUES (?,?,?,?,?,?,?,NULL)", params![stored.id,stored.lead_id,stored.direction,stored.body,stored.status,stored.created_at,stored.updated_at])?;
        if stored.direction == "inbound" {
            tx.execute("UPDATE acquisition_leads SET status='replied',updated_at=MAX(updated_at+1,?) WHERE id=? AND status NOT IN ('ignored','converted')",params![stored.created_at,stored.lead_id])?;
        }
        Ok(stored)
    }

    pub fn acquisition_get_message(
        &self,
        id: &str,
    ) -> Result<Option<AcquisitionMessage>, StorageError> {
        get_message(&self.inner.lock(), id)
    }

    pub fn acquisition_list_messages(
        &self,
        lead_id: &str,
    ) -> Result<Vec<AcquisitionMessage>, StorageError> {
        let conn = self.inner.lock();
        let mut stmt = conn.prepare(&format!(
            "{MESSAGE_SELECT} WHERE lead_id=? ORDER BY created_at,id"
        ))?;
        let rows = stmt.query_map([lead_id], message_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Compare-and-swap terminal outcomes. Entering `sending` must go through
    /// reserve_send so a direct state transition cannot bypass the send policy.
    pub fn acquisition_transition_message(
        &self,
        id: &str,
        expected_status: &str,
        status: &str,
        error: Option<&str>,
    ) -> Result<bool, StorageError> {
        optional_text(error, "message error", 8000)?;
        let allowed = matches!(
            (expected_status, status),
            ("draft", "approved" | "failed")
                | ("approved", "draft" | "failed")
                | ("sending", "sent" | "failed" | "uncertain")
                | ("uncertain", "sent" | "failed")
        );
        if !allowed {
            return invalid("invalid message transition; sending requires reservation");
        }
        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        let now = Utc::now().timestamp_millis();
        let changed = tx.execute("UPDATE acquisition_messages SET status=?,error=?,updated_at=MAX(updated_at+1,?) WHERE id=? AND status=? AND direction='outbound' AND (?!='approved' OR NOT EXISTS(SELECT 1 FROM acquisition_leads l JOIN acquisition_leads blocked ON l.platform=blocked.platform AND l.platform_user_id=blocked.platform_user_id WHERE l.id=acquisition_messages.lead_id AND blocked.status='ignored'))",params![status,error,now,id,expected_status,status])? == 1;
        if changed && status == "sent" {
            tx.execute("UPDATE acquisition_leads SET status='contacted',updated_at=MAX(updated_at+1,?) WHERE id=(SELECT lead_id FROM acquisition_messages WHERE id=?) AND status IN ('new','qualified')", params![now,id])?;
        }
        tx.commit()?;
        Ok(changed)
    }

    /// Atomically claim a send and enforce approval, identity suppression,
    /// campaign-wide UTC daily allowance and interval. Failed/uncertain sends
    /// remain attempts; they cannot be reset and retried through this API.
    pub fn acquisition_reserve_send(
        &self,
        message_id: &str,
        automatic: bool,
    ) -> Result<AcquisitionMessage, StorageError> {
        let mut conn = self.inner.lock();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let reserved = Self::reserve(&tx, message_id, automatic)?;
        tx.commit()?;
        Ok(reserved)
    }

    pub fn acquisition_reserve_first_contact(
        &self,
        draft: &AcquisitionMessage,
    ) -> Result<AcquisitionMessage, StorageError> {
        let mut conn = self.inner.lock();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let draft = Self::insert_message(&tx, draft)?;
        let reserved = Self::reserve(&tx, &draft.id, true)?;
        tx.commit()?;
        Ok(reserved)
    }
    fn reserve(
        tx: &Connection,
        message_id: &str,
        automatic: bool,
    ) -> Result<AcquisitionMessage, StorageError> {
        let mut message =
            get_message(tx, message_id)?.ok_or_else(|| StorageError::NotFound("message".into()))?;
        if message.direction != "outbound"
            || !matches!(message.status.as_str(), "draft" | "approved")
        {
            return Err(SendBlock::Recipient("该消息已有发送记录").into());
        }
        let lead =
            get_lead(tx, &message.lead_id)?.ok_or_else(|| StorageError::NotFound("lead".into()))?;
        let campaign = get_campaign(tx, &lead.campaign_id)?
            .ok_or_else(|| StorageError::NotFound("campaign".into()))?;
        if identity_ignored(tx, &lead)? {
            return Err(SendBlock::Recipient("客户已忽略").into());
        }
        if automatic {
            if !campaign.platforms.contains(&lead.platform)
                || !matches!(lead.status.as_str(), "new" | "qualified")
                || lead.profile.get("scope") != Some(&acquisition_scope(&campaign))
                || lead.profile.get("matched").and_then(Value::as_bool) != Some(true)
            {
                return Err(SendBlock::Recipient("客户已不在批准范围内").into());
            }
            let pending: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM acquisition_inputs i JOIN acquisition_analysis_progress p ON p.lead_id=i.lead_id WHERE i.lead_id=? AND i.seq>p.consumed)", [&lead.id], |r|r.get(0))?;
            if pending {
                return Err(SendBlock::Recipient("客户有新资料待分析").into());
            }
            if campaign.outreach.mode != "approved_auto" || campaign.outreach.approved_at.is_none()
            {
                return invalid("automatic outreach scope has not been approved");
            }
            if lead.intent_score < campaign.outreach.min_intent_score {
                return Err(SendBlock::Recipient("客户未达到批准的匹配分").into());
            }
            if message.body != acquisition_render_message(&campaign, &lead) {
                return invalid("message does not match the approved template");
            }
        }
        let (pending, sent): (i64,i64) = tx.query_row("SELECT COALESCE(SUM(m.status IN ('sending','uncertain')),0),COALESCE(SUM(m.status='sent'),0) FROM acquisition_messages m JOIN acquisition_leads l ON l.id=m.lead_id WHERE l.platform=? AND l.platform_user_id=? AND m.direction='outbound'",params![lead.platform,lead.platform_user_id],|row|Ok((row.get(0)?,row.get(1)?)))?;
        if pending > 0 {
            return Err(SendBlock::Recipient("该平台客户已有发送结果待核实").into());
        }
        if sent > 0 {
            let replied: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM acquisition_messages m JOIN acquisition_leads l ON l.id=m.lead_id WHERE l.platform=? AND l.platform_user_id=? AND m.direction='inbound')",params![lead.platform,lead.platform_user_id],|row|row.get(0))?;
            if automatic || !replied {
                return Err(SendBlock::Recipient("已联系过该平台客户").into());
            }
        }
        let now = Utc::now().timestamp_millis();
        let day_start = now.div_euclid(86_400_000) * 86_400_000;
        let (daily,last): (i64,Option<i64>) = tx.query_row("SELECT COALESCE(SUM(m.attempted_at>=?),0),MAX(m.attempted_at) FROM acquisition_messages m JOIN acquisition_leads l ON l.id=m.lead_id WHERE l.campaign_id=? AND m.attempted_at IS NOT NULL",params![day_start,campaign.id],|row|Ok((row.get(0)?,row.get(1)?)))?;
        if daily >= i64::from(campaign.outreach.max_messages_per_day) {
            return Err(SendBlock::DailyLimit.into());
        }
        if let Some(ms) = last
            .map(|last| {
                last.saturating_add(i64::from(campaign.outreach.min_interval_seconds) * 1000)
                    .saturating_sub(now)
            })
            .filter(|ms| *ms > 0)
        {
            return Err(SendBlock::Interval(ms as u64).into());
        }
        message.status = "sending".into();
        message.error = None;
        message.updated_at = now.max(message.updated_at.saturating_add(1));
        tx.execute("UPDATE acquisition_messages SET status='sending',error=NULL,updated_at=?,attempted_at=? WHERE id=?",params![message.updated_at,now,message.id])?;
        Ok(message)
    }

    pub fn acquisition_add_requirement(
        &self,
        input: &AcquisitionRequirement,
    ) -> Result<AcquisitionRequirement, StorageError> {
        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        let stored = insert_requirement(&tx, input)?;
        tx.commit()?;
        Ok(stored)
    }
}

pub(super) fn save_needs(
    conn: &Connection,
    lead_id: &str,
    needs: &[AcquisitionRequirement],
) -> Result<(), StorageError> {
    for n in needs {
        if n.lead_id != lead_id {
            return invalid("requirement belongs to another lead");
        }
        insert_requirement(conn, n)?;
    }
    Ok(())
}
fn insert_requirement(
    tx: &Connection,
    input: &AcquisitionRequirement,
) -> Result<AcquisitionRequirement, StorageError> {
    text(&input.id, "requirement id", 200, true)?;
    text(&input.summary, "requirement summary", 4000, true)?;
    text(&input.category, "requirement category", 100, true)?;
    text(&input.urgency, "requirement urgency", 100, true)?;
    text(&input.evidence, "requirement evidence", 16000, true)?;
    let previous = tx
        .query_row(
            &format!("{REQUIREMENT_SELECT} WHERE lead_id=? AND summary=? AND evidence=?"),
            params![input.lead_id, input.summary, input.evidence],
            requirement_row,
        )
        .optional()?;
    if let Some(previous) = previous {
        return Ok(previous);
    }
    let mut stored = input.clone();
    stored.created_at = Utc::now().timestamp_millis();
    tx.execute("INSERT INTO acquisition_requirements(id,lead_id,summary,category,urgency,evidence,created_at) VALUES (?,?,?,?,?,?,?)",params![stored.id,stored.lead_id,stored.summary,stored.category,stored.urgency,stored.evidence,stored.created_at])?;
    Ok(stored)
}

impl Repo {
    pub fn acquisition_list_requirements(
        &self,
        lead_id: Option<&str>,
    ) -> Result<Vec<AcquisitionRequirement>, StorageError> {
        let conn = self.inner.lock();
        let mut stmt = conn.prepare(&format!(
            "{REQUIREMENT_SELECT} WHERE (?1 IS NULL OR lead_id=?1) ORDER BY created_at DESC,id"
        ))?;
        let rows = stmt.query_map([lead_id], requirement_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn acquisition_add_activity(
        &self,
        activity: &AcquisitionActivity,
    ) -> Result<(), StorageError> {
        text(&activity.id, "activity id", 200, true)?;
        text(&activity.kind, "activity kind", 100, true)?;
        text(&activity.detail, "activity detail", 32000, true)?;
        let conn = self.inner.lock();
        if let (Some(campaign_id), Some(lead_id)) = (&activity.campaign_id, &activity.lead_id) {
            let belongs: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM acquisition_leads WHERE id=? AND campaign_id=?)",
                params![lead_id, campaign_id],
                |row| row.get(0),
            )?;
            if !belongs {
                return invalid("activity lead does not belong to campaign");
            }
        }
        conn.execute("INSERT INTO acquisition_activity(id,campaign_id,lead_id,kind,detail,created_at) VALUES (?,?,?,?,?,?)",params![activity.id,activity.campaign_id,activity.lead_id,activity.kind,activity.detail,Utc::now().timestamp_millis()])?;
        Ok(())
    }

    pub fn acquisition_list_activity(
        &self,
        campaign_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<AcquisitionActivity>, StorageError> {
        let conn = self.inner.lock();
        let mut stmt = conn.prepare("SELECT id,campaign_id,lead_id,kind,detail,created_at FROM acquisition_activity WHERE (?1 IS NULL OR campaign_id=?1) ORDER BY created_at DESC,id DESC LIMIT ?2")?;
        let rows = stmt.query_map(params![campaign_id, limit.min(1000)], |row| {
            Ok(AcquisitionActivity {
                id: row.get(0)?,
                campaign_id: row.get(1)?,
                lead_id: row.get(2)?,
                kind: row.get(3)?,
                detail: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Run once during application startup, never while a worker is active.
    /// Uncertain sends require result review; startup never retries them.
    pub fn acquisition_recover_interrupted(&self) -> Result<(), StorageError> {
        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        let now = Utc::now().timestamp_millis();
        tx.execute("UPDATE acquisition_campaigns SET status='failed',last_error='Application stopped before completion',updated_at=MAX(updated_at+1,?) WHERE status='running'",[now])?;
        tx.execute("UPDATE acquisition_messages SET status='uncertain',error='Application stopped during sending; verify the conversation before retrying',updated_at=MAX(updated_at+1,?) WHERE status='sending'",[now])?;
        tx.commit()?;
        Ok(())
    }
}

pub(super) fn get_campaign(
    conn: &Connection,
    id: &str,
) -> Result<Option<AcquisitionCampaign>, StorageError> {
    conn.query_row(&format!("{CAMPAIGN_SELECT} WHERE id=?"), [id], campaign_row)
        .optional()
        .map_err(Into::into)
}
fn get_lead(conn: &Connection, id: &str) -> Result<Option<AcquisitionLead>, StorageError> {
    conn.query_row(&format!("{LEAD_SELECT} WHERE id=?"), [id], lead_row)
        .optional()
        .map_err(Into::into)
}

pub fn acquisition_scope(c: &AcquisitionCampaign) -> Value {
    serde_json::json!({"product":c.product,"targetAudience":c.target_audience,"platforms":c.platforms,"keywords":c.keywords})
}
fn get_message(conn: &Connection, id: &str) -> Result<Option<AcquisitionMessage>, StorageError> {
    conn.query_row(&format!("{MESSAGE_SELECT} WHERE id=?"), [id], message_row)
        .optional()
        .map_err(Into::into)
}
fn identity_ignored(conn: &Connection, lead: &AcquisitionLead) -> Result<bool, StorageError> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM acquisition_leads WHERE platform=? AND platform_user_id=? AND status='ignored')",params![lead.platform,lead.platform_user_id],|row|row.get(0)).map_err(Into::into)
}
fn json<T: DeserializeOwned>(row: &Row<'_>, index: usize) -> rusqlite::Result<T> {
    let raw: String = row.get(index)?;
    serde_json::from_str(&raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}
fn campaign_row(row: &Row<'_>) -> rusqlite::Result<AcquisitionCampaign> {
    Ok(AcquisitionCampaign {
        id: row.get(0)?,
        name: row.get(1)?,
        product: row.get(2)?,
        target_audience: row.get(3)?,
        keywords: json(row, 4)?,
        platforms: json(row, 5)?,
        max_leads: row.get(6)?,
        status: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        last_error: row.get(10)?,
        outreach: json(row, 11)?,
    })
}
fn lead_row(row: &Row<'_>) -> rusqlite::Result<AcquisitionLead> {
    Ok(AcquisitionLead {
        id: row.get(0)?,
        campaign_id: row.get(1)?,
        platform: row.get(2)?,
        platform_user_id: row.get(3)?,
        display_name: row.get(4)?,
        profile_url: row.get(5)?,
        source_url: row.get(6)?,
        evidence: row.get(7)?,
        bio: row.get(8)?,
        status: row.get(9)?,
        intent_score: row.get(10)?,
        tags: json(row, 11)?,
        profile: json(row, 12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}
fn message_row(row: &Row<'_>) -> rusqlite::Result<AcquisitionMessage> {
    Ok(AcquisitionMessage {
        id: row.get(0)?,
        lead_id: row.get(1)?,
        direction: row.get(2)?,
        body: row.get(3)?,
        status: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        error: row.get(7)?,
    })
}
fn requirement_row(row: &Row<'_>) -> rusqlite::Result<AcquisitionRequirement> {
    Ok(AcquisitionRequirement {
        id: row.get(0)?,
        lead_id: row.get(1)?,
        summary: row.get(2)?,
        category: row.get(3)?,
        urgency: row.get(4)?,
        evidence: row.get(5)?,
        created_at: row.get(6)?,
    })
}
fn invalid<T>(message: &str) -> Result<T, StorageError> {
    Err(StorageError::Validation(message.into()))
}
fn text(value: &str, name: &str, max: usize, required: bool) -> Result<(), StorageError> {
    if (required && value.trim().is_empty()) || value.chars().count() > max || value.contains('\0')
    {
        return invalid(&format!(
            "{name} must contain {}..{max} characters without NUL",
            if required { 1 } else { 0 }
        ));
    }
    Ok(())
}
fn optional_text(value: Option<&str>, name: &str, max: usize) -> Result<(), StorageError> {
    if let Some(value) = value {
        text(value, name, max, false)?;
    }
    Ok(())
}
fn validate_campaign_status(status: &str) -> Result<(), StorageError> {
    if !matches!(
        status,
        "draft" | "running" | "completed" | "failed" | "cancelled"
    ) {
        return invalid("invalid campaign status");
    }
    Ok(())
}
fn validate_lead_status(status: &str) -> Result<(), StorageError> {
    if !matches!(
        status,
        "new" | "qualified" | "contacted" | "replied" | "converted" | "ignored"
    ) {
        return invalid("invalid lead status");
    }
    Ok(())
}
fn platform(value: &str) -> Result<(), StorageError> {
    if !matches!(value, "xiaohongshu" | "douyin" | "weibo") {
        return invalid("unsupported acquisition platform");
    }
    Ok(())
}
fn validate_campaign(c: &AcquisitionCampaign) -> Result<(), StorageError> {
    text(&c.id, "campaign id", 200, true)?;
    text(&c.name, "campaign name", 200, true)?;
    text(&c.product, "product", 4000, true)?;
    text(&c.target_audience, "target audience", 4000, true)?;
    if c.keywords.is_empty() || c.keywords.len() > 30 {
        return invalid("provide 1..30 keywords");
    }
    for keyword in &c.keywords {
        text(keyword, "keyword", 100, true)?;
    }
    if c.platforms.is_empty() || c.platforms.len() > 3 {
        return invalid("select 1..3 platforms");
    }
    for p in &c.platforms {
        platform(p)?;
    }
    if !(1..=200).contains(&c.max_leads) {
        return invalid("maxLeads must be 1..200");
    }
    validate_campaign_status(&c.status)?;
    validate_policy(&c.outreach)
}
fn validate_policy(p: &AcquisitionOutreachPolicy) -> Result<(), StorageError> {
    if !matches!(p.mode.as_str(), "draft" | "approved_auto") {
        return invalid("invalid outreach mode");
    }
    text(
        &p.message_template,
        "message template",
        3000,
        p.mode == "approved_auto",
    )?;
    let mut template = p.message_template.as_str();
    while let Some(start) = template.find("{{") {
        template = &template[start..];
        let Some(end) = template.find("}}") else {
            return invalid("unclosed template placeholder");
        };
        if !matches!(
            &template[..end + 2],
            "{{name}}" | "{{product}}" | "{{audience}}"
        ) {
            return invalid(
                "only {{name}}, {{product}} and {{audience}} placeholders are supported",
            );
        }
        template = &template[end + 2..];
    }
    if !(1..=50).contains(&p.max_messages_per_run)
        || !(1..=200).contains(&p.max_messages_per_day)
        || p.max_messages_per_run > p.max_messages_per_day
    {
        return invalid(
            "outreach limits require 1..50 per run and 1..200 per day, with run <= day",
        );
    }
    if !(10..=86400).contains(&p.min_interval_seconds) {
        return invalid("message interval must be 10..86400 seconds");
    }
    if p.min_intent_score > 100 {
        return invalid("minIntentScore must be 0..100");
    }
    Ok(())
}
pub(super) fn validate_url(value: &str, platform: &str) -> Result<(), StorageError> {
    text(value, "source URL", 4096, true)?;
    let url = url::Url::parse(value)
        .map_err(|_| StorageError::Validation("invalid source URL".into()))?;
    let host = url.host_str().unwrap_or("");
    let domains: &[&str] = match platform {
        "xiaohongshu" => &["xiaohongshu.com", "xhslink.com"],
        "douyin" => &["douyin.com"],
        "weibo" => &["weibo.com", "weibo.cn"],
        _ => &[],
    };
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|port| port != 443)
        || !domains
            .iter()
            .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
    {
        return invalid("source URL must use HTTPS on the selected platform's domain");
    }
    Ok(())
}
pub(super) fn validate_profile(
    profile: &Value,
    tags: &[String],
    score: u8,
) -> Result<(), StorageError> {
    if score > 100 {
        return invalid("intentScore must be 0..100");
    }
    if !profile.is_object() || serde_json::to_vec(profile)?.len() > 64000 {
        return invalid("profile must be an object of at most 64KB");
    }
    if tags.len() > 30 {
        return invalid("at most 30 profile tags are allowed");
    }
    for tag in tags {
        text(tag, "profile tag", 100, true)?;
    }
    Ok(())
}
fn validate_lead(l: &AcquisitionLead) -> Result<(), StorageError> {
    text(&l.id, "lead id", 200, true)?;
    text(&l.campaign_id, "campaign id", 200, true)?;
    platform(&l.platform)?;
    text(&l.platform_user_id, "platform user id", 300, true)?;
    text(&l.display_name, "display name", 300, true)?;
    validate_url(&l.profile_url, &l.platform)?;
    validate_url(&l.source_url, &l.platform)?;
    text(&l.evidence, "lead evidence", 16000, true)?;
    text(&l.bio, "bio", 8000, false)?;
    validate_lead_status(&l.status)?;
    validate_profile(&l.profile, &l.tags, l.intent_score)
}
