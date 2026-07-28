use super::*;

/// F-20: breakpoint-debug options threaded into a run. Default means a normal run.
#[derive(Default)]
pub(super) struct DebugOpts {
    pub(super) breakpoints: std::collections::HashSet<String>,
    pub(super) step_mode: bool,
    pub(super) resume_from: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_flow(
    home: &Path,
    flow_path: Option<&Path>,
    flow: Flow,
    inputs: Value,
    no_store: bool,
    debug: DebugOpts,
    cancels: &CancelMap,
    prompter: Option<Arc<dyn HumanPrompter>>,
) -> Result<RunResponse, String> {
    let registry = build_action_registry(home, flow_path);
    let repo = if no_store {
        None
    } else {
        Some(Repo::open(home.join("lumo.db")).map_err(|e| e.to_string())?)
    };
    let run_id = ulid::Ulid::new().to_string();
    let cancel = CancelToken::new();
    cancels
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(run_id.clone(), cancel.clone());
    let vm = FlowVm::new(registry, repo.clone())
        .with_run_id(Some(run_id.clone()))
        .with_cancel(cancel)
        .with_step_timeout(step_timeout())
        .with_artifacts_dir(Some(home.join("artifacts")))
        .with_on_step(Some(Arc::new(|event| {
            if let Some(app) = APP_HANDLE.get() {
                let _ = app.emit("lumo://run-progress", run_progress::payload(event));
            }
        })))
        .with_vault(load_vault_identity(home))
        .with_breakpoints(debug.breakpoints)
        .with_step_mode(debug.step_mode)
        .with_resume_from(debug.resume_from)
        .with_human_prompter(prompter);
    let ai = flow.metadata.ai.clone().unwrap_or_default();
    let ai_cfg = ProvidersConfig::load(providers_path(home)).unwrap_or_default();
    let vm = match lumo_ai::build_hook_provider(&ai_cfg, ai.enabled, ai.budget.max_calls_per_run) {
        Some(provider) => vm.with_ai_provider(provider),
        None => vm,
    };
    let result = vm
        .run(
            &flow,
            RunOptions {
                inputs,
                trigger_kind: "desktop".into(),
            },
        )
        .await;
    cancels
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&run_id);
    let report = result.map_err(|e| e.to_string())?;
    let run = repo
        .as_ref()
        .and_then(|r| r.get_run(&report.run_id).ok().flatten())
        .map(run_dto);
    let steps = repo
        .as_ref()
        .and_then(|r| r.list_steps(&report.run_id).ok())
        .unwrap_or_default()
        .into_iter()
        .map(step_dto)
        .collect();
    Ok(RunResponse {
        report: report_dto(report),
        run,
        steps,
    })
}

fn step_timeout() -> std::time::Duration {
    const DEFAULT_MS: u64 = 600_000;
    let ms = std::env::var("LUMO_STEP_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or(DEFAULT_MS);
    std::time::Duration::from_millis(ms)
}

fn load_vault_identity(home: &Path) -> Option<Arc<lumo_storage::VaultIdentity>> {
    let path = std::env::var_os("LUMO_VAULT_IDENTITY")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("age-identity.txt"));
    if !path.exists() {
        return None;
    }
    match lumo_storage::VaultIdentity::load(&path) {
        Ok(id) => Some(Arc::new(id)),
        Err(e) => {
            eprintln!("vault identity at {} unreadable: {e}", path.display());
            None
        }
    }
}

#[cfg(test)]
mod debug_flow_tests {
    //! F-20: integration coverage for the breakpoint debugger at the *desktop*
    //! layer — the `debug_flow` command's real work lives in `execute_flow`, so
    //! we drive that directly against a temp `LUMO_HOME`. This exercises the
    //! whole chain a webview hits: build the registry, run under `DebugOpts`,
    //! surface `paused_at`, persist per-step `vars_json` (F-19), and resume the
    //! paused run to advance — plus the serde `camelCase` DTO contract the
    //! frontend depends on (`pausedAt`, `varsJson`).
    use super::{execute_flow, DebugOpts};
    use crate::{CancelMap, RunResponse};
    use std::collections::HashSet;
    use std::path::Path;

