//! Native-capture acceptance harness. It never injects input itself; an isolated
//! test desktop or a human performs actions during the requested time window.
use lumo_recorder::{desktop::DesktopRecorder, events_to_yaml_patch, Recorder};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let duration: u64 = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "5".into())
        .parse()?;
    anyhow::ensure!(
        (1..=300).contains(&duration),
        "duration must be 1..300 seconds"
    );
    let recorder = DesktopRecorder::new();
    recorder.start(None).await?;
    eprintln!("CAPTURE_READY");
    tokio::time::sleep(std::time::Duration::from_secs(duration)).await;
    let events = recorder.stop().await?;
    println!(
        "{}",
        serde_json::json!({"events": events, "yaml": events_to_yaml_patch(&events)})
    );
    Ok(())
}
