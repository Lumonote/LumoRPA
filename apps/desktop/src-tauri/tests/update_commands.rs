#[path = "../src/update_commands.rs"]
mod update_commands;

use std::collections::BTreeMap;
use update_commands::*;

struct ExactVerifier {
    expected_payload: Vec<u8>,
}

impl SignatureVerifier for ExactVerifier {
    fn verify(&self, key_id: &str, payload: &[u8], signature: &str) -> Result<(), String> {
        if key_id == "release-2026"
            && signature == "valid-signature"
            && payload == self.expected_payload
        {
            Ok(())
        } else {
            Err("signature mismatch".into())
        }
    }
}

fn artifact() -> &'static [u8] {
    b"signed desktop update artifact"
}

fn metadata(channel: UpdateChannel) -> UpdateMetadata {
    UpdateMetadata {
        version: "2.4.0".into(),
        channel,
        artifact_url: "https://updates.example/lumo-2.4.0.tar.zst".into(),
        artifact_sha256: sha256_hex(artifact()),
        key_id: "release-2026".into(),
        signature: "valid-signature".into(),
        published_at: "2026-07-12T00:00:00Z".into(),
        model_runtimes: vec![ModelRuntimeRequirement {
            runtime_id: "sherpa-onnx".into(),
            min_version: "1.11.0".into(),
            max_version_exclusive: Some("2.0.0".into()),
        }],
        rollback: None,
    }
}

fn inventory(version: &str) -> ModelRuntimeInventory {
    ModelRuntimeInventory {
        versions: BTreeMap::from([("sherpa-onnx".into(), version.into())]),
    }
}

fn policy(channel: UpdateChannel) -> UpdatePolicy {
    UpdatePolicy {
        channel,
        current_version: "2.3.0".into(),
        allow_rollback: false,
    }
}

fn verifier(metadata: &UpdateMetadata) -> ExactVerifier {
    ExactVerifier {
        expected_payload: metadata.signing_payload().unwrap(),
    }
}

#[test]
fn signed_metadata_and_artifact_hash_are_both_required() {
    let metadata = metadata(UpdateChannel::Stable);
    let verified = verify_update(
        &metadata,
        artifact(),
        &policy(UpdateChannel::Stable),
        &inventory("1.12.0"),
        &verifier(&metadata),
    )
    .unwrap();
    assert_eq!(verified.version, "2.4.0");
    assert_eq!(verified.artifact_sha256, metadata.artifact_sha256);

    let mut tampered = metadata.clone();
    tampered.version = "2.5.0".into();
    assert!(matches!(
        verify_update(
            &tampered,
            artifact(),
            &policy(UpdateChannel::Stable),
            &inventory("1.12.0"),
            &verifier(&metadata),
        ),
        Err(UpdateError::Signature(_))
    ));
    assert!(matches!(
        verify_update(
            &metadata,
            b"tampered artifact",
            &policy(UpdateChannel::Stable),
            &inventory("1.12.0"),
            &verifier(&metadata),
        ),
        Err(UpdateError::ArtifactHash { .. })
    ));
}

#[test]
fn channel_policy_blocks_prerelease_for_stable_users() {
    let beta = metadata(UpdateChannel::Beta);
    assert!(matches!(
        verify_update(
            &beta,
            artifact(),
            &policy(UpdateChannel::Stable),
            &inventory("1.12.0"),
            &verifier(&beta),
        ),
        Err(UpdateError::ChannelDenied { .. })
    ));

    verify_update(
        &beta,
        artifact(),
        &policy(UpdateChannel::Beta),
        &inventory("1.12.0"),
        &verifier(&beta),
    )
    .unwrap();
}

#[test]
fn model_runtime_must_exist_inside_the_declared_version_range() {
    let metadata = metadata(UpdateChannel::Stable);
    assert!(matches!(
        verify_update(
            &metadata,
            artifact(),
            &policy(UpdateChannel::Stable),
            &inventory("1.10.9"),
            &verifier(&metadata),
        ),
        Err(UpdateError::ModelRuntimeIncompatible { .. })
    ));
    assert!(matches!(
        verify_update(
            &metadata,
            artifact(),
            &policy(UpdateChannel::Stable),
            &ModelRuntimeInventory::default(),
            &verifier(&metadata),
        ),
        Err(UpdateError::ModelRuntimeMissing { .. })
    ));
}

#[test]
fn rollback_channel_requires_explicit_matching_rollback_metadata() {
    let mut rollback = metadata(UpdateChannel::Rollback);
    rollback.version = "2.2.5".into();
    rollback.rollback = Some(RollbackMetadata {
        from_version: "2.3.0".into(),
        target_version: "2.2.5".into(),
        reason: "2.3.0 startup regression".into(),
        issued_at: "2026-07-12T01:00:00Z".into(),
    });
    let mut rollback_policy = policy(UpdateChannel::Stable);
    rollback_policy.allow_rollback = true;

    let verified = verify_update(
        &rollback,
        artifact(),
        &rollback_policy,
        &inventory("1.12.0"),
        &verifier(&rollback),
    )
    .unwrap();
    assert_eq!(verified.rollback.unwrap().target_version, "2.2.5");

    let mut missing = rollback.clone();
    missing.rollback = None;
    assert!(matches!(
        verify_update(
            &missing,
            artifact(),
            &rollback_policy,
            &inventory("1.12.0"),
            &verifier(&missing),
        ),
        Err(UpdateError::RollbackMetadata(_))
    ));

    let mut denied_policy = rollback_policy;
    denied_policy.allow_rollback = false;
    assert!(matches!(
        verify_update(
            &rollback,
            artifact(),
            &denied_policy,
            &inventory("1.12.0"),
            &verifier(&rollback),
        ),
        Err(UpdateError::ChannelDenied { .. })
    ));
}
