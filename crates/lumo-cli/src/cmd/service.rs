//! `lumo service` — 把 `lumo serve`（cron/file-watch/webhook 调度守护）安装成
//! 登录常驻服务（批次 G）：macOS launchd 用户代理 / Linux systemd 用户单元 /
//! Windows 计划任务。模板生成是纯函数（单测覆盖），install/uninstall/status
//! 只做文件落盘与系统命令调用。

use clap::Args as ClapArgs;
use std::path::{Path, PathBuf};

pub(crate) const SERVICE_LABEL: &str = "io.lumorpa.serve";

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    pub cmd: Sub,
}

#[derive(Debug, clap::Subcommand)]
pub enum Sub {
    /// 安装并启动登录常驻的调度守护（包装 `lumo serve`）
    Install {
        /// serve 监听地址
        #[arg(long, default_value = "127.0.0.1:8787")]
        bind: String,
        /// webhook 鉴权 token（会明文写入服务定义文件——注意文件权限）
        #[arg(long)]
        token: Option<String>,
    },
    /// 停止并移除调度守护
    Uninstall,
    /// 查看守护的安装与运行状态
    Status,
}

/// serve 守护的完整参数向量（含显式 `--home`，避免服务环境里 home 漂移）。
pub(crate) fn serve_args(home: &Path, bind: &str, token: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "--home".to_string(),
        home.display().to_string(),
        "serve".to_string(),
        "--bind".to_string(),
        bind.to_string(),
    ];
    if let Some(token) = token {
        args.push("--token".into());
        args.push(token.to_string());
    }
    args
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// macOS launchd 用户代理 plist。
pub(crate) fn launchd_plist(exe: &Path, args: &[String], log: &Path) -> String {
    let mut program_args = String::new();
    program_args.push_str(&format!(
        "    <string>{}</string>\n",
        xml_escape(&exe.display().to_string())
    ));
    for arg in args {
        program_args.push_str(&format!("    <string>{}</string>\n", xml_escape(arg)));
    }
    let log = xml_escape(&log.display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{SERVICE_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
{program_args}  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#
    )
}

/// Linux systemd 用户单元。
pub(crate) fn systemd_unit(exe: &Path, args: &[String]) -> String {
    let exec_start = format!("\"{}\" {}", exe.display(), args.join(" "));
    format!(
        r#"[Unit]
Description=LumoRPA scheduler daemon (lumo serve)
After=network-online.target

[Service]
ExecStart={exec_start}
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
"#
    )
}

/// Windows `schtasks /Create` 参数向量（不含 schtasks 本身）。
pub(crate) fn schtasks_create_args(exe: &Path, args: &[String]) -> Vec<String> {
    let command_line = format!("\"{}\" {}", exe.display(), args.join(" "));
    vec![
        "/Create".into(),
        "/F".into(),
        "/TN".into(),
        "LumoRPA-Serve".into(),
        "/SC".into(),
        "ONLOGON".into(),
        "/TR".into(),
        command_line,
    ]
}

fn user_home() -> anyhow::Result<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("无法确定用户主目录（HOME/USERPROFILE 均未设置）"))
}

fn run_quiet(cmd: &str, args: &[&str]) -> anyhow::Result<bool> {
    Ok(std::process::Command::new(cmd)
        .args(args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false))
}

pub async fn run(home: PathBuf, args: Args) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    match args.cmd {
        Sub::Install { bind, token } => install(&home, &exe, &bind, token.as_deref()),
        Sub::Uninstall => uninstall(),
        Sub::Status => status(),
    }
}

