mod common;

use async_trait::async_trait;
use bytes::Bytes;
use lumo_core::{ai_hook::*, error::StepError, ActionRegistry};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[derive(Default)]
struct LocalProvider {
    local_calls: AtomicUsize,
    cloud_calls: AtomicUsize,
    fail: bool,
}

#[async_trait]
impl AiHookProvider for LocalProvider {
    async fn heal_selector(
        &self,
        _: &str,
        _: &str,
        _: Option<&str>,
        _: Option<&str>,
    ) -> Result<HealedSelector, StepError> {
        unreachable!()
    }
    async fn extract_visual(
        &self,
        _: Option<Bytes>,
        _: &str,
        _: Option<&str>,
        _: Option<&Value>,
        _: Option<&str>,
    ) -> Result<Value, StepError> {
        unreachable!()
    }
    async fn decide(&self, _: &Value, _: &str, _: Option<&str>) -> Result<Decision, StepError> {
        unreachable!()
    }
    async fn diagnose(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: Option<&str>,
    ) -> Result<String, StepError> {
        unreachable!()
    }
    async fn vision_locate(
        &self,
        _: Bytes,
        _: &str,
        _: &[SoMMark],
        _: Option<&str>,
    ) -> Result<LocatedTarget, StepError> {
        unreachable!()
    }
    async fn ocr_image(
        &self,
        _: Bytes,
        _: &str,
        _: &str,
        _: Option<&str>,
    ) -> Result<String, StepError> {
        self.cloud_calls.fetch_add(1, Ordering::SeqCst);
        Err(StepError::msg("cloud must not be called"))
    }
    fn local_ocr_model(&self, _: Option<&str>) -> Option<String> {
        Some("modelscope/ZhipuAI/GLM-OCR".into())
    }
    async fn ocr_image_local(
        &self,
        _: Bytes,
        _: &str,
        _: &str,
        model: Option<&str>,
    ) -> Result<String, StepError> {
        assert_eq!(model, Some("modelscope/ZhipuAI/GLM-OCR"));
        self.local_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            Err(StepError::msg("local dependency missing"))
        } else {
            Ok("本地合同金额\n100000".into())
        }
    }
}

async fn recognize(
    provider: Arc<LocalProvider>,
    options: Value,
    authorized: bool,
) -> Result<Value, StepError> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scan.png");
    std::fs::write(&path, b"image bytes").unwrap();
    let caps = if authorized {
        common::fs_caps(dir.path())
    } else {
        common::Capabilities::default()
    };
    let mut ctx = common::ctx_with(caps).with_ai(Some(provider), None);
    let mut registry = ActionRegistry::new();
    lumo_actions::register_all(&mut registry);
    registry
        .get("doc.ocr")
        .unwrap()
        .execute(&mut ctx, json!({"image": path, "ocr": options}))
        .await
        .map(|result| result.output)
}

#[tokio::test]
async fn local_document_ocr_succeeds_without_cloud_permission_or_network_capabilities() {
    let provider = Arc::new(LocalProvider::default());
    let output = recognize(provider.clone(), json!({"allow_cloud": false}), true)
        .await
        .unwrap();
    assert_eq!(output["text"], "本地合同金额\n100000");
    assert_eq!(output["left_machine"], false);
    assert_eq!(output["engine"], "ocrlocal");
    assert_eq!(output["source"], "ocr_local");
    assert_eq!(provider.local_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.cloud_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn failing_local_backend_never_falls_back_to_cloud_even_when_cloud_is_allowed() {
    let provider = Arc::new(LocalProvider {
        fail: true,
        ..Default::default()
    });
    let error = recognize(
        provider.clone(),
        json!({"allow_cloud": true, "endpoint": "http://127.0.0.1:9/ocr"}),
        true,
    )
    .await
    .unwrap_err();
    assert!(
        error.to_string().contains("local dependency missing"),
        "{error}"
    );
    assert_eq!(provider.local_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.cloud_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn local_backend_still_requires_read_authorization_before_reading_the_image() {
    let provider = Arc::new(LocalProvider::default());
    let error = recognize(provider.clone(), json!({"engine": "native"}), false)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("fs.read"), "{error}");
    assert_eq!(provider.local_calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.cloud_calls.load(Ordering::SeqCst), 0);
}
