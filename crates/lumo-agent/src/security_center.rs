use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{ContentOrigin, RiskLevel};

const REDACTED: &str = "[REDACTED]";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionGrantRequest {
    pub id: String,
    pub subject: String,
    pub capability_pattern: String,
    pub max_risk: RiskLevel,
    pub actor: String,
    pub origin: ContentOrigin,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionGrant {
    id: String,
    subject: String,
    capability_pattern: String,
    max_risk: RiskLevel,
    actor: String,
    origin: ContentOrigin,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    biometric_verified: bool,
}

impl PermissionGrant {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn capability_pattern(&self) -> &str {
        &self.capability_pattern
    }

    pub const fn max_risk(&self) -> RiskLevel {
        self.max_risk
    }

    pub const fn expires_at(&self) -> &DateTime<Utc> {
        &self.expires_at
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRevocationRequest {
    pub id: String,
    pub grant_id: String,
    pub actor: String,
    pub reason: String,
    pub origin: ContentOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRevocation {
    id: String,
    grant_id: String,
    actor: String,
    reason: String,
    origin: ContentOrigin,
    created_at: DateTime<Utc>,
}

impl PermissionRevocation {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn grant_id(&self) -> &str {
        &self.grant_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BiometricChallenge {
    pub subject: String,
    pub capability_pattern: String,
    pub risk: RiskLevel,
    pub reason: String,
}

pub trait PlatformAuthenticator {
    fn authenticate(&self, challenge: &BiometricChallenge) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEventDraft {
    pub kind: String,
    pub actor: String,
    pub capability_id: Option<String>,
    pub arguments: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEvent {
    kind: String,
    actor: String,
    capability_id: Option<String>,
    arguments: Value,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InjectionSeverity {
    High,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InjectionFinding {
    pub kind: String,
    pub origin: ContentOrigin,
    pub severity: InjectionSeverity,
    pub indicators: Vec<String>,
    pub content_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Default)]
pub struct SecurityCenter {
    grants: Vec<PermissionGrant>,
    revocations: Vec<PermissionRevocation>,
    injection_findings: Vec<InjectionFinding>,
    audit_events: Vec<AuditEvent>,
}

impl SecurityCenter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn grant(
        &mut self,
        request: PermissionGrantRequest,
        now: DateTime<Utc>,
        authenticator: Option<&dyn PlatformAuthenticator>,
    ) -> Result<PermissionGrant, SecurityCenterError> {
        require_trusted_mutation(request.origin)?;
        validate_required(&request.id, "grant id")?;
        validate_required(&request.subject, "subject")?;
        validate_required(&request.capability_pattern, "capability pattern")?;
        validate_required(&request.actor, "actor")?;
        if request.expires_at <= now {
            return Err(SecurityCenterError::InvalidExpiry);
        }
        if self.grants.iter().any(|grant| grant.id == request.id) {
            return Err(SecurityCenterError::DuplicateRecord(request.id));
        }

        let biometric_verified = if request.max_risk == RiskLevel::L3 {
            let authenticator = authenticator.ok_or(SecurityCenterError::BiometricRequired)?;
            authenticator
                .authenticate(&BiometricChallenge {
                    subject: request.subject.clone(),
                    capability_pattern: request.capability_pattern.clone(),
                    risk: RiskLevel::L3,
                    reason: "Approve L3 capability access".into(),
                })
                .map_err(SecurityCenterError::AuthenticationFailed)?;
            true
        } else {
            false
        };

        let grant = PermissionGrant {
            id: request.id,
            subject: request.subject,
            capability_pattern: request.capability_pattern,
            max_risk: request.max_risk,
            actor: request.actor,
            origin: request.origin,
            created_at: now,
            expires_at: request.expires_at,
            biometric_verified,
        };
        self.grants.push(grant.clone());
        Ok(grant)
    }

    pub fn revoke(
        &mut self,
        request: PermissionRevocationRequest,
        now: DateTime<Utc>,
    ) -> Result<PermissionRevocation, SecurityCenterError> {
        require_trusted_mutation(request.origin)?;
        validate_required(&request.id, "revocation id")?;
        validate_required(&request.actor, "actor")?;
        validate_required(&request.reason, "reason")?;
        if self
            .revocations
            .iter()
            .any(|revocation| revocation.id == request.id)
        {
            return Err(SecurityCenterError::DuplicateRecord(request.id));
        }
        if !self.grants.iter().any(|grant| grant.id == request.grant_id) {
            return Err(SecurityCenterError::GrantNotFound(request.grant_id));
        }
        if self
            .revocations
            .iter()
            .any(|revocation| revocation.grant_id == request.grant_id)
        {
            return Err(SecurityCenterError::AlreadyRevoked(request.grant_id));
        }
        let revocation = PermissionRevocation {
            id: request.id,
            grant_id: request.grant_id,
            actor: request.actor,
            reason: request.reason,
            origin: request.origin,
            created_at: now,
        };
        self.revocations.push(revocation.clone());
        Ok(revocation)
    }

    pub fn is_allowed(
        &self,
        subject: &str,
        capability_id: &str,
        risk: RiskLevel,
        now: DateTime<Utc>,
    ) -> bool {
        self.grants.iter().any(|grant| {
            grant.subject == subject
                && grant.created_at <= now
                && now < grant.expires_at
                && risk <= grant.max_risk
                && pattern_matches(&grant.capability_pattern, capability_id)
                && !self.revocations.iter().any(|revocation| {
                    revocation.grant_id == grant.id && revocation.created_at <= now
                })
        })
    }

    pub fn grant_history(&self) -> &[PermissionGrant] {
        &self.grants
    }

    pub fn revocation_history(&self) -> &[PermissionRevocation] {
        &self.revocations
    }

    pub fn injection_findings(&self) -> &[InjectionFinding] {
        &self.injection_findings
    }

    pub fn inspect_content(
        &mut self,
        origin: ContentOrigin,
        content: &str,
        now: DateTime<Utc>,
    ) -> Option<InjectionFinding> {
        let normalized = content.to_ascii_lowercase();
        let indicators: Vec<String> = [
            ("ignore previous instructions", "instruction_override"),
            ("ignore all previous instructions", "instruction_override"),
            ("reveal the vault", "secret_exfiltration"),
            ("print secrets", "secret_exfiltration"),
            ("system prompt", "system_prompt_exfiltration"),
            ("disable security", "security_bypass"),
        ]
        .into_iter()
        .filter_map(|(needle, indicator)| normalized.contains(needle).then_some(indicator.into()))
        .collect::<Vec<_>>();
        if indicators.is_empty() {
            return None;
        }
        let severity = if indicators.iter().any(|indicator| {
            matches!(
                indicator.as_str(),
                "secret_exfiltration" | "security_bypass"
            )
        }) {
            InjectionSeverity::Critical
        } else {
            InjectionSeverity::High
        };
        let finding = InjectionFinding {
            kind: "prompt_injection".into(),
            origin,
            severity,
            indicators,
            content_hash: format!("{:x}", Sha256::digest(content.as_bytes())),
            created_at: now,
        };
        self.injection_findings.push(finding.clone());
        Some(finding)
    }

    pub fn record_audit(&mut self, draft: AuditEventDraft) {
        self.audit_events.push(AuditEvent {
            kind: draft.kind,
            actor: draft.actor,
            capability_id: draft.capability_id,
            arguments: redact_arguments(&draft.arguments),
            created_at: draft.created_at,
        });
    }

    pub fn export_redacted_audit(&self) -> Result<String, SecurityCenterError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Export<'a> {
            grants: &'a [PermissionGrant],
            revocations: &'a [PermissionRevocation],
            injection_findings: &'a [InjectionFinding],
            audit_events: &'a [AuditEvent],
        }

        serde_json::to_string_pretty(&Export {
            grants: &self.grants,
            revocations: &self.revocations,
            injection_findings: &self.injection_findings,
            audit_events: &self.audit_events,
        })
        .map_err(Into::into)
    }
}

pub fn redact_arguments(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| {
                    let value = if is_sensitive_key(key) {
                        Value::String(REDACTED.into())
                    } else {
                        redact_arguments(value)
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(redact_arguments).collect()),
        scalar => scalar.clone(),
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    [
        "authorization",
        "apikey",
        "accesstoken",
        "refreshtoken",
        "token",
        "password",
        "passwd",
        "secret",
        "cookie",
        "privatekey",
    ]
    .iter()
    .any(|sensitive| normalized == *sensitive || normalized.ends_with(sensitive))
}

fn require_trusted_mutation(origin: ContentOrigin) -> Result<(), SecurityCenterError> {
    if matches!(origin, ContentOrigin::User | ContentOrigin::CodeOwned) {
        Ok(())
    } else {
        Err(SecurityCenterError::UntrustedMutation(origin))
    }
}

fn validate_required(value: &str, field: &'static str) -> Result<(), SecurityCenterError> {
    if value.trim().is_empty() {
        Err(SecurityCenterError::MissingField(field))
    } else {
        Ok(())
    }
}

fn pattern_matches(pattern: &str, value: &str) -> bool {
    if pattern == "*" || pattern == value {
        return true;
    }
    match pattern.split_once('*') {
        Some((prefix, suffix)) => value.starts_with(prefix) && value.ends_with(suffix),
        None => false,
    }
}

#[derive(Debug, Error)]
pub enum SecurityCenterError {
    #[error("untrusted {0:?} content cannot create approval or revocation records")]
    UntrustedMutation(ContentOrigin),
    #[error("L3 permission requires platform biometric authentication")]
    BiometricRequired,
    #[error("platform authentication failed: {0}")]
    AuthenticationFailed(String),
    #[error("permission expiry must be in the future")]
    InvalidExpiry,
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("duplicate immutable security record `{0}`")]
    DuplicateRecord(String),
    #[error("permission grant `{0}` was not found")]
    GrantNotFound(String),
    #[error("permission grant `{0}` is already revoked")]
    AlreadyRevoked(String),
    #[error("audit serialization failed: {0}")]
    Json(#[from] serde_json::Error),
}
