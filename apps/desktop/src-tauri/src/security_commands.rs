use chrono::{DateTime, Utc};
use lumo_agent::{
    BiometricChallenge, InjectionFinding, PermissionGrant, PermissionGrantRequest,
    PermissionRevocation, PermissionRevocationRequest, PlatformAuthenticator, RiskLevel,
    SecurityCenter,
};
use serde::Serialize;
use std::sync::{Arc, Mutex, MutexGuard};

#[allow(dead_code)]
pub(super) trait LocalAuthenticationClient: Send + Sync {
    fn evaluate_device_owner_authentication(&self, reason: &str) -> Result<(), String>;
}

#[allow(dead_code)]
pub(super) struct MacOsLocalAuthentication<C> {
    client: C,
}

impl<C> MacOsLocalAuthentication<C> {
    #[allow(dead_code)]
    pub(super) const fn new(client: C) -> Self {
        Self { client }
    }
}

pub(super) trait DesktopAuthenticator: Send + Sync {
    fn authenticate(&self, challenge: &BiometricChallenge) -> Result<(), String>;
}

impl<C> DesktopAuthenticator for MacOsLocalAuthentication<C>
where
    C: LocalAuthenticationClient,
{
    fn authenticate(&self, challenge: &BiometricChallenge) -> Result<(), String> {
        self.client.evaluate_device_owner_authentication(&format!(
            "L3 approval for {} ({})",
            challenge.capability_pattern, challenge.reason
        ))
    }
}

pub(super) struct RejectingAuthenticator;

impl DesktopAuthenticator for RejectingAuthenticator {
    fn authenticate(&self, _challenge: &BiometricChallenge) -> Result<(), String> {
        Err("platform biometric authentication is unavailable".into())
    }
}

pub(super) struct DesktopSecurityRuntime {
    center: Mutex<SecurityCenter>,
    authenticator: Arc<dyn DesktopAuthenticator>,
}

impl Default for DesktopSecurityRuntime {
    fn default() -> Self { Self::new(Arc::new(RejectingAuthenticator)) }
}

impl DesktopSecurityRuntime {
    pub(super) fn new(authenticator: Arc<dyn DesktopAuthenticator>) -> Self {
        Self::from_center(SecurityCenter::new(), authenticator)
    }

