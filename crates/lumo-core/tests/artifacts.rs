//! X-07(时光回溯)生产者侧契约 —— 不依赖浏览器/显示器:
//!
//! 1. 动作在执行中调用 `StepCtx::attach_artifact` 后,blob 必须落在
//!    `{artifacts_dir}/{run_id}/{ulid}.{ext}`、`artifacts` 表插入对应行,且
//!    kind/mime/size/sha256/步骤归属与归档内容逐项一致(桌面端 `list_artifacts`
//!    / `read_artifact_blob` 与回放 scrubber 消费的正是这两份数据)。
//! 2. 宿主未接 artifacts_dir(如 CLI `--no-store`)时,attach 必须是无害 no-op:
//!    返回空 id、不落盘、不插行,动作本身照常成功。
//!
//! 用测试动作驱动真实 VM 路径(render → dispatch → persist),与
//! `persist_async.rs` 的 Echo 模式同构。

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, FlowVm, RunOptions, StepCtx};
use lumo_dsl::parse_str;
use lumo_storage::Repo;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

/// 归档用的固定字节(带 PNG magic,内容任意 —— attach 不校验图像合法性)。
const PNG_STUB: &[u8] = b"\x89PNG\r\n\x1a\nlumo-artifact-test-bytes";

/// 测试动作:把固定字节归档为 screenshot artifact,并把返回的 id 回显进输出
/// (no-op 路径下应为空串)。
struct Snap;

#[async_trait]
impl Action for Snap {
    fn id(&self) -> &'static str {
        "test.snap"
    }
    fn summary(&self) -> &'static str {
        "attaches a stub screenshot artifact"
    }
    fn schema(&self) -> &'static Value {
        static S: OnceLock<Value> = OnceLock::new();
        S.get_or_init(|| serde_json::json!({ "type": "object" }))
    }
    async fn execute(&self, ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        let id = ctx.attach_artifact("screenshot", "image/png", PNG_STUB)?;
        Ok(ActionResult::from(
            serde_json::json!({ "artifact_id": id }),
        ))
    }
}

const FLOW: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: artifacts-producer-test }
spec:
  steps:
    - { id: snap, action: test.snap, with: {} }
"#;

fn vm_with(repo: &Repo, artifacts_dir: Option<std::path::PathBuf>) -> FlowVm {
    let mut reg = ActionRegistry::new();
    reg.register(Snap);
    FlowVm::new(reg, Some(repo.clone())).with_artifacts_dir(artifacts_dir)
}

#[tokio::test]
async fn attach_artifact_persists_row_and_readable_blob() {
    let tmp = tempfile::tempdir().unwrap();
    let artifacts_dir = tmp.path().join("artifacts");
    let repo = Repo::open_in_memory().unwrap();
    let flow = parse_str(FLOW).unwrap();

    let report = vm_with(&repo, Some(artifacts_dir.clone()))
        .run(&flow, RunOptions::default())
        .await
        .expect("run ok");
    assert!(report.success);

    // artifacts 表恰好一行,元数据与归档调用一致,且归属到执行它的步骤。
    let rows = repo.list_artifacts(&report.run_id).expect("list artifacts");
    assert_eq!(rows.len(), 1, "exactly one artifact row, got {rows:?}");
    let row = &rows[0];
    assert!(!row.id.is_empty(), "artifact id is a real ULID");
    assert_eq!(row.flow_run_id, report.run_id);
    assert_eq!(row.step_id.as_deref(), Some("snap"), "step attribution");
    assert_eq!(row.kind, "screenshot");
    assert_eq!(row.mime, "image/png");
    assert_eq!(row.size, PNG_STUB.len() as i64);
    assert_eq!(
        row.sha256,
        Sha256::digest(PNG_STUB).to_vec(),
        "sha256 matches the archived bytes"
    );

    // blob 落在 {artifacts_dir}/{run_id}/ 下、按 mime 推扩展名,且逐字节可读回。
    let expected_dir = artifacts_dir.join(&report.run_id);
    let blob_path = std::path::Path::new(&row.blob_path);
    assert_eq!(blob_path.parent(), Some(expected_dir.as_path()));
    assert_eq!(blob_path.extension().and_then(|e| e.to_str()), Some("png"));
    let blob = std::fs::read(blob_path).expect("blob readable");
    assert_eq!(blob, PNG_STUB);
}

#[tokio::test]
async fn attach_artifact_without_artifacts_dir_is_a_harmless_noop() {
    // 不设 artifacts_dir(CLI --no-store 等宿主):动作照常成功、表无行,
    // 且 attach 返回空 id(经步骤输出回显断言)。
    let repo = Repo::open_in_memory().unwrap();
    let flow = parse_str(FLOW).unwrap();

    let report = vm_with(&repo, None)
        .run(&flow, RunOptions::default())
        .await
        .expect("run ok");
    assert!(report.success, "no-op attach must not fail the action");

    let rows = repo.list_artifacts(&report.run_id).expect("list artifacts");
    assert!(rows.is_empty(), "no artifact rows expected, got {rows:?}");

    // 步骤输出里回显的 id 为空串 —— 即 attach_artifact 的 no-op 返回值契约。
    let steps = repo.list_steps(&report.run_id).expect("list steps");
    let snap = steps.iter().find(|s| s.step_id == "snap").expect("snap row");
    let output = snap.output_json.as_ref().expect("snap output persisted");
    assert_eq!(
        output.get("artifact_id").and_then(Value::as_str),
        Some(""),
        "no-op attach returns an empty id, got: {output}"
    );
}