    const FLOW: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: dbg-it }
spec:
  steps:
    - { id: one,   action: control.set_var, with: { name: x, value: "1" } }
    - { id: two,   action: control.set_var, with: { name: y, value: "2" } }
    - { id: three, action: control.set_var, with: { name: z, value: "3" } }
"#;

    fn bps(list: &[&str]) -> HashSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    async fn run_debug(home: &Path, debug: DebugOpts) -> RunResponse {
        let flow = lumo_dsl::parse_str(FLOW).expect("parse flow");
        let cancels = CancelMap::default();
        execute_flow(
            home,
            None,
            flow,
            serde_json::json!({}),
            false,
            debug,
            &cancels,
            None,
        )
        .await
        .expect("execute_flow ok")
    }

    #[tokio::test]
    async fn debug_flow_breakpoint_pause_persists_vars_then_resume_completes() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();

        // ── Run 1: breakpoint on `two` → `one` runs, pause before `two`. ──
        let r1 = run_debug(
            home,
            DebugOpts {
                breakpoints: bps(&["two"]),
                ..Default::default()
            },
        )
        .await;

        assert_eq!(
            r1.report.paused_at.as_deref(),
            Some("two"),
            "paused before `two`"
        );
        assert!(!r1.report.success, "a paused run is not a success");

        // `one` is persisted ok with its F-19 vars snapshot; `two` (the
        // breakpoint) never ran, so it is absent.
        let one = r1
            .steps
            .iter()
            .find(|s| s.path == "one")
            .expect("step `one` persisted");
        assert_eq!(one.state, "ok");
        assert!(
            !r1.steps.iter().any(|s| s.path == "two"),
            "the un-run breakpoint step `two` must not be persisted"
        );
        let vars = one
            .vars_json
            .as_ref()
            .expect("F-19: per-step vars snapshot present");
        assert!(
            vars.get("x").is_some(),
            "vars snapshot carries `x` after set_var, got: {vars}"
        );

        // serde contract the webview reads: camelCase, no snake_case leak.
        let report_json = serde_json::to_value(&r1.report).unwrap();
        assert_eq!(report_json["pausedAt"], serde_json::json!("two"));
        assert!(
            report_json.get("runId").is_some(),
            "report uses camelCase runId"
        );
        let one_json = serde_json::to_value(one).unwrap();
        assert!(
            one_json.get("varsJson").is_some(),
            "StepRunDto serializes vars_json as camelCase varsJson"
        );
        assert!(
            one_json.get("vars_json").is_none(),
            "no snake_case key should leak to the webview"
        );

        // ── Run 2: continue (resume + same breakpoint) → steps off `two`,
        //    runs `two` + `three`, completes. ──
        let r2 = run_debug(
            home,
            DebugOpts {
                breakpoints: bps(&["two"]),
                resume_from: Some(r1.report.run_id.clone()),
                ..Default::default()
            },
        )
        .await;

        assert!(
            r2.report.success,
            "continuing past the breakpoint completes the run"
        );
        assert_eq!(r2.report.paused_at, None, "no further pause");
    }

    #[tokio::test]
    async fn debug_flow_single_step_advances_one_step_per_resume() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();

        // Fresh single-step run pauses before the very first step.
        let r1 = run_debug(
            home,
            DebugOpts {
                step_mode: true,
                ..Default::default()
            },
        )
        .await;
        assert_eq!(r1.report.paused_at.as_deref(), Some("one"));
        assert!(!r1.report.success);

        // Each resume steps off the current step and pauses before the next.
        let r2 = run_debug(
            home,
            DebugOpts {
                step_mode: true,
                resume_from: Some(r1.report.run_id.clone()),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(r2.report.paused_at.as_deref(), Some("two"));

        let r3 = run_debug(
            home,
            DebugOpts {
                step_mode: true,
                resume_from: Some(r2.report.run_id.clone()),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(r3.report.paused_at.as_deref(), Some("three"));

        // Stepping off the last step finishes the run.
        let r4 = run_debug(
            home,
            DebugOpts {
                step_mode: true,
                resume_from: Some(r3.report.run_id.clone()),
                ..Default::default()
            },
        )
        .await;
        assert!(
            r4.report.success,
            "stepping off the last step completes the run"
        );
        assert_eq!(r4.report.paused_at, None);
    }
}

#[cfg(test)]
mod cancel_timeout_tests {
    //! P0-1 / P1-1 桌面接线：`execute_flow` 现在为每次运行注册 CancelToken
    //! （键 = 落库 run_id）并默认挂步级超时。仿照 `debug_flow_tests` 直接驱动
    //! 私有 `execute_flow` + temp `LUMO_HOME`，覆盖 webview 命中的完整链路：
    //! 注册 → 触发取消 / 超时 → run 落库状态 → 句柄表自清理 → 幂等语义。
    use super::{execute_flow, DebugOpts};
    use crate::{cancel_run_inner, CancelMap};
    use std::time::Duration;

    /// `LUMO_STEP_TIMEOUT_MS` 是进程级环境变量，cargo test 默认多线程并行，
    /// 两个用例必须串行持锁，否则超时覆盖会污染对方的取消/默认语义。锁要
    /// 跨 await 持有，故用 tokio 的异步 Mutex（std 版会触发 await_holding_lock）。
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// 单步长睡：取消 / 超时都要能打断一个仍在 await 的动作，而不是等它跑完。
    const SLEEP_FLOW: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: cancel-it }
spec:
  steps:
    - { id: slow, action: control.sleep, with: { ms: 30000 } }
"#;

    async fn spawn_sleep_flow(
        home: std::path::PathBuf,
        cancels: CancelMap,
    ) -> Result<super::RunResponse, String> {
        let flow = lumo_dsl::parse_str(SLEEP_FLOW).expect("parse flow");
        execute_flow(
            &home,
            None,
            flow,
            serde_json::json!({}),
            false,
            DebugOpts::default(),
            &cancels,
            None,
        )
        .await
    }

    #[tokio::test]
    async fn cancel_run_interrupts_inflight_step_and_clears_token_table() {
        let _env = ENV_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let cancels = CancelMap::default();

        let task = tokio::spawn(spawn_sleep_flow(home.clone(), cancels.clone()));

        // run_id 由宿主在 run 启动前注册进取消表 —— 轮询表拿到真实键，
        // 这正是前端 cancel 按钮可依赖的同一份数据源。
        let mut run_id = None;
        for _ in 0..250 {
            if let Some(id) = cancels
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .keys()
                .next()
                .cloned()
            {
                run_id = Some(id);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let run_id = run_id.expect("run registered its cancel token");

        assert!(
            !cancel_run_inner(&cancels, "no-such-run"),
            "不存在的 run 必须返回 ok=false 而非报错"
        );
        assert!(
            cancel_run_inner(&cancels, &run_id),
            "运行中的 run 取消应返回 ok=true"
        );
        assert!(
            cancel_run_inner(&cancels, &run_id),
            "运行结束前的重复取消保持幂等 ok=true"
        );

        let result = task.await.expect("task join");
        let err = match result {
            Ok(_) => panic!("被取消的 run 不应成功返回"),
            Err(e) => e,
        };
        assert!(err.contains("cancelled"), "错误应表明取消，got: {err}");

        // run 落库为 cancelled，句柄表已被 execute_flow 清理，再取消 → false。
        let repo = lumo_storage::Repo::open(home.join("lumo.db")).unwrap();
        let run = repo.get_run(&run_id).unwrap().expect("run row persisted");
        assert_eq!(run.state, "cancelled");
        assert!(
            cancels.lock().unwrap_or_else(|e| e.into_inner()).is_empty(),
            "运行结束后 token 表必须清空"
        );
        assert!(
            !cancel_run_inner(&cancels, &run_id),
            "已结束的 run 重复取消返回 ok=false"
        );
    }

    #[tokio::test]
    async fn step_timeout_env_override_marks_step_timeout() {
        let _env = ENV_LOCK.lock().await;
        std::env::set_var("LUMO_STEP_TIMEOUT_MS", "80");
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let cancels = CancelMap::default();

        let result = spawn_sleep_flow(home.clone(), cancels.clone()).await;
        // 先恢复环境再断言：断言失败也不能把覆盖值泄漏给后续用例。
        std::env::remove_var("LUMO_STEP_TIMEOUT_MS");

        let err = match result {
            Ok(_) => panic!("80ms 超时下 30s 的步骤不应成功"),
            Err(e) => e,
        };
        assert!(err.contains("timed out"), "错误应表明超时，got: {err}");

        let repo = lumo_storage::Repo::open(home.join("lumo.db")).unwrap();
        let run = repo
            .list_runs(10)
            .unwrap()
            .into_iter()
            .next()
            .expect("run row persisted");
        assert_eq!(run.state, "failed", "超时的 run 落库为 failed");
        let steps = repo.list_steps(&run.id).unwrap();
        assert!(
            steps.iter().any(|s| s.state == "timeout"),
            "超时步骤应标记 state=timeout，got: {:?}",
            steps.iter().map(|s| s.state.clone()).collect::<Vec<_>>()
        );
        assert!(
            cancels.lock().unwrap_or_else(|e| e.into_inner()).is_empty(),
            "超时失败的 run 也要清理 token 表"
        );
    }
}

#[cfg(test)]
mod vault_wiring_tests {
    //! P1-5：桌面宿主的 vault 接线 —— `execute_flow` 现在与 CLI 一样加载
    //! `$LUMO_HOME/age-identity.txt`，`{{vault.*}}` 模板在 Studio 运行里
    //! 从加密存储解析，而不再报 vault 不可用。仿照 `debug_flow_tests` 直接
    //! 驱动私有 `execute_flow` + temp `LUMO_HOME`。
    use super::{execute_flow, DebugOpts};
    use crate::CancelMap;
    use lumo_storage::{Repo, Vault, VaultIdentity};
    use std::collections::BTreeMap;

    const FLOW: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: vault-it }
spec:
  vault: [smtp]
  steps:
    - { id: read, action: control.set_var, with: { name: u, value: "{{ vault.smtp.user }}" } }
"#;

    #[tokio::test]
    async fn execute_flow_resolves_vault_templates_from_home_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();

        // 预置身份 + 密文：与 `lumo vault set` 落盘的产物同构 —— 身份文件在
        // `$LUMO_HOME/age-identity.txt`，密文在 `$LUMO_HOME/lumo.db`。
        let id = VaultIdentity::generate();
        id.save(&home.join("age-identity.txt")).unwrap();
        {
            let repo = Repo::open(home.join("lumo.db")).unwrap();
            let mut fields = BTreeMap::new();
            fields.insert("user".to_string(), "alice@example.com".to_string());
            Vault::new(&repo, &id).put("smtp", &fields).unwrap();
        }

        let flow = lumo_dsl::parse_str(FLOW).expect("parse flow");
        let cancels = CancelMap::default();
        let resp = execute_flow(
            home,
            None,
            flow,
            serde_json::json!({}),
            false,
            DebugOpts::default(),
            &cancels,
            None,
        )
        .await
        .expect("vault-templated flow must run on the desktop host");

        assert!(resp.report.success, "run must succeed");
        // set_var 回显解析后的值 —— 证明 {{vault.smtp.user}} 真被解密，而非
        // 仅仅没报错。
        assert_eq!(
            resp.report
                .outputs
                .as_ref()
                .and_then(|o| o.pointer("/read/result"))
                .and_then(serde_json::Value::as_str),
            Some("alice@example.com"),
            "outputs: {:?}",
            resp.report.outputs
        );
        let read = resp
            .steps
            .iter()
            .find(|s| s.step_id == "read")
            .expect("read step persisted");
        assert_eq!(read.state, "ok");
    }
}
