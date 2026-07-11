use chrono::{DateTime, Duration, TimeZone, Utc};
use rusqlite::{params, OptionalExtension, Transaction};

use crate::{
    AgentJobRow, EnqueueJobResult, JobNodeCheckpoint, NewAgentJob, RecoveredJob, Repo, StorageError,
};

impl Repo {
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
        let conn = self.inner.lock();
        Ok(conn.execute(
            "UPDATE agent_jobs SET \
               state=CASE WHEN attempts>=max_attempts THEN 'failed' ELSE 'waiting' END,\
               available_at=?,worker_id=NULL,lease_until=NULL,heartbeat_at=NULL,last_error=?,updated_at=? \
             WHERE id=? AND state='running' AND worker_id=?",
            params![
                retry_at.timestamp_millis(),
                error,
                now.timestamp_millis(),
                job_id,
                worker_id,
            ],
        )? == 1)
    }

    pub fn pause_job(&self, job_id: &str, now: DateTime<Utc>) -> Result<bool, StorageError> {
        self.transition_without_owner(
            job_id,
            now,
            "paused",
            "state IN ('queued','waiting','running')",
        )
    }

    pub fn resume_job(&self, job_id: &str, now: DateTime<Utc>) -> Result<bool, StorageError> {
        let conn = self.inner.lock();
        Ok(conn.execute(
            "UPDATE agent_jobs SET state='queued',available_at=MAX(available_at,?),updated_at=? \
             WHERE id=? AND state='paused' AND cancelled_at IS NULL",
            params![now.timestamp_millis(), now.timestamp_millis(), job_id],
        )? == 1)
    }

    pub fn cancel_job(&self, job_id: &str, now: DateTime<Utc>) -> Result<bool, StorageError> {
        let conn = self.inner.lock();
        let now_ms = now.timestamp_millis();
        Ok(conn.execute(
            "UPDATE agent_jobs SET state='failed',worker_id=NULL,lease_until=NULL,heartbeat_at=NULL,\
             last_error='cancelled',cancelled_at=?,updated_at=? \
             WHERE id=? AND state IN ('queued','waiting','paused','running')",
            params![now_ms, now_ms, job_id],
        )? == 1)
    }

    pub fn complete_job(
        &self,
        job_id: &str,
        worker_id: &str,
        now: DateTime<Utc>,
        next_run_at: Option<DateTime<Utc>>,
    ) -> Result<bool, StorageError> {
        let conn = self.inner.lock();
        let now_ms = now.timestamp_millis();
        let (state, available_at, attempts) = match next_run_at {
            Some(next) => ("queued", next.timestamp_millis(), 0),
            None => ("completed", now_ms, 0),
        };
        Ok(conn.execute(
            "UPDATE agent_jobs SET state=?,available_at=?,attempts=?,worker_id=NULL,lease_until=NULL,\
             heartbeat_at=NULL,last_error=NULL,updated_at=? \
             WHERE id=? AND state='running' AND worker_id=?",
            params![state, available_at, attempts, now_ms, job_id, worker_id],
        )? == 1)
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

    pub fn recover_expired_jobs(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<RecoveredJob>, StorageError> {
        let mut conn = self.inner.lock();
        let tx = conn.transaction()?;
        let job_ids = expired_job_ids(&tx, now)?;
        let mut recovered = Vec::with_capacity(job_ids.len());
        for job_id in job_ids {
            let nodes = list_running_nodes(&tx, &job_id)?;
            let state = recovery_state(&nodes);
            let node_state = match state {
                "queued" => "queued",
                "unknown" => "unknown",
                _ => "failed",
            };
            tx.execute(
                "UPDATE agent_job_nodes SET state=?,updated_at=? WHERE job_id=? AND state='running'",
                params![node_state, now.timestamp_millis(), job_id],
            )?;
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

    fn transition_without_owner(
        &self,
        job_id: &str,
        now: DateTime<Utc>,
        target: &str,
        condition: &str,
    ) -> Result<bool, StorageError> {
        let conn = self.inner.lock();
        let sql = format!(
            "UPDATE agent_jobs SET state=?,worker_id=NULL,lease_until=NULL,heartbeat_at=NULL,updated_at=? \
             WHERE id=? AND {condition}"
        );
        Ok(conn.execute(&sql, params![target, now.timestamp_millis(), job_id])? == 1)
    }
}

const JOB_SELECT: &str = "SELECT id,idempotency_key,payload_json,schedule_kind,schedule_spec_json,\
 state,priority,available_at,attempts,max_attempts,worker_id,lease_until,heartbeat_at,last_error,\
 cancelled_at,created_at,updated_at FROM agent_jobs";

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
    } else if nodes
        .iter()
        .any(|node| !node.idempotent && matches!(node.risk.as_str(), "L2" | "L3"))
    {
        "unknown"
    } else {
        "failed"
    }
}

fn recovery_error(state: &str) -> Option<&'static str> {
    match state {
        "unknown" => Some("worker crashed during a non-idempotent L2/L3 node"),
        "failed" => Some("worker crashed during non-idempotent work"),
        _ => None,
    }
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
