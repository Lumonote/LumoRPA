//! Installer defaults with explicit runtime overrides for managed deployments.
//! Only public key/URLs/channel are embedded; private signing keys never are.

fn setting(runtime: Option<String>, builtin: &str) -> Option<String> {
    runtime
        .or_else(|| (!builtin.is_empty()).then(|| builtin.to_owned()))
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

pub(super) fn pubkey() -> Option<String> {
    setting(
        std::env::var("LUMO_UPDATER_PUBKEY").ok(),
        env!("LUMO_BUILTIN_UPDATER_PUBKEY"),
    )
}

pub(super) fn channel() -> String {
    setting(
        std::env::var("LUMO_UPDATE_CHANNEL").ok(),
        env!("LUMO_BUILTIN_UPDATE_CHANNEL"),
    )
    .unwrap_or_else(|| "stable".into())
}

fn https_endpoint(value: Option<String>) -> Option<String> {
    value.filter(|value| {
        reqwest::Url::parse(value).is_ok_and(|url| {
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
        })
    })
}

pub(super) fn endpoint(channel: &str) -> Option<String> {
    let (name, builtin) = match channel {
        "beta" => (
            "LUMO_UPDATE_BETA_ENDPOINT",
            env!("LUMO_BUILTIN_UPDATE_BETA_ENDPOINT"),
        ),
        "rollback" => (
            "LUMO_UPDATE_ROLLBACK_ENDPOINT",
            env!("LUMO_BUILTIN_UPDATE_ROLLBACK_ENDPOINT"),
        ),
        _ => ("LUMO_UPDATE_ENDPOINT", env!("LUMO_BUILTIN_UPDATE_ENDPOINT")),
    };
    https_endpoint(setting(std::env::var(name).ok(), builtin))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_app_uses_embedded_defaults_without_environment() {
        assert_eq!(setting(None, "public-key"), Some("public-key".into()));
        assert_eq!(
            https_endpoint(setting(None, "https://updates.example/latest.json")),
            Some("https://updates.example/latest.json".into())
        );
        assert_eq!(setting(None, "beta"), Some("beta".into()));
        assert_eq!(setting(None, ""), None);
    }

    #[test]
    fn explicit_override_can_disable_but_cannot_downgrade_https() {
        assert_eq!(
            setting(Some("override".into()), "built-in"),
            Some("override".into())
        );
        assert_eq!(setting(Some(" ".into()), "built-in"), None);
        for invalid in [
            "http://updates.example",
            "https://",
            "https://user:secret@updates.example",
        ] {
            assert_eq!(
                https_endpoint(setting(Some(invalid.into()), "https://safe.example")),
                None
            );
        }
    }
}
