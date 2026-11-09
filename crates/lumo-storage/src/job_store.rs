use chrono::{DateTime, Duration, TimeZone, Utc};
use rusqlite::{params, OptionalExtension, Transaction};

use crate::{
    AgentJobRow, DurableJobNode, EnqueueJobResult, JobNodeCheckpoint, NewAgentJob, RecoveredJob,
    Repo, StorageError,
};

impl Repo {
    /// Edit only an idle occurrence. A retry, durable approval or uncertain
    /// operation must keep its original plan and checkpoint binding.
    pub fn update_job_schedule(
        &self,
        job_id: &str,
        expected_updated_at: DateTime<Utc>,
        replacement: &NewAgentJob,
        now: DateTime<Utc>,
    ) -> Result<bool, StorageError> {
        let payload = serde_json::to_string(&replacement.payload)?;
        let spec = serde_json::to_string(&replacement.schedule_spec)?;
        let conn = self.inner.lock();
        Ok(conn.execute(
            "UPDATE agent_jobs SET payload_json=?,schedule_kind=?,schedule_spec_json=?,priority=?,available_at=?,max_attempts=?,updated_at=? \
             WHERE id=? AND updated_at=? AND state IN ('queued','paused') AND attempts=0 AND run_id IS NULL AND worker_id IS NULL AND cancelled_at IS NULL \
             AND NOT EXISTS (SELECT 1 FROM agent_job_nodes WHERE job_id=agent_jobs.id)",
            params![payload,replacement.schedule_kind,spec,replacement.priority,replacement.available_at.timestamp_millis(),replacement.max_attempts,now.timestamp_millis().max(expected_updated_at.timestamp_millis().saturating_add(1)),job_id,expected_updated_at.timestamp_millis()],
        )? == 1)
    }

