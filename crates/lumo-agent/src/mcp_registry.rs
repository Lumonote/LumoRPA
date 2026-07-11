//! MCP discovery approval gate and deterministic health supervision primitives.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpSignatureStatus {
    Verified,
    Unverified,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpSignatureMetadata {
    pub algorithm: String,
    pub key_id: String,
    pub digest: String,
    pub status: McpSignatureStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpPublisherMetadata {
    pub id: String,
    pub display_name: String,
    pub website: Option<String>,
    pub signature: Option<McpSignatureMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpRelease {
    pub server_id: String,
    pub version: String,
    pub publisher: McpPublisherMetadata,
    pub tools: Vec<McpToolDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredMcpTool {
    pub server_id: String,
    pub release_version: String,
    pub publisher: McpPublisherMetadata,
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub schema_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryDriftKind {
    NewTool,
    SchemaChanged,
    VersionChanged,
    SchemaAndVersionChanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryDrift {
    pub server_id: String,
    pub tool_name: String,
    pub kind: RegistryDriftKind,
    pub previous_version: Option<String>,
    pub discovered_version: String,
    pub previous_schema_hash: Option<String>,
    pub discovered_schema_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryUpdate {
    pub server_id: String,
    pub version: String,
    pub drifts: Vec<RegistryDrift>,
}

#[derive(Debug, Default)]
pub struct McpRegistry {
    publishers: BTreeMap<String, McpPublisherMetadata>,
    visible: BTreeMap<(String, String), RegisteredMcpTool>,
    pending: BTreeMap<(String, String), RegisteredMcpTool>,
}

impl McpRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn discover(&mut self, release: McpRelease) -> Result<RegistryUpdate, RegistryError> {
        validate_release(&release)?;
        let mut names = BTreeSet::new();
        for tool in &release.tools {
            if !names.insert(tool.name.as_str()) {
                return Err(RegistryError::InvalidRelease {
                    message: format!("duplicate tool `{}`", tool.name),
                });
            }
        }
        self.publishers
            .insert(release.server_id.clone(), release.publisher.clone());

        let mut drifts = Vec::new();
        for tool in release.tools.clone() {
            let discovered = registered_tool(&release, tool);
            let key = (release.server_id.clone(), discovered.name.clone());
            let baseline = self.visible.get(&key);
            let pending = self.pending.get(&key);
            if pending == Some(&discovered) || baseline == Some(&discovered) {
                continue;
            }
            let kind = match baseline {
                None => RegistryDriftKind::NewTool,
                Some(previous) => match (
                    previous.schema_hash != discovered.schema_hash,
                    previous.release_version != discovered.release_version,
                ) {
                    (true, true) => RegistryDriftKind::SchemaAndVersionChanged,
                    (true, false) => RegistryDriftKind::SchemaChanged,
                    (false, true) => RegistryDriftKind::VersionChanged,
                    (false, false) => continue,
                },
            };
            drifts.push(RegistryDrift {
                server_id: release.server_id.clone(),
                tool_name: discovered.name.clone(),
                kind,
                previous_version: baseline.map(|tool| tool.release_version.clone()),
                discovered_version: discovered.release_version.clone(),
                previous_schema_hash: baseline.map(|tool| tool.schema_hash.clone()),
                discovered_schema_hash: discovered.schema_hash.clone(),
            });
            self.pending.insert(key, discovered);
        }
        Ok(RegistryUpdate {
            server_id: release.server_id,
            version: release.version,
            drifts,
        })
    }

    pub fn latest_publisher(&self, server_id: &str) -> Option<&McpPublisherMetadata> {
        self.publishers.get(server_id)
    }

    pub fn visible_tools(&self, server_id: &str) -> Vec<RegisteredMcpTool> {
        self.visible
            .iter()
            .filter(|((server, _), _)| server == server_id)
            .map(|(_, tool)| tool.clone())
            .collect()
    }

    pub fn visible_tool(&self, server_id: &str, tool_name: &str) -> Option<RegisteredMcpTool> {
        self.visible
            .get(&(server_id.to_owned(), tool_name.to_owned()))
            .cloned()
    }

    pub fn pending_tool(&self, server_id: &str, tool_name: &str) -> Option<RegisteredMcpTool> {
        self.pending
            .get(&(server_id.to_owned(), tool_name.to_owned()))
            .cloned()
    }

    pub fn approve_tool(
        &mut self,
        server_id: &str,
        tool_name: &str,
        release_version: &str,
        schema_hash: &str,
    ) -> Result<RegisteredMcpTool, RegistryError> {
        let key = (server_id.to_owned(), tool_name.to_owned());
        let pending = self
            .pending
            .get(&key)
            .ok_or_else(|| RegistryError::PendingToolNotFound {
                server_id: server_id.into(),
                tool_name: tool_name.into(),
            })?;
        if pending.release_version != release_version || pending.schema_hash != schema_hash {
            return Err(RegistryError::ApprovalMismatch {
                server_id: server_id.into(),
                tool_name: tool_name.into(),
            });
        }
        let approved = self
            .pending
            .remove(&key)
            .expect("pending tool checked above");
        self.visible.insert(key, approved.clone());
        Ok(approved)
    }
}

fn validate_release(release: &McpRelease) -> Result<(), RegistryError> {
    if release.server_id.trim().is_empty()
        || release.version.trim().is_empty()
        || release.publisher.id.trim().is_empty()
    {
        return Err(RegistryError::InvalidRelease {
            message: "server id, version and publisher id are required".into(),
        });
    }
    for tool in &release.tools {
        if tool.name.trim().is_empty() || !tool.input_schema.is_object() {
            return Err(RegistryError::InvalidRelease {
                message: format!("tool `{}` must have a name and object schema", tool.name),
            });
        }
    }
    if let Some(signature) = &release.publisher.signature {
        if signature.algorithm.trim().is_empty()
            || signature.key_id.trim().is_empty()
            || signature.digest.trim().is_empty()
        {
            return Err(RegistryError::InvalidRelease {
                message: "signature algorithm, key id and digest are required".into(),
            });
        }
    }
    Ok(())
}

fn registered_tool(release: &McpRelease, tool: McpToolDefinition) -> RegisteredMcpTool {
    RegisteredMcpTool {
        server_id: release.server_id.clone(),
        release_version: release.version.clone(),
        publisher: release.publisher.clone(),
        schema_hash: schema_hash(&tool.input_schema),
        name: tool.name,
        description: tool.description,
        input_schema: tool.input_schema,
    }
}

fn schema_hash(schema: &serde_json::Value) -> String {
    let canonical = canonicalize_json(schema).to_string();
    format!("{:x}", Sha256::digest(canonical.as_bytes()))
}

fn canonicalize_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => values
            .iter()
            .map(canonicalize_json)
            .collect::<Vec<_>>()
            .into(),
        serde_json::Value::Object(values) => {
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_unstable_by_key(|(key, _)| *key);
            serde_json::Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key.clone(), canonicalize_json(value)))
                    .collect(),
            )
        }
        scalar => scalar.clone(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    #[error("invalid MCP release: {message}")]
    InvalidRelease { message: String },
    #[error("pending MCP tool `{server_id}/{tool_name}` was not found")]
    PendingToolNotFound {
        server_id: String,
        tool_name: String,
    },
    #[error("approval does not match pending MCP tool `{server_id}/{tool_name}`")]
    ApprovalMismatch {
        server_id: String,
        tool_name: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitPolicy {
    pub max_requests: u32,
    pub window: Duration,
}

#[derive(Debug, Clone)]
pub struct RateLimiter {
    policy: RateLimitPolicy,
    window_started: Instant,
    used: u32,
}

impl RateLimiter {
    pub fn new(policy: RateLimitPolicy, now: Instant) -> Self {
        Self {
            policy,
            window_started: now,
            used: 0,
        }
    }

    pub fn acquire(&mut self, now: Instant) -> Result<(), RateLimitError> {
        let mut elapsed = now
            .checked_duration_since(self.window_started)
            .unwrap_or_default();
        if elapsed >= self.policy.window {
            self.window_started = now;
            self.used = 0;
            elapsed = Duration::ZERO;
        }
        if self.used >= self.policy.max_requests {
            return Err(RateLimitError::Limited {
                retry_after: self.policy.window.saturating_sub(elapsed),
            });
        }
        self.used = self.used.saturating_add(1);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RateLimitError {
    #[error("MCP tool rate limited; retry after {retry_after:?}")]
    Limited { retry_after: Duration },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CircuitBreakerConfig {
    pub failure_threshold: u32,
    pub base_backoff: Duration,
    pub max_backoff: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitPhase {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CircuitPermit {
    half_open_probe: bool,
}

impl CircuitPermit {
    pub fn is_half_open_probe(self) -> bool {
        self.half_open_probe
    }
}

#[derive(Debug, Clone, Copy)]
enum CircuitState {
    Closed {
        failures: u32,
    },
    Open {
        retry_at: Instant,
        recovery_failures: u32,
    },
    HalfOpen {
        recovery_failures: u32,
        probe_in_flight: bool,
    },
}

#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    state: CircuitState,
}

impl CircuitBreaker {
    pub fn new(mut config: CircuitBreakerConfig) -> Self {
        config.failure_threshold = config.failure_threshold.max(1);
        config.max_backoff = config.max_backoff.max(config.base_backoff);
        Self {
            config,
            state: CircuitState::Closed { failures: 0 },
        }
    }

    pub fn phase(&self) -> CircuitPhase {
        match self.state {
            CircuitState::Closed { .. } => CircuitPhase::Closed,
            CircuitState::Open { .. } => CircuitPhase::Open,
            CircuitState::HalfOpen { .. } => CircuitPhase::HalfOpen,
        }
    }

    pub fn before_call(&mut self, now: Instant) -> Result<CircuitPermit, CircuitError> {
        match self.state {
            CircuitState::Closed { .. } => Ok(CircuitPermit {
                half_open_probe: false,
            }),
            CircuitState::Open {
                retry_at,
                recovery_failures,
            } if now >= retry_at => {
                self.state = CircuitState::HalfOpen {
                    recovery_failures,
                    probe_in_flight: true,
                };
                Ok(CircuitPermit {
                    half_open_probe: true,
                })
            }
            CircuitState::Open { retry_at, .. } => Err(CircuitError::Open {
                retry_after: retry_at.checked_duration_since(now).unwrap_or_default(),
            }),
            CircuitState::HalfOpen {
                probe_in_flight: true,
                ..
            } => Err(CircuitError::HalfOpenProbeInFlight),
            CircuitState::HalfOpen {
                recovery_failures,
                probe_in_flight: false,
            } => {
                self.state = CircuitState::HalfOpen {
                    recovery_failures,
                    probe_in_flight: true,
                };
                Ok(CircuitPermit {
                    half_open_probe: true,
                })
            }
        }
    }

    pub fn record_failure(&mut self, now: Instant) {
        self.state = match self.state {
            CircuitState::Closed { failures } => {
                let failures = failures.saturating_add(1);
                if failures >= self.config.failure_threshold {
                    CircuitState::Open {
                        retry_at: now + self.config.base_backoff,
                        recovery_failures: 0,
                    }
                } else {
                    CircuitState::Closed { failures }
                }
            }
            CircuitState::HalfOpen {
                recovery_failures, ..
            } => {
                let recovery_failures = recovery_failures.saturating_add(1);
                CircuitState::Open {
                    retry_at: now + self.backoff(recovery_failures),
                    recovery_failures,
                }
            }
            open @ CircuitState::Open { .. } => open,
        };
    }

    pub fn record_success(&mut self) {
        self.state = CircuitState::Closed { failures: 0 };
    }

    fn backoff(&self, recovery_failures: u32) -> Duration {
        let exponent = recovery_failures.min(31);
        self.config
            .base_backoff
            .checked_mul(1_u32 << exponent)
            .unwrap_or(self.config.max_backoff)
            .min(self.config.max_backoff)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CircuitError {
    #[error("MCP circuit is open; retry after {retry_after:?}")]
    Open { retry_after: Duration },
    #[error("MCP circuit half-open probe is already in flight")]
    HalfOpenProbeInFlight,
}
