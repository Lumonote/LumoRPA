//! 架构 P1-1 契约测试：CLI 四条宿主线（`lumo run` / `lumo serve` 的
//! webhook·cron·file 触发 / `lumo mcp` / hotkey 派发）统一经
//! `lumo_cli::cmd::host_vm` 组装 VM。
//!
//! `FlowVm` 没有公开 getter（字段私有，且本任务不改 lumo-core/vm.rs），按
//! 任务约定退而求其次 —— 给 helper 写**可观察行为**测试：
//!   * 快照探针：`StepCtx` 的公开 getter（`step_timeout` / `cancel_token` /
//!     `vault_identity` / `artifacts_dir` / `human_prompter`）暴露的就是引擎
//!     运行时真正消费的值，用一个自定义探针动作在 run 内部拍快照，等价于
//!     读到了 VM 的装配结果；
//!   * 行为验证：预先取消的令牌 ⇒ `ExecError::Cancelled`；
//!     `LUMO_STEP_TIMEOUT_MS` 覆盖 ⇒ 长睡步骤以 `ExecError::Timeout` 快速
//!     失败；age 身份 + 加密存储 ⇒ `${{ vault.* }}` 解出真值；探针归档 ⇒
//!     blob 落 `$LUMO_HOME/artifacts/{run_id}/`。
//!
//! 参数化：所有用例对 [`HOSTS`] 循环。四宿主收敛到 `host_vm` 后，装配差异
//! 只剩「有无 repo」（`run --no-store` 是唯一无 repo 形态）；宿主名单保留在
//! 断言消息里，未来任何宿主脱离 helper 单独装配时，这里就是要先改的清单。

use lumo_cli::cmd::{host_vm, step_timeout_from_ms};
use lumo_core::{
    Action, ActionRegistry, ActionResult, CancelToken, ExecError, RunOptions, StepCtx, StepError,
};
use lumo_storage::Repo;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

/// 宿主形态：名称 × 是否带 repo。`run --no-store` 是唯一 repo=None 的宿主线；
/// serve 的三条触发 lane、mcp、hotkey 全部固定开 repo。
const HOSTS: &[(&str, bool)] = &[
    ("run", true),
    ("run --no-store", false),
    ("serve.webhook", true),
    ("serve.cron", true),
    ("serve.file", true),
    ("mcp", true),
    ("hotkey", true),
];

/// `LUMO_STEP_TIMEOUT_MS` 是进程级环境变量，cargo test 默认并行；凡是读默认
/// 值或做覆盖的用例都必须串行持锁（跨 await 持有，用 tokio 异步 Mutex 避免
/// clippy 的 await_holding_lock）。
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ─── 探针动作:在 run 内部拍 StepCtx 装配快照 ────────────────────────────────

#[derive(Debug, Clone, Default)]
struct Wiring {
    step_timeout: Option<Duration>,
    has_cancel: bool,
    has_vault: bool,
    has_prompter: bool,
    artifacts_dir: Option<PathBuf>,
    /// `attach_artifact` 的返回：接了 artifacts_dir ⇒ 非空 ULID；no-op ⇒ 空串。
    artifact_id: String,
}

struct WiringProbe(Arc<Mutex<Option<Wiring>>>);

#[async_trait::async_trait]
impl Action for WiringProbe {
    fn id(&self) -> &'static str {
        "test.wiring_probe"
    }
    fn summary(&self) -> &'static str {
        "契约测试探针:快照 StepCtx 运行时装配并归档一个文本 artifact"
    }
    async fn execute(&self, ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        let artifact_id = ctx.attach_artifact("probe", "text/plain", b"host-contract-probe")?;
        *self.0.lock() = Some(Wiring {
            step_timeout: ctx.step_timeout(),
            has_cancel: ctx.cancel_token().is_some(),
            has_vault: ctx.vault_identity().is_some(),
            has_prompter: ctx.human_prompter().is_some(),
            artifacts_dir: ctx.artifacts_dir().cloned(),
            artifact_id: artifact_id.clone(),
        });
        Ok(ActionResult::from(
            serde_json::json!({ "artifact_id": artifact_id }),
        ))
    }
}

// ─── 公共脚手架 ──────────────────────────────────────────────────────────────

fn parse(yaml: &str) -> lumo_dsl::Flow {
    let flow = lumo_dsl::parse_str(yaml.trim_start()).expect("flow yaml parses");
    lumo_dsl::validate(&flow).expect("flow validates");
    flow
}

fn opts() -> RunOptions {
    RunOptions {
        inputs: Value::Object(serde_json::Map::new()),
        trigger_kind: "contract-test".into(),
    }
}

/// 建一个带 age 身份的 `$LUMO_HOME`（host_vm 的 vault 接线要能捞起它）。
fn home_with_identity() -> (TempDir, lumo_storage::VaultIdentity) {
    let home = TempDir::new().unwrap();
    let identity = lumo_storage::VaultIdentity::generate();
    identity
        .save(&home.path().join("age-identity.txt"))
        .expect("save identity");
    (home, identity)
}