    pub fn enqueue_job(&self, job: &NewAgentJob) -> Result<EnqueueJobResult, StorageError> {
        let payload = serde_json::to_string(&job.payload)?;
        let schedule_spec = serde_json::to_string(&job.schedule_spec)?;
        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO agent_jobs(\
             id,idempotency_key,payload_json,schedule_kind,schedule_spec_json,state,priority,\
             available_at,attempts,max_attempts,created_at,updated_at) \
             VALUES (?,?,?,?,?,'queued',?,?,0,?,?,?)",
            params![
                job.id,
                job.idempotency_key,
                payload,
                job.schedule_kind,
                schedule_spec,
                job.priority,
                job.available_at.timestamp_millis(),
                job.max_attempts,
                job.created_at.timestamp_millis(),
                job.created_at.timestamp_millis(),
            ],
        )? > 0;
        let stored = tx.query_row(
            &format!("{} WHERE idempotency_key=?", JOB_SELECT),
            [&job.idempotency_key],
            row_to_job,
        )?;
        tx.commit()?;
        Ok(EnqueueJobResult {
            job: stored,
            inserted,
        })
    }

    pub fn get_job(&self, id: &str) -> Result<Option<AgentJobRow>, StorageError> {
        let conn = self.inner.lock();
        conn.query_row(&format!("{} WHERE id=?", JOB_SELECT), [id], row_to_job)
            .optional()
            .map_err(Into::into)
    }

    pub fn list_jobs(&self, limit: u32) -> Result<Vec<AgentJobRow>, StorageError> {
        let conn = self.inner.lock();
        let mut stmt = conn.prepare(&format!(
            "{} ORDER BY updated_at DESC, id ASC LIMIT ?",
            JOB_SELECT
        ))?;
        let rows = stmt.query_map([limit], row_to_job)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn acquire_job_lease(
        &self,
        worker_id: &str,
        now: DateTime<Utc>,
        lease_duration: Duration,
    ) -> Result<Option<AgentJobRow>, StorageError> {
        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        let now_ms = now.timestamp_millis();
        let candidate = tx
            .query_row(
                "SELECT id FROM agent_jobs \
                 WHERE state IN ('queued','waiting') AND available_at<=? \
                 ORDER BY priority DESC, available_at ASC, created_at ASC, id ASC LIMIT 1",
                [now_ms],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(id) = candidate else {
            tx.commit()?;
            return Ok(None);
        };
        let changed = tx.execute(
            "UPDATE agent_jobs SET state='running',worker_id=?,lease_until=?,heartbeat_at=?,\
             attempts=attempts+1,updated_at=? \
             WHERE id=? AND state IN ('queued','waiting') AND available_at<=?",
            params![
                worker_id,
                (now + lease_duration).timestamp_millis(),
                now_ms,
                now_ms,
                id,
                now_ms,
            ],
        )?;
        let job = if changed == 1 {
            Some(tx.query_row(&format!("{} WHERE id=?", JOB_SELECT), [&id], row_to_job)?)
        } else {
            None
        };
        tx.commit()?;
        Ok(job)
    }

    pub fn heartbeat_job(
        &self,
        job_id: &str,
        worker_id: &str,
        now: DateTime<Utc>,
        lease_duration: Duration,
    ) -> Result<bool, StorageError> {
        let conn = self.inner.lock();
        let now_ms = now.timestamp_millis();
        Ok(conn.execute(
            "UPDATE agent_jobs SET heartbeat_at=?,lease_until=?,updated_at=? \
             WHERE id=? AND state='running' AND worker_id=? AND lease_until>=?",
            params![
                now_ms,
                (now + lease_duration).timestamp_millis(),
                now_ms,
                job_id,
                worker_id,
                now_ms,
            ],
        )? == 1)
    }

    pub fn retry_job(
        &self,
        job_id: &str,
        worker_id: &str,
        now: DateTime<Utc>,
        retry_at: DateTime<Utc>,
        error: &str,
    ) -> Result<bool, StorageError> {
        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        let owned = owns_lease(&tx, job_id, worker_id, now)?;
        if !owned {
            return Ok(false);
        }
        let uncertain = has_uncertain_nodes(&tx, job_id)?;
        if uncertain {
            classify_running_nodes(&tx, job_id, now)?;
        }
        let changed = tx.execute(
            "UPDATE agent_jobs SET \
               state=CASE WHEN ? THEN 'unknown' WHEN attempts>=max_attempts THEN 'failed' ELSE 'waiting' END,\
               available_at=?,worker_id=NULL,lease_until=NULL,heartbeat_at=NULL,last_error=?,updated_at=? \
             WHERE id=? AND state='running' AND worker_id=?",
            params![
                uncertain,
                retry_at.timestamp_millis(),
                error,
                now.timestamp_millis(),
                job_id,
                worker_id,
            ],
        )? == 1;
        tx.commit()?;
        Ok(changed)
    }

    pub fn pause_job(&self, job_id: &str, now: DateTime<Utc>) -> Result<bool, StorageError> {
        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        let uncertain = has_uncertain_nodes(&tx, job_id)?;
        let target = if uncertain { "unknown" } else { "paused" };
        let changed = tx.execute(
            "UPDATE agent_jobs SET state=?,worker_id=NULL,lease_until=NULL,heartbeat_at=NULL,updated_at=? \
             WHERE id=? AND state IN ('queued','waiting','running')",
            params![target, now.timestamp_millis(), job_id],
        )? == 1;
        if changed {
            classify_running_nodes(&tx, job_id, now)?;
        }
        tx.commit()?;
        Ok(changed)
    }

    pub fn resume_job(&self, job_id: &str, now: DateTime<Utc>) -> Result<bool, StorageError> {
        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        if has_uncertain_nodes(&tx, job_id)? {
            let changed = tx.execute(
                "UPDATE agent_jobs SET state='unknown',updated_at=? WHERE id=? AND state='paused'",
                params![now.timestamp_millis(), job_id],
            )?;
            if changed == 1 {
                classify_running_nodes(&tx, job_id, now)?;
            }
            tx.commit()?;
            return Ok(false);
        }
        let changed = tx.execute(
            "UPDATE agent_jobs SET state='queued',available_at=MAX(available_at,?),updated_at=? \
             WHERE id=? AND state='paused' AND cancelled_at IS NULL",
            params![now.timestamp_millis(), now.timestamp_millis(), job_id],
        )? == 1;
        tx.commit()?;
        Ok(changed)
    }

    pub fn cancel_job(&self, job_id: &str, now: DateTime<Utc>) -> Result<bool, StorageError> {
        let conn = self.inner.lock();
        let now_ms = now.timestamp_millis();
        Ok(conn.execute(
            "UPDATE agent_jobs SET state='failed',worker_id=NULL,lease_until=NULL,heartbeat_at=NULL,\
             last_error='cancelled',cancelled_at=?,updated_at=? \
             WHERE id=? AND state IN ('queued','waiting','waiting_approval','paused','running','unknown')",
            params![now_ms, now_ms, job_id],
        )? == 1)
    }

    /// Mission Control addresses one occurrence by run ID. Bind the state
    /// change and its event in one transaction so stale controls cannot affect
    /// a later cron occurrence or leave history inconsistent after disk errors.
    pub fn control_job_run(
        &self,
        run_id: &str,
        action: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<(AgentJobRow, crate::AgentEventRow)>, StorageError> {
        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        let job: Option<AgentJobRow> = tx
            .query_row(
                &format!("{JOB_SELECT} WHERE run_id=?"),
                [run_id],
                row_to_job,
            )
            .optional()?;
        let Some(job) = job else {
            return Ok(None);
        };
        let uncertain = has_uncertain_nodes(&tx, &job.id)?;
        let (target, event_kind, run_state, terminal) = match action {
            "pause" if matches!(job.state.as_str(), "queued" | "waiting" | "running") => (
                if uncertain { "unknown" } else { "paused" },
                "run.paused",
                "paused",
                false,
            ),
            "resume" if job.state == "paused" && !uncertain && job.cancelled_at.is_none() => {
                ("queued", "run.resumed", "running", false)
            }
            "cancel"
                if matches!(
                    job.state.as_str(),
                    "queued" | "waiting" | "waiting_approval" | "paused" | "running" | "unknown"
                ) =>
            {
                ("failed", "run.cancelled", "cancelled", true)
            }
            _ => return Ok(None),
        };
        let changed = tx.execute(
            "UPDATE agent_jobs SET state=?,worker_id=NULL,lease_until=NULL,heartbeat_at=NULL,updated_at=?,\
             available_at=CASE WHEN ?='resume' THEN MAX(available_at,?) ELSE available_at END,\
             last_error=CASE WHEN ?='cancel' THEN 'cancelled' ELSE last_error END,\
             cancelled_at=CASE WHEN ?='cancel' THEN ? ELSE cancelled_at END \
             WHERE id=? AND run_id=? AND state=?",
            params![target,now.timestamp_millis(),action,now.timestamp_millis(),action,action,now.timestamp_millis(),job.id,run_id,job.state],
        )?;
        if changed != 1 {
            return Ok(None);
        }
        if action == "pause" {
            classify_running_nodes(&tx, &job.id, now)?;
        }
        let updated = tx.execute(
            "UPDATE agent_runs SET state=?,finished_at=CASE WHEN ? THEN ? ELSE finished_at END,\
             error=CASE WHEN ? THEN 'cancelled' ELSE error END WHERE id=? AND finished_at IS NULL",
            params![
                run_state,
                terminal,
                now.timestamp_millis(),
                terminal,
                run_id
            ],
        )?;
        if updated != 1 {
            return Ok(None);
        }
        let last: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq),0) FROM agent_events WHERE run_id=?",
            [run_id],
            |row| row.get(0),
        )?;
        let seq = last
            .checked_add(1)
            .ok_or_else(|| StorageError::NotFound("event sequence exhausted".into()))?;
        let payload = serde_json::json!({"jobId":job.id,"jobState":target,"state":run_state,"scope":"background_job"});
        tx.execute(
            "INSERT INTO agent_events(run_id,seq,kind,payload,created_at) VALUES (?,?,?,?,?)",
            params![
                run_id,
                seq,
                event_kind,
                payload.to_string(),
                now.timestamp_millis()
            ],
        )?;
        let updated_job =
            tx.query_row(&format!("{JOB_SELECT} WHERE id=?"), [&job.id], row_to_job)?;
        tx.commit()?;
        Ok(Some((
            updated_job,
            crate::AgentEventRow {
                run_id: run_id.into(),
                seq,
                kind: event_kind.into(),
                node_id: None,
                parent_node_id: None,
                payload,
                created_at: now,
            },
        )))
    }

    pub fn complete_job(
        &self,
        job_id: &str,
        worker_id: &str,
        now: DateTime<Utc>,
        next_run_at: Option<DateTime<Utc>>,
    ) -> Result<bool, StorageError> {
        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        let now_ms = now.timestamp_millis();
        let (state, available_at, attempts) = match next_run_at {
            Some(next) => ("queued", next.timestamp_millis(), 0),
            None => ("completed", now_ms, 0),
        };
        let changed = tx.execute(
            "UPDATE agent_jobs SET state=?,available_at=?,attempts=?,worker_id=NULL,lease_until=NULL,\
             heartbeat_at=NULL,last_error=NULL,updated_at=?,run_id=CASE WHEN ? THEN NULL ELSE run_id END \
             WHERE id=? AND state='running' AND worker_id=? AND lease_until>=?",
            params![state, available_at, attempts, now_ms, next_run_at.is_some(), job_id, worker_id, now_ms],
        )? == 1;
        if changed && next_run_at.is_some() {
            tx.execute("DELETE FROM agent_job_nodes WHERE job_id=?", [job_id])?;
        }
        tx.commit()?;
        Ok(changed)
    }

    pub fn record_job_node(&self, node: &JobNodeCheckpoint) -> Result<(), StorageError> {
        let conn = self.inner.lock();
        conn.execute(
            "INSERT INTO agent_job_nodes(job_id,node_id,state,risk,idempotent,attempt,updated_at) \
             VALUES (?,?,?,?,?,?,?) \
             ON CONFLICT(job_id,node_id) DO UPDATE SET state=excluded.state,risk=excluded.risk,\
               idempotent=excluded.idempotent,attempt=excluded.attempt,updated_at=excluded.updated_at",
            params![
                node.job_id,
                node.node_id,
                node.state,
                node.risk,
                node.idempotent,
                node.attempt,
                node.updated_at.timestamp_millis(),
            ],
        )?;
        Ok(())
    }

    pub fn list_job_nodes(&self, job_id: &str) -> Result<Vec<JobNodeCheckpoint>, StorageError> {
        let conn = self.inner.lock();
        let mut stmt = conn.prepare(
            "SELECT job_id,node_id,state,risk,idempotent,attempt,updated_at \
             FROM agent_job_nodes WHERE job_id=? ORDER BY node_id ASC",
        )?;
        let rows = stmt.query_map([job_id], row_to_node)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn bind_job_run(
        &self,
        job_id: &str,
        worker_id: &str,
        run_id: &str,
        now: DateTime<Utc>,
    ) -> Result<bool, StorageError> {
        let conn = self.inner.lock();
        Ok(conn.execute(
            "UPDATE agent_jobs SET run_id=?,updated_at=? WHERE id=? AND state='running' AND worker_id=? AND lease_until>=? AND (run_id IS NULL OR run_id=?)",
            params![run_id, now.timestamp_millis(), job_id, worker_id, now.timestamp_millis(), run_id],
        )? == 1)
    }

    pub fn job_for_run(&self, run_id: &str) -> Result<Option<AgentJobRow>, StorageError> {
        let conn = self.inner.lock();
        conn.query_row(
            &format!("{JOB_SELECT} WHERE run_id=?"),
            [run_id],
            row_to_job,
        )
        .optional()
        .map_err(Into::into)
    }

    /// Fail closed before dispatch and before acknowledging a result. A worker
    /// that has lost its lease cannot mutate a checkpoint owned by a new run.
    pub fn save_durable_job_node(
        &self,
        worker_id: &str,
        node: &DurableJobNode,
    ) -> Result<(), StorageError> {
        let checkpoint = &node.checkpoint;
        if node.node_hash.is_empty() || (checkpoint.state == "completed" && node.result.is_none()) {
            return Err(StorageError::NotFound(
                "durable checkpoint requires a hash and completed result".into(),
            ));
        }
        let encoded = node
            .result
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        if !owns_lease(&tx, &checkpoint.job_id, worker_id, checkpoint.updated_at)? {
            return Err(StorageError::NotFound(
                "job lease is no longer owned by this worker".into(),
            ));
        }
        let changed = tx.execute(
            "INSERT INTO agent_job_nodes(job_id,node_id,state,risk,idempotent,attempt,updated_at,node_hash,result_json) \
             VALUES (?,?,?,?,?,?,?,?,?) ON CONFLICT(job_id,node_id) DO UPDATE SET \
             state=excluded.state,risk=excluded.risk,idempotent=excluded.idempotent,attempt=excluded.attempt,\
             updated_at=excluded.updated_at,node_hash=excluded.node_hash,result_json=excluded.result_json \
             WHERE agent_job_nodes.state NOT IN ('unknown','completed')",
            params![checkpoint.job_id, checkpoint.node_id, checkpoint.state, checkpoint.risk, checkpoint.idempotent,
                checkpoint.attempt, checkpoint.updated_at.timestamp_millis(), node.node_hash, encoded],
        )?;
        if changed != 1 {
            return Err(StorageError::NotFound(
                "checkpoint is completed or requires human review".into(),
            ));
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_durable_job_nodes(
        &self,
        job_id: &str,
    ) -> Result<Vec<DurableJobNode>, StorageError> {
        let conn = self.inner.lock();
        let mut statement = conn.prepare("SELECT job_id,node_id,state,risk,idempotent,attempt,updated_at,node_hash,result_json FROM agent_job_nodes WHERE job_id=? ORDER BY node_id ASC")?;
        let nodes = statement.query_map([job_id], |row| {
            let encoded: Option<String> = row.get(8)?;
            let result = encoded
                .map(|value| {
                    serde_json::from_str(&value).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            8,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })
                })
                .transpose()?;
            Ok(DurableJobNode {
                checkpoint: row_to_node(row)?,
                node_hash: row.get(7)?,
                result,
            })
        })?;
        nodes.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn wait_job_approval(
        &self,
        job_id: &str,
        worker_id: &str,
        now: DateTime<Utc>,
    ) -> Result<bool, StorageError> {
        let conn = self.inner.lock();
        Ok(conn.execute(
            "UPDATE agent_jobs SET state='waiting_approval',attempts=MAX(0,attempts-1),worker_id=NULL,lease_until=NULL,heartbeat_at=NULL,last_error=NULL,updated_at=? \
             WHERE id=? AND state='running' AND worker_id=? AND lease_until>=? AND run_id IS NOT NULL \
             AND NOT EXISTS(SELECT 1 FROM agent_job_nodes WHERE job_id=? AND state IN ('running','unknown'))",
            params![now.timestamp_millis(),job_id,worker_id,now.timestamp_millis(),job_id],
        )? == 1)
    }

    pub fn resume_job_approval(
        &self,
        job_id: &str,
        now: DateTime<Utc>,
    ) -> Result<bool, StorageError> {
        let conn = self.inner.lock();
        Ok(conn.execute(
            "UPDATE agent_jobs SET state='queued',available_at=?,updated_at=? WHERE id=? AND state='waiting_approval' AND cancelled_at IS NULL",
            params![now.timestamp_millis(), now.timestamp_millis(),job_id],
        )? == 1)
    }

    /// An explicit human decision is the sole transition out of uncertainty.
    /// None means verified safe to retry; Some contains the verified full result.
    pub fn resolve_unknown_job_node(
        &self,
        job_id: &str,
        node_id: &str,
        completed_result: Option<serde_json::Value>,
        now: DateTime<Utc>,
    ) -> Result<bool, StorageError> {
        let encoded = completed_result
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let target = if completed_result.is_some() {
            "completed"
        } else {
            "queued"
        };
        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        let node_hash: Option<String> = tx.query_row(
            "SELECT node_hash FROM agent_job_nodes WHERE job_id=? AND node_id=? AND state='unknown'",
            params![job_id,node_id], |row| row.get(0),
        ).optional()?;
        if completed_result.is_some() && node_hash.as_deref() == Some("") {
            // Legacy checkpoints do not bind a result to a plan. They can only
            // be cleared after a person verifies that retry is safe.
            return Ok(false);
        }
        let changed = tx.execute(
            "UPDATE agent_job_nodes SET state=?,result_json=?,updated_at=? WHERE job_id=? AND node_id=? AND state='unknown' \
             AND EXISTS(SELECT 1 FROM agent_jobs WHERE id=? AND state='unknown' AND cancelled_at IS NULL)",
            params![target,encoded,now.timestamp_millis(),job_id,node_id,job_id],
        )? == 1;
        if changed {
            if completed_result.is_none() && node_hash.as_deref() == Some("") {
                tx.execute(
                    "DELETE FROM agent_job_nodes WHERE job_id=? AND node_id=?",
                    params![job_id, node_id],
                )?;
            }
            tx.execute(
                "UPDATE agent_jobs SET state=CASE WHEN EXISTS(SELECT 1 FROM agent_job_nodes WHERE job_id=? AND state='unknown') THEN 'unknown' ELSE 'queued' END,\
                 available_at=?,updated_at=?,last_error=NULL WHERE id=?",
                params![job_id,now.timestamp_millis(),now.timestamp_millis(),job_id],
            )?;
        }
        tx.commit()?;
        Ok(changed)
    }

    pub fn recover_expired_jobs(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<RecoveredJob>, StorageError> {
        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        let job_ids = expired_job_ids(&tx, now)?;
        let mut recovered = Vec::with_capacity(job_ids.len());
        for job_id in job_ids {
            tx.execute(
                "UPDATE agent_job_nodes SET state='unknown',updated_at=? WHERE job_id=? AND state='completed' AND (node_hash='' OR result_json IS NULL)",
                params![now.timestamp_millis(),job_id],
            )?;
            let nodes = list_running_nodes(&tx, &job_id)?;
            let mut state = if has_uncertain_nodes(&tx, &job_id)? {
                "unknown"
            } else {
                recovery_state(&nodes)
            };
            let has_binding: bool = tx.query_row(
                "SELECT run_id IS NOT NULL FROM agent_jobs WHERE id=?",
                [&job_id],
                |row| row.get(0),
            )?;
            let has_nodes: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM agent_job_nodes WHERE job_id=?)",
                [&job_id],
                |row| row.get(0),
            )?;
            if !has_nodes && !has_binding {
                state = "unknown";
                // A legacy execution without checkpoints cannot prove whether
                // it dispatched a side effect. Keep a visible review marker.
                tx.execute("INSERT INTO agent_job_nodes(job_id,node_id,state,risk,idempotent,attempt,updated_at) VALUES (?,'__legacy_execution__','unknown','L3',0,1,?)", params![job_id,now.timestamp_millis()])?;
            }
            classify_running_nodes(&tx, &job_id, now)?;
            tx.execute(
                "UPDATE agent_jobs SET state=?,available_at=CASE WHEN ?='queued' THEN ? ELSE available_at END,\
                 worker_id=NULL,lease_until=NULL,heartbeat_at=NULL,last_error=?,updated_at=? WHERE id=?",
                params![
                    state,
                    state,
                    now.timestamp_millis(),
                    recovery_error(state),
                    now.timestamp_millis(),
                    job_id,
                ],
            )?;
            recovered.push(RecoveredJob {
                job_id,
                state: state.into(),
            });
        }
        tx.commit()?;
        Ok(recovered)
    }
}

