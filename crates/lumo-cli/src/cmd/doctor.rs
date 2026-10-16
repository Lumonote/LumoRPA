//! `lumo doctor` — 环境体检（批次 D）：一条命令回答「这台机器上哪些 flow 能力
//! 能跑」。检查 LUMO_HOME 可写性、Chromium 浏览器、OCR Python、providers 配置、
//! LUMO_ALLOW_* 环境门禁、vault 身份、步骤超时与已安装 skills。
//! 门禁双体系（capability YAML × 环境开关）在 validate 静态侧不可见 —— doctor
//! 是运行前看清目标机开关状态的入口。

use clap::Args as ClapArgs;
use std::path::{Path, PathBuf};

use super::vault_identity_path;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// 以 JSON 输出检查结果（供脚本消费）
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct DoctorCheck {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
}

fn check(name: &'static str, status: Status, detail: impl Into<String>) -> DoctorCheck {
    DoctorCheck {
        name,
        status,
        detail: detail.into(),
    }
}

/// `LUMO_STEP_TIMEOUT_MS` 分类：未设→默认 600s；`0`→警告（禁用超时）；
/// 非法→警告（宿主会回退默认值）。
pub(crate) fn classify_step_timeout(raw: Option<&str>) -> (Status, String) {
    match raw {
        None => (Status::Ok, "未设置，使用默认 600000 ms (600s)".into()),
        Some(value) => match value.trim().parse::<u64>() {
            Ok(0) => (
                Status::Warn,
                "LUMO_STEP_TIMEOUT_MS=0 —— 步骤超时被禁用，卡死的步骤会挂住宿主".into(),
            ),
            Ok(ms) => (Status::Ok, format!("LUMO_STEP_TIMEOUT_MS={ms} ms")),
            Err(_) => (
                Status::Warn,
                format!("`{value}` 无法解析为毫秒数 —— 宿主将回退默认 600000 ms"),
            ),
        },
    }
}

/// 在候选目录里找首个存在的可执行文件（`exists` 注入以便测试）。
/// 目录优先级高于候选名顺序（PATH 语义）。
pub(crate) fn find_executable(
    dirs: &[PathBuf],
    names: &[&str],
    exists: &dyn Fn(&Path) -> bool,
) -> Option<PathBuf> {
    for dir in dirs {
        for name in names {
            let candidate = dir.join(name);
            if exists(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

const BROWSER_PATH_CANDIDATES: &[&str] = &[
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "chrome",
    "msedge",
    "microsoft-edge",
    "chrome.exe",
    "msedge.exe",
];

#[cfg(target_os = "macos")]
const BROWSER_APP_CANDIDATES: &[&str] = &[
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
];
#[cfg(not(target_os = "macos"))]
const BROWSER_APP_CANDIDATES: &[&str] = &[];

fn detect_browser() -> Option<PathBuf> {
    let dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    let exists = |p: &Path| p.is_file();
    find_executable(&dirs, BROWSER_PATH_CANDIDATES, &exists).or_else(|| {
        BROWSER_APP_CANDIDATES
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
    })
}

fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name).map(|v| v == "1").unwrap_or(false)
}

pub(crate) fn run_checks(home: &Path) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();

    // 1. LUMO_HOME 可写性
    checks.push(
        match std::fs::create_dir_all(home).and_then(|()| {
            let probe = home.join(".doctor-probe");
            std::fs::write(&probe, b"ok")?;
            std::fs::remove_file(&probe)
        }) {
            Ok(()) => check("home", Status::Ok, format!("{} 可写", home.display())),
            Err(e) => check(
                "home",
                Status::Fail,
                format!("{} 不可写: {e}", home.display()),
            ),
        },
    );

    // 2. Chromium 系浏览器
    checks.push(match detect_browser() {
        Some(path) => check("browser", Status::Ok, path.display().to_string()),
        None => check(
            "browser",
            Status::Warn,
            "未在 PATH / 常见安装位置找到 Chrome/Chromium/Edge —— browser.* 动作将无法启动",
        ),
    });

    // 3. 本地 OCR Python
    let python = std::env::var("LUMO_OCR_PYTHON").unwrap_or_else(|_| "python3".into());
    checks.push(
        match std::process::Command::new(&python)
            .arg("--version")
            .output()
        {
            Ok(out) if out.status.success() => {
                let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let version = if version.is_empty() {
                    String::from_utf8_lossy(&out.stderr).trim().to_string()
                } else {
                    version
                };
                check("ocr-python", Status::Ok, format!("{python} ({version})"))
            }
            _ => check(
                "ocr-python",
                Status::Warn,
                format!(
                    "`{python}` 不可用 —— 本地 OCR（modelscope 预设）无法运行；云 OCR 不受影响"
                ),
            ),
        },
    );

    // 4. providers 配置
    let providers_path = crate::cmd::providers_path(home);
    checks.push(if !providers_path.exists() {
        check(
            "providers",
            Status::Warn,
            format!(
                "{} 不存在 —— AI 动作前先运行 `lumo providers init`",
                providers_path.display()
            ),
        )
    } else {
        match lumo_ai::ProvidersConfig::load(&providers_path) {
            Ok(cfg) => check(
                "providers",
                Status::Ok,
                format!(
                    "{} 个 provider，active={}",
                    cfg.profiles.len(),
                    cfg.active.as_deref().unwrap_or("(未设置)")
                ),
            ),
            Err(e) => check("providers", Status::Fail, format!("解析失败: {e}")),
        }
    });

    // 5. LLM 网络开关
    checks.push(if env_flag_enabled("LUMO_ALLOW_LLM_NETWORK") {
        check("llm-network", Status::Ok, "LUMO_ALLOW_LLM_NETWORK=1 已开启")
    } else {
        check(
            "llm-network",
            Status::Warn,
            "未开启 —— ai.chat / 云 OCR / AI hooks 需要 LUMO_ALLOW_LLM_NETWORK=1",
        )
    });

    // 6. 破坏性动作环境门禁（默认关闭是安全态，只做信息呈现）
    for (name, env, effect) in [
        ("gate-shell", "LUMO_ALLOW_SHELL", "system.shell"),
        (
            "gate-process",
            "LUMO_ALLOW_PROCESS",
            "system.process_kill / app_start",
        ),
    ] {
        checks.push(if env_flag_enabled(env) {
            check(
                name,
                Status::Ok,
                format!("{env}=1 已开启（{effect} 可执行）"),
            )
        } else {
            check(
                name,
                Status::Ok,
                format!("{env} 未开启（{effect} 将被拒绝）"),
            )
        });
    }

    // 7. vault 身份
    let vault_path = vault_identity_path(home);
    checks.push(if vault_path.exists() {
        check("vault", Status::Ok, vault_path.display().to_string())
    } else {
        check(
            "vault",
            Status::Warn,
            "未初始化 —— 使用 vault 占位符的 flow 会失败（`lumo vault init`）",
        )
    });

    // 8. 步骤超时
    let raw = std::env::var("LUMO_STEP_TIMEOUT_MS").ok();
    let (status, detail) = classify_step_timeout(raw.as_deref());
    checks.push(check("step-timeout", status, detail));

    // 9. skills 目录
    let skills_dir = home.join("skills");
    let count = std::fs::read_dir(&skills_dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|e| e.path().join("SKILL.md").is_file())
                .count()
        })
        .unwrap_or(0);
    checks.push(check(
        "skills",
        Status::Ok,
        format!("{count} 个已安装（{}）", skills_dir.display()),
    ));

    checks
}

