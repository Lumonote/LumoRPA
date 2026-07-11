use std::str::FromStr;

use chrono::{DateTime, Utc};
use cron::Schedule;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{AgentPlan, RiskLevel};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Waiting,
    Paused,
    Completed,
    Failed,
    Unknown,
}

impl JobState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Unknown)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobSchedule {
    OneShot { run_at: DateTime<Utc> },
    Cron { expression: String },
}

impl JobSchedule {
    pub const fn one_shot(run_at: DateTime<Utc>) -> Self {
        Self::OneShot { run_at }
    }

    pub fn cron(expression: impl Into<String>) -> Result<Self, JobError> {
        let expression = expression.into();
        Schedule::from_str(&expression)
            .map_err(|error| JobError::InvalidCron(error.to_string()))?;
        Ok(Self::Cron { expression })
    }

    pub fn next_after(&self, after: DateTime<Utc>) -> Result<Option<DateTime<Utc>>, JobError> {
        match self {
            Self::OneShot { run_at } => Ok((*run_at > after).then_some(*run_at)),
            Self::Cron { expression } => {
                let schedule = Schedule::from_str(expression)
                    .map_err(|error| JobError::InvalidCron(error.to_string()))?;
                Ok(schedule.after(&after).next())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentJob {
    pub id: String,
    pub idempotency_key: String,
    pub plan: AgentPlan,
    pub schedule: JobSchedule,
    pub state: JobState,
    pub next_run_at: Option<DateTime<Utc>>,
    pub attempts: u32,
    pub max_attempts: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl AgentJob {
    pub fn new(
        id: impl Into<String>,
        idempotency_key: impl Into<String>,
        plan: AgentPlan,
        schedule: JobSchedule,
        now: DateTime<Utc>,
    ) -> Result<Self, JobError> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(JobError::EmptyId);
        }
        let idempotency_key = idempotency_key.into();
        if idempotency_key.trim().is_empty() {
            return Err(JobError::EmptyIdempotencyKey);
        }
        let next_run_at = match &schedule {
            JobSchedule::OneShot { run_at } => Some(*run_at),
            JobSchedule::Cron { .. } => schedule.next_after(now)?,
        };
        Ok(Self {
            id,
            idempotency_key,
            plan,
            schedule,
            state: JobState::Queued,
            next_run_at,
            attempts: 0,
            max_attempts: 3,
            created_at: now,
            updated_at: now,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryNode {
    pub node_id: String,
    pub risk: RiskLevel,
    pub idempotent: bool,
}

impl RecoveryNode {
    pub fn new(node_id: impl Into<String>, risk: RiskLevel, idempotent: bool) -> Self {
        Self {
            node_id: node_id.into(),
            risk,
            idempotent,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryDisposition {
    Resume,
    Fail,
    Unknown,
}

pub fn crash_recovery_disposition(nodes: &[RecoveryNode]) -> RecoveryDisposition {
    if nodes.iter().all(|node| node.idempotent) {
        return RecoveryDisposition::Resume;
    }
    if nodes
        .iter()
        .any(|node| !node.idempotent && node.risk >= RiskLevel::L2)
    {
        RecoveryDisposition::Unknown
    } else {
        RecoveryDisposition::Fail
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum JobError {
    #[error("job id must not be empty")]
    EmptyId,
    #[error("job idempotency key must not be empty")]
    EmptyIdempotencyKey,
    #[error("invalid cron expression: {0}")]
    InvalidCron(String),
}
