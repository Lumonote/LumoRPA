fn main() {
    // Public update configuration is embedded in the installer. Launching a
    // packaged app from Finder/Explorer must not require CI environment vars.
    for name in [
        "LUMO_UPDATER_PUBKEY",
        "LUMO_UPDATE_ENDPOINT",
        "LUMO_UPDATE_BETA_ENDPOINT",
        "LUMO_UPDATE_ROLLBACK_ENDPOINT",
        "LUMO_UPDATE_CHANNEL",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
        let value = std::env::var(name).unwrap_or_default();
        assert!(
            !value.contains(['\r', '\n']),
            "{name} must be a single line"
        );
        println!(
            "cargo:rustc-env=LUMO_BUILTIN_{}={}",
            &name[5..],
            value.trim()
        );
    }
    tauri_build::build()
}