pub async fn run(home: PathBuf, args: Args) -> anyhow::Result<()> {
    let checks = run_checks(&home);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&checks)?);
    } else {
        for c in &checks {
            let icon = match c.status {
                Status::Ok => "✅",
                Status::Warn => "⚠️ ",
                Status::Fail => "❌",
            };
            println!("{icon} {:<14} {}", c.name, c.detail);
        }
        let warns = checks.iter().filter(|c| c.status == Status::Warn).count();
        let fails = checks.iter().filter(|c| c.status == Status::Fail).count();
        println!("\n{} 项检查：{} 失败 / {} 提醒", checks.len(), fails, warns);
    }
    let fails = checks.iter().filter(|c| c.status == Status::Fail).count();
    if fails > 0 {
        anyhow::bail!("doctor 发现 {fails} 项失败");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_step_timeout_covers_all_branches() {
        let (s, d) = classify_step_timeout(None);
        assert_eq!(s, Status::Ok);
        assert!(d.contains("600"), "default mentioned: {d}");

        let (s, d) = classify_step_timeout(Some("30000"));
        assert_eq!(s, Status::Ok);
        assert!(d.contains("30000"), "{d}");

        let (s, _) = classify_step_timeout(Some("0"));
        assert_eq!(s, Status::Warn);

        let (s, d) = classify_step_timeout(Some("abc"));
        assert_eq!(s, Status::Warn);
        assert!(d.contains("600"), "fallback mentioned: {d}");
    }

    #[test]
    fn find_executable_scans_dirs_in_priority_order() {
        let dirs = vec![PathBuf::from("/a"), PathBuf::from("/b")];
        let exists = |p: &Path| p == Path::new("/b/chrome") || p == Path::new("/a/msedge");
        // /a 优先于 /b，即使 chrome 排在候选名单前面
        let found = find_executable(&dirs, &["chrome", "msedge"], &exists).unwrap();
        assert_eq!(found, PathBuf::from("/a/msedge"));
        assert!(find_executable(&[], &["chrome"], &exists).is_none());
        let none = |_: &Path| false;
        assert!(find_executable(&dirs, &["chrome"], &none).is_none());
    }

    #[test]
    fn run_checks_reports_unique_names_and_writable_home() {
        let home = std::env::temp_dir().join(format!("lumo-doctor-{}", std::process::id()));
        let checks = run_checks(&home);
        assert!(checks.len() >= 8);
        let mut names: Vec<_> = checks.iter().map(|c| c.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), checks.len(), "check names must be unique");
        let home_check = checks.iter().find(|c| c.name == "home").unwrap();
        assert_eq!(home_check.status, Status::Ok);
        let _ = std::fs::remove_dir_all(&home);
    }
}
