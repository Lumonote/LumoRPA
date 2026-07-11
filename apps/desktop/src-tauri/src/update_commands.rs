use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{cmp::Ordering, collections::BTreeMap, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateChannel {
    Stable,
    Beta,
    Rollback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRuntimeRequirement {
    pub runtime_id: String,
    pub min_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_version_exclusive: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackMetadata {
    pub from_version: String,
    pub target_version: String,
    pub reason: String,
    pub issued_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMetadata {
    pub version: String,
    pub channel: UpdateChannel,
    pub artifact_url: String,
    pub artifact_sha256: String,
    pub key_id: String,
    pub signature: String,
    pub published_at: String,
    #[serde(default)]
    pub model_runtimes: Vec<ModelRuntimeRequirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback: Option<RollbackMetadata>,
}

impl UpdateMetadata {
    pub fn signing_payload(&self) -> Result<Vec<u8>, UpdateError> {
        serde_json::to_vec(&UnsignedUpdateMetadata {
            version: &self.version,
            channel: self.channel,
            artifact_url: &self.artifact_url,
            artifact_sha256: &self.artifact_sha256,
            key_id: &self.key_id,
            published_at: &self.published_at,
            model_runtimes: &self.model_runtimes,
            rollback: self.rollback.as_ref(),
        })
        .map_err(|error| UpdateError::InvalidMetadata(error.to_string()))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UnsignedUpdateMetadata<'a> {
    version: &'a str,
    channel: UpdateChannel,
    artifact_url: &'a str,
    artifact_sha256: &'a str,
    key_id: &'a str,
    published_at: &'a str,
    model_runtimes: &'a [ModelRuntimeRequirement],
    #[serde(skip_serializing_if = "Option::is_none")]
    rollback: Option<&'a RollbackMetadata>,
}

pub trait SignatureVerifier: Send + Sync {
    fn verify(&self, key_id: &str, payload: &[u8], signature: &str) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatePolicy {
    pub channel: UpdateChannel,
    pub current_version: String,
    pub allow_rollback: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRuntimeInventory {
    pub versions: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedUpdate {
    pub version: String,
    pub channel: UpdateChannel,
    pub artifact_url: String,
    pub artifact_sha256: String,
    pub rollback: Option<RollbackMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    InvalidMetadata(String),
    Signature(String),
    ArtifactHash {
        expected: String,
        actual: String,
    },
    ChannelDenied {
        configured: UpdateChannel,
        offered: UpdateChannel,
    },
    VersionPolicy(String),
    ModelRuntimeMissing {
        runtime_id: String,
    },
    ModelRuntimeIncompatible {
        runtime_id: String,
        installed: String,
        required: String,
    },
    RollbackMetadata(String),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMetadata(error) => write!(formatter, "invalid update metadata: {error}"),
            Self::Signature(error) => write!(formatter, "update signature rejected: {error}"),
            Self::ArtifactHash { expected, actual } => write!(
                formatter,
                "update artifact hash mismatch: expected {expected}, got {actual}"
            ),
            Self::ChannelDenied {
                configured,
                offered,
            } => write!(
                formatter,
                "update channel {offered:?} is denied by {configured:?} policy"
            ),
            Self::VersionPolicy(error) => write!(formatter, "update version rejected: {error}"),
            Self::ModelRuntimeMissing { runtime_id } => {
                write!(
                    formatter,
                    "required model runtime `{runtime_id}` is missing"
                )
            }
            Self::ModelRuntimeIncompatible {
                runtime_id,
                installed,
                required,
            } => write!(
                formatter,
                "model runtime `{runtime_id}` version {installed} does not satisfy {required}"
            ),
            Self::RollbackMetadata(error) => {
                write!(formatter, "rollback metadata rejected: {error}")
            }
        }
    }
}

impl std::error::Error for UpdateError {}

pub fn verify_update(
    metadata: &UpdateMetadata,
    artifact: &[u8],
    policy: &UpdatePolicy,
    runtimes: &ModelRuntimeInventory,
    signature_verifier: &dyn SignatureVerifier,
) -> Result<VerifiedUpdate, UpdateError> {
    let payload = metadata.signing_payload()?;
    signature_verifier
        .verify(&metadata.key_id, &payload, &metadata.signature)
        .map_err(UpdateError::Signature)?;
    validate_metadata(metadata)?;

    let actual_sha256 = sha256_hex(artifact);
    if !actual_sha256.eq_ignore_ascii_case(&metadata.artifact_sha256) {
        return Err(UpdateError::ArtifactHash {
            expected: metadata.artifact_sha256.to_ascii_lowercase(),
            actual: actual_sha256,
        });
    }
    validate_channel(metadata, policy)?;
    validate_version_policy(metadata, policy)?;
    validate_model_runtimes(&metadata.model_runtimes, runtimes)?;
    validate_rollback(metadata, policy)?;

    Ok(VerifiedUpdate {
        version: metadata.version.clone(),
        channel: metadata.channel,
        artifact_url: metadata.artifact_url.clone(),
        artifact_sha256: metadata.artifact_sha256.to_ascii_lowercase(),
        rollback: metadata.rollback.clone(),
    })
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn validate_metadata(metadata: &UpdateMetadata) -> Result<(), UpdateError> {
    parse_version(&metadata.version)?;
    if metadata.artifact_url.trim().is_empty() {
        return Err(UpdateError::InvalidMetadata(
            "artifact URL must not be empty".into(),
        ));
    }
    if metadata.key_id.trim().is_empty() || metadata.signature.trim().is_empty() {
        return Err(UpdateError::InvalidMetadata(
            "key id and signature must not be empty".into(),
        ));
    }
    if metadata.artifact_sha256.len() != 64
        || !metadata
            .artifact_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(UpdateError::InvalidMetadata(
            "artifact SHA-256 must contain 64 hexadecimal characters".into(),
        ));
    }
    Ok(())
}

fn validate_channel(metadata: &UpdateMetadata, policy: &UpdatePolicy) -> Result<(), UpdateError> {
    let allowed = match metadata.channel {
        UpdateChannel::Stable => {
            matches!(policy.channel, UpdateChannel::Stable | UpdateChannel::Beta)
        }
        UpdateChannel::Beta => policy.channel == UpdateChannel::Beta,
        UpdateChannel::Rollback => policy.allow_rollback,
    };
    if allowed {
        Ok(())
    } else {
        Err(UpdateError::ChannelDenied {
            configured: policy.channel,
            offered: metadata.channel,
        })
    }
}

fn validate_version_policy(
    metadata: &UpdateMetadata,
    policy: &UpdatePolicy,
) -> Result<(), UpdateError> {
    let offered = parse_version(&metadata.version)?;
    let current = parse_version(&policy.current_version)?;
    match metadata.channel {
        UpdateChannel::Rollback if offered >= current => Err(UpdateError::VersionPolicy(
            "rollback target must be older than the installed version".into(),
        )),
        UpdateChannel::Rollback => Ok(()),
        _ if offered <= current => Err(UpdateError::VersionPolicy(
            "normal updates must be newer than the installed version".into(),
        )),
        _ => Ok(()),
    }
}

fn validate_model_runtimes(
    requirements: &[ModelRuntimeRequirement],
    inventory: &ModelRuntimeInventory,
) -> Result<(), UpdateError> {
    for requirement in requirements {
        let installed = inventory
            .versions
            .get(&requirement.runtime_id)
            .ok_or_else(|| UpdateError::ModelRuntimeMissing {
                runtime_id: requirement.runtime_id.clone(),
            })?;
        let installed_version = parse_version(installed)?;
        let minimum = parse_version(&requirement.min_version)?;
        let maximum = requirement
            .max_version_exclusive
            .as_deref()
            .map(parse_version)
            .transpose()?;
        if installed_version < minimum
            || maximum
                .as_ref()
                .is_some_and(|maximum| installed_version >= *maximum)
        {
            let required = match &requirement.max_version_exclusive {
                Some(maximum) => format!(">={}, <{}", requirement.min_version, maximum),
                None => format!(">={}", requirement.min_version),
            };
            return Err(UpdateError::ModelRuntimeIncompatible {
                runtime_id: requirement.runtime_id.clone(),
                installed: installed.clone(),
                required,
            });
        }
    }
    Ok(())
}

fn validate_rollback(metadata: &UpdateMetadata, policy: &UpdatePolicy) -> Result<(), UpdateError> {
    match (metadata.channel, metadata.rollback.as_ref()) {
        (UpdateChannel::Rollback, Some(rollback)) => {
            if rollback.from_version != policy.current_version {
                return Err(UpdateError::RollbackMetadata(format!(
                    "fromVersion {} does not match installed version {}",
                    rollback.from_version, policy.current_version
                )));
            }
            if rollback.target_version != metadata.version {
                return Err(UpdateError::RollbackMetadata(format!(
                    "targetVersion {} does not match update version {}",
                    rollback.target_version, metadata.version
                )));
            }
            if rollback.reason.trim().is_empty() || rollback.issued_at.trim().is_empty() {
                return Err(UpdateError::RollbackMetadata(
                    "reason and issuedAt must not be empty".into(),
                ));
            }
            Ok(())
        }
        (UpdateChannel::Rollback, None) => Err(UpdateError::RollbackMetadata(
            "rollback channel requires rollback metadata".into(),
        )),
        (_, Some(_)) => Err(UpdateError::RollbackMetadata(
            "normal update channels cannot carry rollback metadata".into(),
        )),
        (_, None) => Ok(()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NumericVersion(Vec<u64>);

impl Ord for NumericVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        let length = self.0.len().max(other.0.len());
        (0..length)
            .map(|index| {
                self.0
                    .get(index)
                    .copied()
                    .unwrap_or(0)
                    .cmp(&other.0.get(index).copied().unwrap_or(0))
            })
            .find(|ordering| *ordering != Ordering::Equal)
            .unwrap_or(Ordering::Equal)
    }
}

impl PartialOrd for NumericVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn parse_version(value: &str) -> Result<NumericVersion, UpdateError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(UpdateError::InvalidMetadata(
            "version must not be empty".into(),
        ));
    }
    value
        .split('.')
        .map(|component| {
            component.parse::<u64>().map_err(|_| {
                UpdateError::InvalidMetadata(format!(
                    "version `{value}` must contain only numeric dot-separated components"
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(NumericVersion)
}
