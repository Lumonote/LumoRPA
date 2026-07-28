use super::AppHandle;
use serde::Serialize;
use tauri::Emitter;
use tauri_plugin_updater::UpdaterExt;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DesktopUpdateStatus {
    current_version: String,
    channel: String,
    configured: bool,
    endpoint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DesktopUpdateInfo {
    available: bool,
    current_version: String,
    version: Option<String>,
    body: Option<String>,
    date: Option<String>,
    target: Option<String>,
}

fn normalized_channel(channel: Option<String>) -> Result<String, String> {
    let channel = channel.unwrap_or_else(|| {
        std::env::var("LUMO_UPDATE_CHANNEL").unwrap_or_else(|_| "stable".into())
    });
    if matches!(channel.as_str(), "stable" | "beta" | "rollback") {
        Ok(channel)
    } else {
        Err("update channel must be stable, beta or rollback".into())
    }
}

fn endpoint_for(channel: &str) -> Option<String> {
    let key = match channel {
        "beta" => "LUMO_UPDATE_BETA_ENDPOINT",
        "rollback" => "LUMO_UPDATE_ROLLBACK_ENDPOINT",
        _ => "LUMO_UPDATE_ENDPOINT",
    };
    std::env::var(key)
        .ok()
        .filter(|value| value.starts_with("https://"))
}

async fn check_update(
    app: &AppHandle,
    channel: &str,
) -> Result<Option<tauri_plugin_updater::Update>, String> {
    let endpoint = endpoint_for(channel)
        .ok_or_else(|| format!("HTTPS updater endpoint for `{channel}` is not configured"))?;
    let pubkey = std::env::var("LUMO_UPDATER_PUBKEY")
        .map_err(|_| "LUMO_UPDATER_PUBKEY is not configured".to_string())?;
    let endpoint = endpoint
        .parse()
        .map_err(|error| format!("invalid updater endpoint: {error}"))?;
    app.updater_builder()
        .pubkey(pubkey)
        .endpoints(vec![endpoint])
        .map_err(|error| error.to_string())?
        .build()
        .map_err(|error| error.to_string())?
        .check()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(super) fn desktop_update_status(
    app: AppHandle,
    channel: Option<String>,
) -> Result<DesktopUpdateStatus, String> {
    let channel = normalized_channel(channel)?;
    let endpoint = endpoint_for(&channel);
    Ok(DesktopUpdateStatus {
        current_version: app.package_info().version.to_string(),
        channel,
        configured: endpoint.is_some() && std::env::var("LUMO_UPDATER_PUBKEY").is_ok(),
        endpoint,
    })
}

#[tauri::command]
pub(super) async fn desktop_update_check(
    app: AppHandle,
    channel: Option<String>,
) -> Result<DesktopUpdateInfo, String> {
    let channel = normalized_channel(channel)?;
    let current_version = app.package_info().version.to_string();
    let update = check_update(&app, &channel).await?;
    Ok(match update {
        Some(update) => DesktopUpdateInfo {
            available: true,
            current_version,
            version: Some(update.version),
            body: update.body,
            date: update.date.map(|date| date.to_string()),
            target: Some(update.target),
        },
        None => DesktopUpdateInfo {
            available: false,
            current_version,
            version: None,
            body: None,
            date: None,
            target: None,
        },
    })
}

#[tauri::command]
pub(super) async fn desktop_update_install(
    app: AppHandle,
    channel: Option<String>,
) -> Result<(), String> {
    let channel = normalized_channel(channel)?;
    let update = check_update(&app, &channel)
        .await?
        .ok_or_else(|| "no update is available".to_string())?;
    let progress_app = app.clone();
    let finish_app = app.clone();
    let mut downloaded = 0_u64;
    update.download_and_install(
        move |chunk, total| {
            downloaded = downloaded.saturating_add(chunk as u64);
            let _ = progress_app.emit("lumo://update-progress", serde_json::json!({ "downloaded": downloaded, "total": total, "phase": "downloading" }));
        },
        move || { let _ = finish_app.emit("lumo://update-progress", serde_json::json!({ "phase": "installing" })); },
    ).await.map_err(|error| error.to_string())?;
    let _ = app.emit(
        "lumo://update-progress",
        serde_json::json!({ "phase": "restart_required" }),
    );
    Ok(())
}

#[tauri::command]
pub(super) fn desktop_update_restart(app: AppHandle) {
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_update_channels_and_requires_https_endpoints() {
        assert_eq!(normalized_channel(Some("stable".into())).unwrap(), "stable");
        assert!(normalized_channel(Some("nightly".into())).is_err());
        std::env::set_var("LUMO_UPDATE_ENDPOINT", "http://unsafe.example/latest.json");
        assert!(endpoint_for("stable").is_none());
        std::env::remove_var("LUMO_UPDATE_ENDPOINT");
    }
}
