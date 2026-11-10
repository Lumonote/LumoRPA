use crate::{
    InvocationAttemptRow, InvocationFlowRunRow, InvocationUsageResolution, Repo, StorageError,
};
use chrono::{TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension};

impl Repo {
    /// Must succeed before dispatch. Reusing an attempt key is forbidden so a
    /// retry can never overwrite a previous failed call's metered usage.
    pub fn begin_invocation_attempt(&self, row: &InvocationAttemptRow) -> Result<(), StorageError> {
        if row.state != "running" || row.usage_state != "pending" {
            return Err(StorageError::NotFound(
                "new invocation must be running and pending".into(),
            ));
        }
        let conn = self.inner.lock();
        conn.execute(
            "INSERT INTO agent_invocation_attempts(run_id,node_id,attempt,capability_id,source_kind,flow_run_id,state,usage_state,budget_sensitive,started_at) VALUES(?,?,?,?,?,?,'running','pending',?,?)",
            params![row.run_id,row.node_id,row.attempt,row.capability_id,row.source_kind,row.flow_run_id,row.budget_sensitive,row.started_at.timestamp_millis()],
        )?;
        Ok(())
    }

    pub fn set_invocation_budget_sensitive(
        &self,
        run_id: &str,
        node_id: &str,
        attempt: u32,
        sensitive: bool,
    ) -> Result<(), StorageError> {
        self.inner.lock().execute(
            "UPDATE agent_invocation_attempts SET budget_sensitive=?,revision=revision+1 WHERE run_id=? AND node_id=? AND attempt=? AND state='running'",
            params![sensitive,run_id,node_id,attempt],
        )?;
        Ok(())
    }

