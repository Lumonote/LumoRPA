use crate::{
    acquisition_scope,
    acquisition_store::{get_campaign, save_needs, validate_profile, validate_url},
    AcquisitionAnalysisBatch, AcquisitionLead, AcquisitionRequirement, AcquisitionSource, Repo,
    StorageError,
};
use chrono::Utc;
use rusqlite::{params, OptionalExtension, TransactionBehavior};

impl Repo {
    pub fn acquisition_add_source(
        &self,
        lead_id: &str,
        url: &str,
        excerpt: &str,
    ) -> Result<bool, StorageError> {
        if excerpt.trim().is_empty() || excerpt.chars().count() > 16000 || excerpt.contains('\0') {
            return Err(StorageError::Validation("invalid public evidence".into()));
        }
        let mut conn = self.inner.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (platform, status): (String, String) = tx.query_row(
            "SELECT platform,status FROM acquisition_leads WHERE id=?",
            [lead_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if status == "ignored" {
            return Ok(false);
        }
        validate_url(url, &platform)?;
        let now = Utc::now().timestamp_millis();
        let added = tx.execute("INSERT INTO acquisition_inputs(lead_id,source_url,excerpt,observed_at) VALUES(?,?,?,?) ON CONFLICT(lead_id,source_url,excerpt) DO NOTHING", params![lead_id,url,excerpt,now])? == 1;
        if added {
            tx.execute(
                "UPDATE acquisition_leads SET updated_at=MAX(updated_at+1,?) WHERE id=?",
                params![now, lead_id],
            )?;
        }
        tx.commit()?;
        Ok(added)
    }

    pub fn acquisition_sources(
        &self,
        lead_id: &str,
    ) -> Result<Vec<AcquisitionSource>, StorageError> {
        let conn = self.inner.lock();
        let mut stmt = conn.prepare("SELECT source_url,excerpt,observed_at FROM acquisition_inputs WHERE lead_id=? AND source_url IS NOT NULL ORDER BY seq DESC")?;
        let rows = stmt.query_map([lead_id], |r| {
            Ok(AcquisitionSource {
                source_url: r.get(0)?,
                evidence: r.get(1)?,
                observed_at: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn acquisition_pending_leads(
        &self,
        campaign_id: &str,
    ) -> Result<Vec<String>, StorageError> {
        let conn = self.inner.lock();
        let mut stmt = conn.prepare("SELECT l.id FROM acquisition_leads l JOIN acquisition_analysis_progress p ON p.lead_id=l.id WHERE l.campaign_id=? AND l.status!='ignored' AND EXISTS(SELECT 1 FROM acquisition_inputs i WHERE i.lead_id=l.id AND i.seq>p.consumed) ORDER BY l.id")?;
        let rows = stmt.query_map([campaign_id], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// An explicit analysis replays completed work, but resumes an unfinished queue.
    pub fn acquisition_prepare_manual_analysis(&self, id: &str) -> Result<(), StorageError> {
        self.inner.lock().execute("UPDATE acquisition_analysis_progress SET consumed=0,revision=revision+1 WHERE lead_id=? AND NOT EXISTS(SELECT 1 FROM acquisition_inputs WHERE lead_id=acquisition_analysis_progress.lead_id AND seq>acquisition_analysis_progress.consumed)", [id])?;
        Ok(())
    }

    /// Advance in bounded batches, including a small amount of earlier context.
    /// Every pending input appears in full before its cursor can advance.
    pub fn acquisition_analysis_batch(
        &self,
        id: &str,
    ) -> Result<Option<AcquisitionAnalysisBatch>, StorageError> {
        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        let progress: Option<(i64,i64,String)> = tx.query_row("SELECT p.consumed,p.revision,l.evidence FROM acquisition_analysis_progress p JOIN acquisition_leads l ON l.id=p.lead_id WHERE l.id=? AND l.status!='ignored'", [id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let Some((consumed, revision, initial)) = progress else {
            return Ok(None);
        };
        let mut stmt = tx.prepare("SELECT i.seq,COALESCE(i.excerpt,m.body) FROM acquisition_inputs i LEFT JOIN acquisition_messages m ON m.id=i.message_id WHERE i.lead_id=? AND i.seq>? ORDER BY i.seq LIMIT 6")?;
        let rows = stmt.query_map(params![id, consumed], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut through = consumed;
        let mut sources = vec![initial];
        if consumed > 0 {
            let last: String = tx.query_row("SELECT COALESCE(i.excerpt,m.body) FROM acquisition_inputs i LEFT JOIN acquisition_messages m ON m.id=i.message_id WHERE i.lead_id=? AND i.seq=?", params![id,consumed], |r|r.get(0))?;
            if !sources.contains(&last) {
                sources.push(last);
            }
        }
        for row in rows {
            let (seq, body) = row?;
            if through != consumed
                && sources.iter().map(String::len).sum::<usize>() + body.len() > 32000
            {
                break;
            }
            through = seq;
            if !sources.contains(&body) {
                sources.push(body);
            }
        }
        Ok((through != consumed).then(|| AcquisitionAnalysisBatch {
            sources,
            lead_id: id.into(),
            revision,
            through,
        }))
    }

    /// CAS and all derived records share a transaction. A competing analysis
    /// or scope edit invalidates this result; a later input remains queued.
    pub fn acquisition_finish_analysis(
        &self,
        batch: &AcquisitionAnalysisBatch,
        lead: &AcquisitionLead,
        needs: &[AcquisitionRequirement],
    ) -> Result<bool, StorageError> {
        if lead.id != batch.lead_id {
            return Err(StorageError::Validation("analysis lead mismatch".into()));
        }
        validate_profile(&lead.profile, &lead.tags, lead.intent_score)?;
        let mut conn = self.inner.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let campaign = get_campaign(&tx, &lead.campaign_id)?
            .ok_or_else(|| StorageError::NotFound("campaign".into()))?;
        if lead.profile.get("scope") != Some(&acquisition_scope(&campaign)) {
            return Ok(false);
        }
        if tx.execute("UPDATE acquisition_analysis_progress SET consumed=?,revision=revision+1 WHERE lead_id=? AND revision=? AND EXISTS(SELECT 1 FROM acquisition_leads WHERE id=? AND campaign_id=? AND status!='ignored')",params![batch.through,lead.id,batch.revision,lead.id,campaign.id])? != 1 { return Ok(false); }
        save_needs(&tx, &lead.id, needs)?;
        tx.execute("UPDATE acquisition_leads SET profile=?,tags=?,intent_score=?,updated_at=MAX(updated_at+1,?) WHERE id=?", params![serde_json::to_string(&lead.profile)?,serde_json::to_string(&lead.tags)?,lead.intent_score,Utc::now().timestamp_millis(),lead.id])?;
        tx.commit()?;
        Ok(true)
    }
}