fn install(home: &Path, exe: &Path, bind: &str, token: Option<&str>) -> anyhow::Result<()> {
    let args = serve_args(home, bind, token);
    if cfg!(target_os = "macos") {
        let plist_path = user_home()?.join(format!("Library/LaunchAgents/{SERVICE_LABEL}.plist"));
        let log = home.join("logs").join("serve.log");
        std::fs::create_dir_all(home.join("logs"))?;
        std::fs::create_dir_all(plist_path.parent().unwrap())?;
        std::fs::write(&plist_path, launchd_plist(exe, &args, &log))?;
        let path_str = plist_path.display().to_string();
        let _ = run_quiet("launchctl", &["unload", "-w", &path_str]);
        if !run_quiet("launchctl", &["load", "-w", &path_str])? {
            anyhow::bail!("launchctl load 失败：{path_str}");
        }
        println!("✅ 已安装并启动 launchd 用户代理：{path_str}");
        println!("   日志: {}", log.display());
    } else if cfg!(target_os = "linux") {
        let unit_path = user_home()?.join(".config/systemd/user/lumo-serve.service");
        std::fs::create_dir_all(unit_path.parent().unwrap())?;
        std::fs::write(&unit_path, systemd_unit(exe, &args))?;
        let _ = run_quiet("systemctl", &["--user", "daemon-reload"]);
        if !run_quiet(
            "systemctl",
            &["--user", "enable", "--now", "lumo-serve.service"],
        )? {
            anyhow::bail!("systemctl enable --now 失败（无 systemd 用户会话？）");
        }
        println!("✅ 已安装并启动 systemd 用户单元：{}", unit_path.display());
        println!("   日志: journalctl --user -u lumo-serve.service -f");
    } else if cfg!(target_os = "windows") {
        let create = schtasks_create_args(exe, &args);
        let create_ref: Vec<&str> = create.iter().map(String::as_str).collect();
        if !run_quiet("schtasks", &create_ref)? {
            anyhow::bail!("schtasks /Create 失败");
        }
        println!("✅ 已注册计划任务 LumoRPA-Serve（登录时启动）");
    } else {
        anyhow::bail!("当前平台不支持 service 安装");
    }
    if token.is_some() {
        println!("⚠️  token 以明文写入服务定义，请确认该文件仅当前用户可读。");
    }
    Ok(())
}

fn uninstall() -> anyhow::Result<()> {
    if cfg!(target_os = "macos") {
        let plist_path = user_home()?.join(format!("Library/LaunchAgents/{SERVICE_LABEL}.plist"));
        let path_str = plist_path.display().to_string();
        let _ = run_quiet("launchctl", &["unload", "-w", &path_str]);
        match std::fs::remove_file(&plist_path) {
            Ok(()) => println!("✅ 已停止并移除 {path_str}"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                println!("ℹ️  未安装（{path_str} 不存在）")
            }
            Err(e) => return Err(e.into()),
        }
    } else if cfg!(target_os = "linux") {
        let _ = run_quiet(
            "systemctl",
            &["--user", "disable", "--now", "lumo-serve.service"],
        );
        let unit_path = user_home()?.join(".config/systemd/user/lumo-serve.service");
        let _ = std::fs::remove_file(&unit_path);
        let _ = run_quiet("systemctl", &["--user", "daemon-reload"]);
        println!("✅ 已停止并移除 lumo-serve.service");
    } else if cfg!(target_os = "windows") {
        if !run_quiet("schtasks", &["/Delete", "/TN", "LumoRPA-Serve", "/F"])? {
            anyhow::bail!("schtasks /Delete 失败（可能未安装）");
        }
        println!("✅ 已移除计划任务 LumoRPA-Serve");
    } else {
        anyhow::bail!("当前平台不支持 service 卸载");
    }
    Ok(())
}