    pub(super) fn from_center(
        center: SecurityCenter,
        authenticator: Arc<dyn DesktopAuthenticator>,
    ) -> Self {
        Self {
            center: Mutex::new(center),
            authenticator,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SecuritySnapshotDto {
    pub(super) grants: Vec<PermissionGrant>,
    pub(super) active_grant_ids: Vec<String>,
    pub(super) revocations: Vec<PermissionRevocation>,
    pub(super) injection_findings: Vec<InjectionFinding>,
}

pub(super) fn security_list(
    runtime: &DesktopSecurityRuntime,
    now: DateTime<Utc>,
) -> Result<SecuritySnapshotDto, String> {
    let center = lock_center(runtime)?;
    let grants = center.grant_history().to_vec();
    let active_grant_ids = grants
        .iter()
        .filter(|grant| {
            center.is_allowed(
                grant.subject(),
                grant.capability_pattern(),
                grant.max_risk(),
                now,
            )
        })
        .map(|grant| grant.id().to_string())
        .collect();
    Ok(SecuritySnapshotDto {
        grants,
        active_grant_ids,
        revocations: center.revocation_history().to_vec(),
        injection_findings: center.injection_findings().to_vec(),
    })
}

pub(super) fn security_revoke(
    runtime: &DesktopSecurityRuntime,
    request: PermissionRevocationRequest,
    now: DateTime<Utc>,
) -> Result<PermissionRevocation, String> {
    lock_center(runtime)?
        .revoke(request, now)
        .map_err(|error| error.to_string())
}

pub(super) fn security_export_audit(runtime: &DesktopSecurityRuntime) -> Result<String, String> {
    lock_center(runtime)?
        .export_redacted_audit()
        .map_err(|error| error.to_string())
}

pub(super) fn security_biometric_challenge(
    runtime: &DesktopSecurityRuntime,
    request: PermissionGrantRequest,
    now: DateTime<Utc>,
) -> Result<PermissionGrant, String> {
    if request.max_risk != RiskLevel::L3 {
        return Err("biometric challenge is reserved for L3 grants".into());
    }
    let bridge = AuthenticatorBridge(runtime.authenticator.as_ref());
    lock_center(runtime)?
        .grant(request, now, Some(&bridge))
        .map_err(|error| error.to_string())
}

struct AuthenticatorBridge<'a>(&'a dyn DesktopAuthenticator);

impl PlatformAuthenticator for AuthenticatorBridge<'_> {
    fn authenticate(&self, challenge: &BiometricChallenge) -> Result<(), String> {
        self.0.authenticate(challenge)
    }
}

fn lock_center(runtime: &DesktopSecurityRuntime) -> Result<MutexGuard<'_, SecurityCenter>, String> {
    runtime
        .center
        .lock()
        .map_err(|_| "security center is unavailable".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone, Utc};
    use lumo_agent::{
        AuditEventDraft, ContentOrigin, PermissionGrantRequest, PermissionRevocationRequest,
        RiskLevel, SecurityCenter,
    };
    use serde_json::json;
    use std::sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    };

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 12, 8, 0, 0).unwrap()
    }

    fn request(id: &str, risk: RiskLevel) -> PermissionGrantRequest {
        PermissionGrantRequest {
            id: id.into(),
            subject: "profile-1".into(),
            capability_pattern: "mcp:orders/*".into(),
            max_risk: risk,
            actor: "alice".into(),
            origin: ContentOrigin::User,
            expires_at: now() + Duration::hours(1),
        }
    }

    struct FakeLocalAuthentication {
        accepted: bool,
        calls: Arc<AtomicU32>,
    }

    impl LocalAuthenticationClient for FakeLocalAuthentication {
        fn evaluate_device_owner_authentication(&self, reason: &str) -> Result<(), String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert!(reason.contains("L3"));
            if self.accepted {
                Ok(())
            } else {
                Err("user cancelled".into())
            }
        }
    }

    #[test]
    fn biometric_command_routes_l3_through_local_authentication_boundary() {
        let calls = Arc::new(AtomicU32::new(0));
        let authenticator = MacOsLocalAuthentication::new(FakeLocalAuthentication {
            accepted: true,
            calls: calls.clone(),
        });
        let runtime = DesktopSecurityRuntime::new(Arc::new(authenticator));

        let grant =
            security_biometric_challenge(&runtime, request("l3", RiskLevel::L3), now()).unwrap();
        assert_eq!(grant.id(), "l3");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn rejected_biometric_challenge_does_not_create_a_grant() {
        let authenticator = MacOsLocalAuthentication::new(FakeLocalAuthentication {
            accepted: false,
            calls: Arc::new(AtomicU32::new(0)),
        });
        let runtime = DesktopSecurityRuntime::new(Arc::new(authenticator));

        assert!(
            security_biometric_challenge(&runtime, request("l3", RiskLevel::L3), now()).is_err()
        );
        assert!(security_list(&runtime, now()).unwrap().grants.is_empty());
    }

    #[test]
    fn list_and_revoke_preserve_immutable_history_and_active_grants() {
        let mut center = SecurityCenter::new();
        center
            .grant(request("grant-1", RiskLevel::L1), now(), None)
            .unwrap();
        let runtime = DesktopSecurityRuntime::from_center(center, Arc::new(RejectingAuthenticator));

        let before = security_list(&runtime, now()).unwrap();
        assert_eq!(before.grants.len(), 1);
        assert_eq!(before.active_grant_ids, ["grant-1"]);

        security_revoke(
            &runtime,
            PermissionRevocationRequest {
                id: "revoke-1".into(),
                grant_id: "grant-1".into(),
                actor: "alice".into(),
                reason: "finished".into(),
                origin: ContentOrigin::User,
            },
            now() + Duration::minutes(1),
        )
        .unwrap();
        let after = security_list(&runtime, now() + Duration::minutes(2)).unwrap();
        assert_eq!(after.grants.len(), 1);
        assert_eq!(after.revocations.len(), 1);
        assert!(after.active_grant_ids.is_empty());
    }

    #[test]
    fn audit_export_command_returns_only_redacted_content() {
        let mut center = SecurityCenter::new();
        center.record_audit(AuditEventDraft {
            kind: "tool.call".into(),
            actor: "agent".into(),
            capability_id: Some("mcp:orders/list".into()),
            arguments: json!({"token": "raw-token", "query": "orders"}),
            created_at: now(),
        });
        let runtime = DesktopSecurityRuntime::from_center(center, Arc::new(RejectingAuthenticator));

        let exported = security_export_audit(&runtime).unwrap();
        assert!(exported.contains("[REDACTED]"));
        assert!(!exported.contains("raw-token"));
    }
}