    pub fn finish_invocation_attempt(
        &self,
        row: &InvocationAttemptRow,
    ) -> Result<(), StorageError> {
        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        let changed = tx.execute(
            "UPDATE agent_invocation_attempts SET state=?,usage_state=?,tokens_used=?,cost_usd_micro=?,duration_ms=?,finished_at=?,error=?,revision=revision+1 WHERE run_id=? AND node_id=? AND attempt=? AND state='running'",
            params![row.state,row.usage_state,sqlite_number(row.tokens_used)?,sqlite_number(row.cost_usd_micro)?,sqlite_number(Some(row.duration_ms))?,row.finished_at.map(|t|t.timestamp_millis()),row.error,row.run_id,row.node_id,row.attempt],
        )?;
        if changed > 0 {
            aggregate_flow_usage(&tx, &row.run_id, &row.node_id, row.attempt)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// VM observers call this for every root and nested run. Late completion
    /// can refine unknown usage after the Agent's timeout/cancel returned.
    pub fn record_invocation_flow_run(
        &self,
        row: &InvocationFlowRunRow,
    ) -> Result<(), StorageError> {
        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO agent_invocation_flow_runs(flow_run_id,run_id,node_id,attempt,parent_flow_run_id,state,usage_unknown,tokens_used,cost_usd_micro,duration_ms) VALUES(?,?,?,?,?,?,?,?,?,?) \
             ON CONFLICT(flow_run_id) DO UPDATE SET state=excluded.state,usage_unknown=excluded.usage_unknown,tokens_used=CASE WHEN excluded.tokens_used IS NULL THEN agent_invocation_flow_runs.tokens_used ELSE MAX(COALESCE(agent_invocation_flow_runs.tokens_used,0),excluded.tokens_used) END,cost_usd_micro=CASE WHEN excluded.cost_usd_micro IS NULL THEN agent_invocation_flow_runs.cost_usd_micro ELSE MAX(COALESCE(agent_invocation_flow_runs.cost_usd_micro,0),excluded.cost_usd_micro) END,duration_ms=MAX(agent_invocation_flow_runs.duration_ms,excluded.duration_ms) \
             WHERE agent_invocation_flow_runs.run_id=excluded.run_id AND agent_invocation_flow_runs.node_id=excluded.node_id AND agent_invocation_flow_runs.attempt=excluded.attempt AND (agent_invocation_flow_runs.state='running' OR excluded.state!='running')",
            params![row.flow_run_id,row.run_id,row.node_id,row.attempt,row.parent_flow_run_id,row.state,row.usage_unknown,sqlite_number(row.tokens_used)?,sqlite_number(row.cost_usd_micro)?,sqlite_number(Some(row.duration_ms))?],
        )?;
        aggregate_flow_usage(&tx, &row.run_id, &row.node_id, row.attempt)?;
        tx.commit()?;
        Ok(())
    }

    pub fn list_invocation_attempts(
        &self,
        run_id: &str,
    ) -> Result<Vec<InvocationAttemptRow>, StorageError> {
        let conn = self.inner.lock();
        let mut statement = conn.prepare("SELECT run_id,node_id,attempt,capability_id,source_kind,flow_run_id,state,usage_state,budget_sensitive,tokens_used,cost_usd_micro,duration_ms,started_at,finished_at,error,revision,resolved_by,resolution_reason,resolved_at FROM agent_invocation_attempts WHERE run_id=? ORDER BY started_at,node_id,attempt")?;
        let rows = statement
            .query_map([run_id], row_to_attempt)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_invocation_attempt(
        &self,
        run_id: &str,
        node_id: &str,
        attempt: u32,
    ) -> Result<Option<InvocationAttemptRow>, StorageError> {
        let conn = self.inner.lock();
        Ok(conn.query_row("SELECT run_id,node_id,attempt,capability_id,source_kind,flow_run_id,state,usage_state,budget_sensitive,tokens_used,cost_usd_micro,duration_ms,started_at,finished_at,error,revision,resolved_by,resolution_reason,resolved_at FROM agent_invocation_attempts WHERE run_id=? AND node_id=? AND attempt=?",params![run_id,node_id,attempt],row_to_attempt).optional()?)
    }

    pub fn list_invocation_flow_runs(
        &self,
        run_id: &str,
    ) -> Result<Vec<InvocationFlowRunRow>, StorageError> {
        let conn = self.inner.lock();
        let mut statement = conn.prepare("SELECT flow_run_id,run_id,node_id,attempt,parent_flow_run_id,state,usage_unknown,tokens_used,cost_usd_micro,duration_ms FROM agent_invocation_flow_runs WHERE run_id=? ORDER BY flow_run_id")?;
        let rows = statement
            .query_map([run_id], |row| {
                Ok(InvocationFlowRunRow {
                    flow_run_id: row.get(0)?,
                    run_id: row.get(1)?,
                    node_id: row.get(2)?,
                    attempt: row.get(3)?,
                    parent_flow_run_id: row.get(4)?,
                    state: row.get(5)?,
                    usage_unknown: row.get(6)?,
                    tokens_used: row.get(7)?,
                    cost_usd_micro: row.get(8)?,
                    duration_ms: row.get(9)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Confirm the final billed totals after checking provider records. The
    /// audit entry is immutable; concurrent UI/VM updates invalidate the CAS.
    pub fn resolve_invocation_usage(
        &self,
        resolution: &InvocationUsageResolution,
    ) -> Result<bool, StorageError> {
        if resolution.resolved_by.trim().is_empty() || resolution.reason.trim().is_empty() {
            return Err(StorageError::NotFound(
                "usage reconciliation requires a reviewer and reason".into(),
            ));
        }
        let tokens = sqlite_number(Some(resolution.tokens_used))?;
        let cost = sqlite_number(Some(resolution.cost_usd_micro))?;
        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        let now = Utc::now().timestamp_millis();
        let changed=tx.execute(
            "UPDATE agent_invocation_attempts SET tokens_used=?1,cost_usd_micro=?2,usage_state='recorded',resolved_by=?3,resolution_reason=?4,resolved_at=?5,revision=revision+1 \
             WHERE run_id=?6 AND node_id=?7 AND attempt=?8 AND revision=?9 AND state!='running' AND usage_state='unknown' \
             AND COALESCE(tokens_used,0)<=?1 AND COALESCE(cost_usd_micro,0)<=?2 \
             AND NOT EXISTS(SELECT 1 FROM agent_invocation_flow_runs WHERE run_id=?6 AND node_id=?7 AND attempt=?8 AND state='running')",
            params![tokens,cost,resolution.resolved_by.trim(),resolution.reason.trim(),now,resolution.run_id,resolution.node_id,resolution.attempt,resolution.expected_revision],
        )?;
        if changed == 0 {
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO agent_invocation_usage_resolutions(run_id,node_id,attempt,revision,tokens_used,cost_usd_micro,resolved_by,reason,resolved_at) VALUES(?,?,?,?,?,?,?,?,?)",
            params![resolution.run_id,resolution.node_id,resolution.attempt,resolution.expected_revision+1,tokens,cost,resolution.resolved_by.trim(),resolution.reason.trim(),now],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Recover observed lower bounds if the process stopped between persisting
    /// ai_calls and the VM's final observer. Missing completion evidence stays
    /// unknown; a crashed provider's unreported charge is never invented.
    pub fn reconcile_invocation_usage(&self, run_id: &str) -> Result<(), StorageError> {
        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE agent_invocation_flow_runs SET \
             tokens_used=MAX(COALESCE(tokens_used,0),COALESCE((SELECT SUM(input_tokens+output_tokens) FROM ai_calls WHERE flow_run_id=agent_invocation_flow_runs.flow_run_id),0)), \
             cost_usd_micro=MAX(COALESCE(cost_usd_micro,0),COALESCE((SELECT SUM(cost_usd_micro) FROM ai_calls WHERE flow_run_id=agent_invocation_flow_runs.flow_run_id),0)) \
             WHERE run_id=? AND (usage_unknown=1 OR state='running') AND EXISTS(SELECT 1 FROM ai_calls WHERE flow_run_id=agent_invocation_flow_runs.flow_run_id)", [run_id],
        )?;
        let keys = {
            let mut statement =
                tx.prepare("SELECT node_id,attempt FROM agent_invocation_attempts WHERE run_id=?")?;
            let result = statement
                .query_map([run_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            result
        };
        for (node_id, attempt) in keys {
            aggregate_flow_usage(&tx, run_id, &node_id, attempt)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Startup-only recovery for interactive Agent runs owned by the previous
    /// process. Durable jobs are recovered separately through expired leases.
    /// Call only after ensuring no live in-memory sessions use this database.
    pub fn recover_interrupted_invocations(&self) -> Result<usize, StorageError> {
        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        let run_ids = {
            let mut statement=tx.prepare("SELECT DISTINCT run_id FROM agent_invocation_attempts WHERE state='running' AND NOT EXISTS(SELECT 1 FROM agent_jobs WHERE agent_jobs.run_id=agent_invocation_attempts.run_id AND state='running')")?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        let mut count = 0;
        for run_id in run_ids {
            count += mark_interrupted_invocations(&tx, &run_id)?;
        }
        tx.commit()?;
        Ok(count)
    }
}

pub(crate) fn mark_interrupted_invocations(
    conn: &Connection,
    run_id: &str,
) -> Result<usize, rusqlite::Error> {
    conn.execute("UPDATE agent_invocation_flow_runs SET state='interrupted',usage_unknown=1 WHERE run_id=? AND state='running'",[run_id])?;
    conn.execute("UPDATE agent_invocation_attempts SET state='dropped',usage_state='unknown',finished_at=?,error='previous process stopped before reporting final usage',revision=revision+1 WHERE run_id=? AND state='running'",params![Utc::now().timestamp_millis(),run_id])
}

fn sqlite_number(number: Option<u64>) -> Result<Option<i64>, StorageError> {
    number
        .map(|n| {
            i64::try_from(n).map_err(|_| {
                StorageError::NotFound("invocation usage exceeds storage range".into())
            })
        })
        .transpose()
}

fn aggregate_flow_usage(
    conn: &Connection,
    run_id: &str,
    node_id: &str,
    attempt: u32,
) -> Result<(), rusqlite::Error> {
    let (count,tokens,cost,unknown):(i64,Option<i64>,Option<i64>,bool)=conn.query_row(
        "SELECT COUNT(*),SUM(tokens_used),SUM(cost_usd_micro),COALESCE(MAX(usage_unknown=1 OR state='running'),0) FROM agent_invocation_flow_runs WHERE run_id=? AND node_id=? AND attempt=?",
        params![run_id,node_id,attempt],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
    )?;
    if count == 0 {
        return Ok(());
    }
    let (old_tokens,old_cost,old_state,resolved):(Option<i64>,Option<i64>,String,bool)=conn.query_row(
        "SELECT tokens_used,cost_usd_micro,usage_state,resolved_at IS NOT NULL FROM agent_invocation_attempts WHERE run_id=? AND node_id=? AND attempt=?",
        params![run_id,node_id,attempt],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
    )?;
    let state = if resolved {
        if old_state == "unknown"
            || tokens.unwrap_or(0) > old_tokens.unwrap_or(0)
            || cost.unwrap_or(0) > old_cost.unwrap_or(0)
        {
            "unknown"
        } else {
            "recorded"
        }
    } else if unknown {
        "unknown"
    } else {
        "recorded"
    };
    let tokens = if resolved {
        Some(old_tokens.unwrap_or(0).max(tokens.unwrap_or(0)))
    } else {
        tokens
    };
    let cost = if resolved {
        Some(old_cost.unwrap_or(0).max(cost.unwrap_or(0)))
    } else {
        cost
    };
    if old_tokens != tokens || old_cost != cost || old_state != state {
        conn.execute("UPDATE agent_invocation_attempts SET tokens_used=?,cost_usd_micro=?,usage_state=?,revision=revision+1 WHERE run_id=? AND node_id=? AND attempt=?",params![tokens,cost,state,run_id,node_id,attempt])?;
    }
    Ok(())
}

fn row_to_attempt(row: &rusqlite::Row<'_>) -> rusqlite::Result<InvocationAttemptRow> {
    Ok(InvocationAttemptRow {
        run_id: row.get(0)?,
        node_id: row.get(1)?,
        attempt: row.get(2)?,
        capability_id: row.get(3)?,
        source_kind: row.get(4)?,
        flow_run_id: row.get(5)?,
        state: row.get(6)?,
        usage_state: row.get(7)?,
        budget_sensitive: row.get(8)?,
        tokens_used: row.get(9)?,
        cost_usd_micro: row.get(10)?,
        duration_ms: row.get(11)?,
        started_at: Utc
            .timestamp_millis_opt(row.get(12)?)
            .single()
            .unwrap_or_default(),
        finished_at: row
            .get::<_, Option<i64>>(13)?
            .and_then(|n| Utc.timestamp_millis_opt(n).single()),
        error: row.get(14)?,
        revision: row.get(15)?,
        resolved_by: row.get(16)?,
        resolution_reason: row.get(17)?,
        resolved_at: row
            .get::<_, Option<i64>>(18)?
            .and_then(|n| Utc.timestamp_millis_opt(n).single()),
    })
}