fn status() -> anyhow::Result<()> {
    if cfg!(target_os = "macos") {
        let plist_path = user_home()?.join(format!("Library/LaunchAgents/{SERVICE_LABEL}.plist"));
        let installed = plist_path.exists();
        let running = run_quiet("launchctl", &["list", SERVICE_LABEL])?;
        println!(
            "安装: {} · 运行: {}",
            if installed {
                format!("✅ {}", plist_path.display())
            } else {
                "❌ 未安装".into()
            },
            if running {
                "✅ launchd 已加载"
            } else {
                "❌ 未加载"
            }
        );
    } else if cfg!(target_os = "linux") {
        let running = run_quiet(
            "systemctl",
            &["--user", "is-active", "--quiet", "lumo-serve.service"],
        )?;
        println!(
            "运行: {}",
            if running {
                "✅ active"
            } else {
                "❌ inactive/未安装"
            }
        );
    } else if cfg!(target_os = "windows") {
        let installed = run_quiet("schtasks", &["/Query", "/TN", "LumoRPA-Serve"])?;
        println!(
            "安装: {}",
            if installed {
                "✅ 已注册"
            } else {
                "❌ 未注册"
            }
        );
    } else {
        anyhow::bail!("当前平台不支持 service 状态查询");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serve_args_carry_home_bind_and_optional_token() {
        let args = serve_args(Path::new("/data/lumo"), "0.0.0.0:9000", Some("s3cr3t"));
        assert_eq!(
            args,
            vec![
                "--home",
                "/data/lumo",
                "serve",
                "--bind",
                "0.0.0.0:9000",
                "--token",
                "s3cr3t"
            ]
        );
        let no_token = serve_args(Path::new("/data/lumo"), "127.0.0.1:8787", None);
        assert!(!no_token.contains(&"--token".to_string()));
    }

    #[test]
    fn launchd_plist_lists_program_arguments_and_keepalive() {
        let plist = launchd_plist(
            Path::new("/usr/local/bin/lumo"),
            &serve_args(Path::new("/Users/u/.lumorpa"), "127.0.0.1:8787", None),
            Path::new("/Users/u/.lumorpa/logs/serve.log"),
        );
        assert!(
            plist.contains(&format!("<string>{SERVICE_LABEL}</string>")),
            "{plist}"
        );
        assert!(plist.contains("<string>/usr/local/bin/lumo</string>"));
        assert!(plist.contains("<string>serve</string>"));
        assert!(plist.contains("<string>--bind</string>"));
        assert!(plist.contains("<key>KeepAlive</key>"));
        assert!(plist.contains("<key>RunAtLoad</key>"));
        assert!(plist.contains("<string>/Users/u/.lumorpa/logs/serve.log</string>"));
    }

    #[test]
    fn launchd_plist_escapes_xml_sensitive_paths() {
        let plist = launchd_plist(
            Path::new("/opt/a&b/lumo"),
            &["serve".to_string()],
            Path::new("/tmp/log"),
        );
        assert!(plist.contains("/opt/a&amp;b/lumo"), "{plist}");
        assert!(!plist.contains("/opt/a&b/lumo"));
    }

    #[test]
    fn systemd_unit_has_execstart_restart_and_install_target() {
        let unit = systemd_unit(
            Path::new("/usr/bin/lumo"),
            &serve_args(Path::new("/home/u/.lumorpa"), "127.0.0.1:8787", Some("t")),
        );
        assert!(unit.contains("ExecStart=\"/usr/bin/lumo\" --home /home/u/.lumorpa serve --bind 127.0.0.1:8787 --token t"), "{unit}");
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("WantedBy=default.target"));
    }

    #[test]
    fn schtasks_args_register_onlogon_task() {
        let args = schtasks_create_args(
            Path::new(r"C:\lumo\lumo.exe"),
            &[
                "serve".to_string(),
                "--bind".to_string(),
                "127.0.0.1:8787".to_string(),
            ],
        );
        assert!(args.contains(&"/Create".to_string()));
        assert!(args.contains(&"LumoRPA-Serve".to_string()));
        assert!(args.contains(&"ONLOGON".to_string()));
        let tr = args
            .iter()
            .find(|a| a.contains("lumo.exe"))
            .expect("command line present");
        assert!(tr.contains("--bind 127.0.0.1:8787"), "{tr}");
    }
}
