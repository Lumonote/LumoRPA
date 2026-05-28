# LumoRPA Desktop

LumoRPA Desktop is a Tauri-based desktop workbench that calls the Rust runtime directly.

## Local Build

```bash
cd apps/desktop
cargo tauri build --bundles app,dmg
```

macOS outputs are written under `src-tauri/target/release/bundle/` when built from this directory, or the workspace `target/release/bundle/` when Cargo uses the workspace target directory.

## Windows

Build on a Windows runner with MSVC, WebView2 and the NSIS/MSI toolchain installed:

```powershell
cd apps\desktop
cargo tauri build --target x86_64-pc-windows-msvc --bundles nsis,msi
```

Expected outputs:

- `*.exe` NSIS installer
- `*.msi` MSI installer

## Linux

Build on the target Linux distribution:

```bash
cd apps/desktop
cargo tauri build --target x86_64-unknown-linux-gnu --bundles deb,rpm,appimage
```

Expected outputs:

- `*.deb`
- `*.rpm`
- `*.AppImage`

## Kylin / Xinchuang Linux

For Kylin and other Xinchuang Linux distributions, build on the matching CPU and OS image whenever possible:

```bash
cd apps/desktop
cargo tauri build --target x86_64-unknown-linux-gnu --bundles deb,rpm,appimage
cargo tauri build --target aarch64-unknown-linux-gnu --bundles deb,rpm,appimage
cargo tauri build --target loongarch64-unknown-linux-gnu --bundles deb,rpm
```

AppImage support depends on the target distribution and CPU architecture. For loongarch64, prefer native `.deb` or `.rpm` builds on a loongarch64 Kylin builder.