const JOB_SELECT: &str = "SELECT id,idempotency_key,payload_json,schedule_kind,schedule_spec_json,\
 state,priority,available_at,attempts,max_attempts,worker_id,lease_until,heartbeat_at,last_error,\
 cancelled_at,created_at,updated_at,run_id FROM agent_jobs";

fn row_to_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentJobRow> {
    Ok(AgentJobRow {
        id: row.get(0)?,
        idempotency_key: row.get(1)?,
        payload: json_at(row, 2)?,
        schedule_kind: row.get(3)?,
        schedule_spec: json_at(row, 4)?,
        state: row.get(5)?,
        priority: row.get(6)?,
        available_at: timestamp_at(row, 7)?,
        attempts: row.get(8)?,
        max_attempts: row.get(9)?,
        worker_id: row.get(10)?,
        lease_until: optional_timestamp_at(row, 11)?,
        heartbeat_at: optional_timestamp_at(row, 12)?,
        last_error: row.get(13)?,
        cancelled_at: optional_timestamp_at(row, 14)?,
        created_at: timestamp_at(row, 15)?,
        updated_at: timestamp_at(row, 16)?,
        run_id: row.get(17)?,
    })
}

fn row_to_node(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobNodeCheckpoint> {
    Ok(JobNodeCheckpoint {
        job_id: row.get(0)?,
        node_id: row.get(1)?,
        state: row.get(2)?,
        risk: row.get(3)?,
        idempotent: row.get(4)?,
        attempt: row.get(5)?,
        updated_at: timestamp_at(row, 6)?,
    })
}

fn list_running_nodes(
    tx: &Transaction<'_>,
    job_id: &str,
) -> Result<Vec<JobNodeCheckpoint>, StorageError> {
    let mut stmt = tx.prepare(
        "SELECT job_id,node_id,state,risk,idempotent,attempt,updated_at \
         FROM agent_job_nodes WHERE job_id=? AND state='running' ORDER BY node_id ASC",
    )?;
    let rows = stmt.query_map([job_id], row_to_node)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn expired_job_ids(tx: &Transaction<'_>, now: DateTime<Utc>) -> Result<Vec<String>, StorageError> {
    let mut stmt = tx.prepare(
        "SELECT id FROM agent_jobs WHERE state='running' AND lease_until<? ORDER BY id ASC",
    )?;
    let rows = stmt.query_map([now.timestamp_millis()], |row| row.get::<_, String>(0))?;
    let result = rows.collect::<Result<Vec<_>, _>>();
    drop(stmt);
    result.map_err(Into::into)
}

fn recovery_state(nodes: &[JobNodeCheckpoint]) -> &'static str {
    if nodes.iter().all(|node| node.idempotent) {
        "queued"
    } else {
        "unknown"
    }
}

fn recovery_error(state: &str) -> Option<&'static str> {
    match state {
        "unknown" => Some("execution outcome requires human review before retry"),
        "failed" => Some("worker crashed during non-idempotent work"),
        _ => None,
    }
}