fn open_repo(home: &TempDir) -> Repo {
    Repo::open(home.path().join("lumo.db")).expect("open repo")
}

fn probe_registry(cell: Arc<Mutex<Option<Wiring>>>) -> ActionRegistry {
    let mut registry = ActionRegistry::new();
    lumo_actions::register_all(&mut registry);
    registry.register(WiringProbe(cell));
    registry
}

fn sleep_registry() -> ActionRegistry {
    let mut registry = ActionRegistry::new();
    lumo_actions::register_all(&mut registry);
    registry
}

const PROBE_FLOW: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: wiring-probe }
spec:
  steps:
    - { id: probe, action: test.wiring_probe, with: {} }
"#;

const SLEEP_FLOW: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: sleepy }
spec:
  steps:
    - { id: nap, action: control.sleep, with: { ms: 60000 } }
"#;

const VAULT_FLOW: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: vault-echo }
spec:
  vault: [db]
  steps:
    - { id: echo, action: control.log, with: { message: "pw=${{ vault.db.password }}" } }
"#;

// ─── 契约 1:装配快照 —— 四件套 + 无 prompter + artifacts 落盘 ───────────────

/// 每条宿主线经 `host_vm` 组装出的 run，其 `StepCtx` 必须带齐:
/// step_timeout（默认 600s，与桌面端对齐）/ cancel / vault / artifacts_dir
/// （repo 存在时 = `$LUMO_HOME/artifacts`，`--no-store` 保持 no-op）；且
/// **不**注入 human prompter（headless 语义，`lumo run` 在调用方链上另加）。
#[tokio::test]
async fn all_hosts_wire_timeout_artifacts_cancel_vault() {
    let _env = ENV_LOCK.lock().await;
    std::env::remove_var("LUMO_STEP_TIMEOUT_MS"); // 断言默认值,先清干扰

    for (host, with_repo) in HOSTS {
        let (home, _identity) = home_with_identity();
        let repo = with_repo.then(|| open_repo(&home));
        let cell: Arc<Mutex<Option<Wiring>>> = Arc::new(Mutex::new(None));
        let flow = parse(PROBE_FLOW);

        let vm = host_vm(
            home.path(),
            &flow,
            probe_registry(cell.clone()),
            repo,
            CancelToken::new(),
        );
        let report = vm.run(&flow, opts()).await.unwrap_or_else(|e| {
            panic!("host {host}: probe run failed: {e}");
        });
        assert!(report.success, "host {host}: probe run must succeed");

        let w = cell.lock().take().unwrap_or_else(|| {
            panic!("host {host}: probe never executed");
        });
        assert_eq!(
            w.step_timeout,
            Some(Duration::from_secs(600)),
            "host {host}: 默认步级超时 600s(与桌面端 LUMO_STEP_TIMEOUT_MS 语义对齐)"
        );
        assert!(w.has_cancel, "host {host}: cancel 令牌必须接入");
        assert!(w.has_vault, "host {host}: age 身份必须被捞起");
        assert!(
            !w.has_prompter,
            "host {host}: host_vm 不注入 prompter(headless 宿主让 human.* 显式报错)"
        );

        let artifacts_root = home.path().join("artifacts");
        if *with_repo {
            assert_eq!(
                w.artifacts_dir.as_deref(),
                Some(artifacts_root.as_path()),
                "host {host}: artifacts_dir 固定 $LUMO_HOME/artifacts"
            );
            assert!(
                !w.artifact_id.is_empty(),
                "host {host}: attach_artifact 应真正归档"
            );
            let run_dir = artifacts_root.join(&report.run_id);
            let blobs = std::fs::read_dir(&run_dir)
                .unwrap_or_else(|e| panic!("host {host}: read {}: {e}", run_dir.display()))
                .count();
            assert_eq!(
                blobs, 1,
                "host {host}: 探针 blob 应落 artifacts/{{run_id}}/"
            );
        } else {
            assert!(
                w.artifacts_dir.is_none(),
                "host {host}: 无 repo(--no-store)时 artifacts 保持 no-op"
            );
            assert!(w.artifact_id.is_empty(), "host {host}: no-op 归档返回空 id");
            assert!(
                !artifacts_root.exists(),
                "host {host}: 不应产生孤儿 artifacts 目录"
            );
        }
    }
}

// ─── 契约 2:cancel 行为 —— 调用方令牌真正管住 run ───────────────────────────