fn owns_lease(
    tx: &Transaction<'_>,
    job_id: &str,
    worker_id: &str,
    now: DateTime<Utc>,
) -> Result<bool, StorageError> {
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_jobs WHERE id=? AND state='running' AND worker_id=? AND lease_until>=?)",
        params![job_id,worker_id,now.timestamp_millis()], |row| row.get(0),
    )?)
}

fn has_uncertain_nodes(tx: &Transaction<'_>, job_id: &str) -> Result<bool, StorageError> {
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_job_nodes WHERE job_id=? AND (state='unknown' OR (state='running' AND idempotent=0)))",
        [job_id], |row| row.get(0),
    )?)
}

fn classify_running_nodes(
    tx: &Transaction<'_>,
    job_id: &str,
    now: DateTime<Utc>,
) -> Result<(), StorageError> {
    tx.execute(
        "UPDATE agent_job_nodes SET state=CASE WHEN idempotent=1 THEN 'queued' ELSE 'unknown' END,updated_at=? \
         WHERE job_id=? AND state='running'",
        params![now.timestamp_millis(),job_id],
    )?;
    Ok(())
}

fn json_at(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<serde_json::Value> {
    let value: String = row.get(index)?;
    serde_json::from_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn timestamp_at(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<DateTime<Utc>> {
    let millis = row.get(index)?;
    Utc.timestamp_millis_opt(millis).single().ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            "timestamp out of range".into(),
        )
    })
}

fn optional_timestamp_at(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<Option<DateTime<Utc>>> {
    let millis: Option<i64> = row.get(index)?;
    millis
        .map(|millis| {
            Utc.timestamp_millis_opt(millis).single().ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    index,
                    rusqlite::types::Type::Integer,
                    "timestamp out of range".into(),
                )
            })
        })
        .transpose()
}