/// 调用方持有的令牌预先取消 ⇒ 每条宿主线的 run 立刻以 `Cancelled` 收场,
/// 证明 host_vm 注入的是**同源**令牌而非摆设。
#[tokio::test]
async fn pre_cancelled_token_cancels_run_on_every_host() {
    for (host, with_repo) in HOSTS {
        let (home, _identity) = home_with_identity();
        let repo = with_repo.then(|| open_repo(&home));
        let flow = parse(SLEEP_FLOW);
        let cancel = CancelToken::new();
        cancel.cancel(); // 预先取消:run 必须在第一步前判死

        let vm = host_vm(home.path(), &flow, sleep_registry(), repo, cancel);
        let err = tokio::time::timeout(Duration::from_secs(10), vm.run(&flow, opts()))
            .await
            .unwrap_or_else(|_| panic!("host {host}: cancelled run must return promptly"))
            .expect_err("pre-cancelled run must fail");
        assert!(
            matches!(err, ExecError::Cancelled),
            "host {host}: expected Cancelled, got {err}"
        );
    }
}

// ─── 契约 3:step_timeout 行为 —— 环境变量覆盖真正生效 ───────────────────────

/// `LUMO_STEP_TIMEOUT_MS=250` 下,60s 长睡步骤必须以 `ExecError::Timeout`
/// 快速失败 —— 证明解析上移后每条宿主线都吃到了这个值。
#[tokio::test]
async fn step_timeout_env_override_times_out_stuck_step_on_every_host() {
    let _env = ENV_LOCK.lock().await;
    std::env::set_var("LUMO_STEP_TIMEOUT_MS", "250");

    for (host, with_repo) in HOSTS {
        let (home, _identity) = home_with_identity();
        let repo = with_repo.then(|| open_repo(&home));
        let flow = parse(SLEEP_FLOW);

        let vm = host_vm(
            home.path(),
            &flow,
            sleep_registry(),
            repo,
            CancelToken::new(),
        );
        // 外层 10s 兜底:接线断了的话步骤会睡满 60s,用超时把失败前置且可读。
        let err = tokio::time::timeout(Duration::from_secs(10), vm.run(&flow, opts()))
            .await
            .unwrap_or_else(|_| {
                std::env::remove_var("LUMO_STEP_TIMEOUT_MS");
                panic!("host {host}: step timeout did not fire (wiring broken?)");
            })
            .expect_err("stuck step must time out");
        assert!(
            matches!(err, ExecError::Timeout { .. }),
            "host {host}: expected Timeout, got {err}"
        );
    }

    std::env::remove_var("LUMO_STEP_TIMEOUT_MS");
}

// ─── 契约 4:vault 行为 —— 加密存储字段可被 ${{ vault.* }} 解出 ──────────────

/// `$LUMO_HOME/age-identity.txt` + 加密存储的 `db.password`,经每条带 repo
/// 的宿主线运行后,`${{ vault.db.password }}` 渲染出真值(落进步骤输出)。
/// `run --no-store` 无 repo,加密存储天然不可达(env 覆盖路径与 helper 无关),
/// 不在本契约内。
#[tokio::test]
async fn vault_store_field_resolves_on_every_repo_host() {
    const SECRET: &str = "s3cret-host-contract";
    for (host, with_repo) in HOSTS {
        if !with_repo {
            continue;
        }
        let (home, identity) = home_with_identity();
        let repo = open_repo(&home);
        let mut fields = BTreeMap::new();
        fields.insert("password".to_string(), SECRET.to_string());
        lumo_storage::vault::Vault::new(&repo, &identity)
            .put("db", &fields)
            .expect("seed vault");

        let flow = parse(VAULT_FLOW);
        let vm = host_vm(
            home.path(),
            &flow,
            sleep_registry(),
            Some(repo.clone()),
            CancelToken::new(),
        );
        let report = vm
            .run(&flow, opts())
            .await
            .unwrap_or_else(|e| panic!("host {host}: vault run failed: {e}"));
        assert!(report.success, "host {host}");

        let steps = repo.list_steps(&report.run_id).expect("list steps");
        let echoed = steps
            .iter()
            .filter_map(|s| s.output_json.as_ref())
            .any(|o| o.to_string().contains(SECRET));
        assert!(
            echoed,
            "host {host}: ${{{{ vault.db.password }}}} 应从加密存储解出真值"
        );
    }
}

// ─── 契约 5:LUMO_STEP_TIMEOUT_MS 解析规则(纯函数,无 env 竞争) ─────────────

#[test]
fn step_timeout_parsing_matches_desktop_semantics() {
    let default = Duration::from_secs(600);
    assert_eq!(step_timeout_from_ms(None), default, "未设置 ⇒ 默认 600s");
    assert_eq!(
        step_timeout_from_ms(Some("1500")),
        Duration::from_millis(1500),
        "合法毫秒数生效"
    );
    assert_eq!(
        step_timeout_from_ms(Some("0")),
        default,
        "0 会让所有步骤瞬间超时,按配置错误回退默认"
    );
    assert_eq!(
        step_timeout_from_ms(Some("not-a-number")),
        default,
        "解析失败回退默认"
    );
    assert_eq!(
        step_timeout_from_ms(Some("-5")),
        default,
        "负数解析失败回退默认"
    );
}
