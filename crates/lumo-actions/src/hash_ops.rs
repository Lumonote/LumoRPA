//! Hash + encoding actions (`hash.*`, `util.*`).

use async_trait::async_trait;
use base64::Engine;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use md5::Digest as Md5Digest;
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::Value;
use sha1::Sha1;
use sha2::{Sha256, Sha512};

pub fn register(r: &mut ActionRegistry) {
    r.register(Sha256Action);
    r.register(Sha512Action);
    r.register(Sha1Action);
    r.register(Md5Action);
    r.register(Base64EncodeAction);
    r.register(Base64DecodeAction);
    r.register(UuidAction);
}

fn text_schema() -> &'static Value {
    static S: Lazy<Value> = Lazy::new(|| {
        serde_json::json!({
            "type": "object",
            "required": ["text"],
            "properties": { "text": { "type": "string" } },
            "additionalProperties": false
        })
    });
    &S
}

#[derive(Deserialize)]
struct TextIn {
    text: String,
}

pub struct Sha256Action;
#[async_trait]
impl Action for Sha256Action {
    fn id(&self) -> &'static str {
        "hash.sha256"
    }
    fn summary(&self) -> &'static str {
        "SHA-256 hex digest of `text`"
    }
    fn schema(&self) -> &'static Value {
        text_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let TextIn { text } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("hash.sha256 invalid: {e}")))?;
        let mut h = <Sha256 as sha2::Digest>::new();
        sha2::Digest::update(&mut h, text.as_bytes());
        let bytes = sha2::Digest::finalize(h);
        Ok(ActionResult::from(Value::String(hex(&bytes))))
    }
}

pub struct Sha512Action;
#[async_trait]
impl Action for Sha512Action {
    fn id(&self) -> &'static str {
        "hash.sha512"
    }
    fn summary(&self) -> &'static str {
        "SHA-512 hex digest of `text`"
    }
    fn schema(&self) -> &'static Value {
        text_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let TextIn { text } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("hash.sha512 invalid: {e}")))?;
        let mut h = <Sha512 as sha2::Digest>::new();
        sha2::Digest::update(&mut h, text.as_bytes());
        let bytes = sha2::Digest::finalize(h);
        Ok(ActionResult::from(Value::String(hex(&bytes))))
    }
}

pub struct Sha1Action;
#[async_trait]
impl Action for Sha1Action {
    fn id(&self) -> &'static str {
        "hash.sha1"
    }
    fn summary(&self) -> &'static str {
        "SHA-1 hex digest of `text` (legacy)"
    }
    fn schema(&self) -> &'static Value {
        text_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let TextIn { text } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("hash.sha1 invalid: {e}")))?;
        let mut h = <Sha1 as sha1::Digest>::new();
        sha1::Digest::update(&mut h, text.as_bytes());
        let bytes = sha1::Digest::finalize(h);
        Ok(ActionResult::from(Value::String(hex(&bytes))))
    }
}

pub struct Md5Action;
#[async_trait]
impl Action for Md5Action {
    fn id(&self) -> &'static str {
        "hash.md5"
    }
    fn summary(&self) -> &'static str {
        "MD5 hex digest of `text` (legacy)"
    }
    fn schema(&self) -> &'static Value {
        text_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let TextIn { text } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("hash.md5 invalid: {e}")))?;
        let mut h = md5::Md5::new();
        h.update(text.as_bytes());
        let bytes = h.finalize();
        Ok(ActionResult::from(Value::String(hex(&bytes))))
    }
}

pub struct Base64EncodeAction;
#[async_trait]
impl Action for Base64EncodeAction {
    fn id(&self) -> &'static str {
        "util.base64_encode"
    }
    fn summary(&self) -> &'static str {
        "Base64 (standard) encode UTF-8 text"
    }
    fn schema(&self) -> &'static Value {
        text_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let TextIn { text } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("util.base64_encode invalid: {e}")))?;
        let out = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
        Ok(ActionResult::from(Value::String(out)))
    }
}

pub struct Base64DecodeAction;
#[async_trait]
impl Action for Base64DecodeAction {
    fn id(&self) -> &'static str {
        "util.base64_decode"
    }
    fn summary(&self) -> &'static str {
        "Base64 decode → UTF-8 text"
    }
    fn schema(&self) -> &'static Value {
        text_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let TextIn { text } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("util.base64_decode invalid: {e}")))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(text.as_bytes())
            .map_err(|e| StepError::msg(format!("base64 decode: {e}")))?;
        let out = String::from_utf8(bytes)
            .map_err(|e| StepError::msg(format!("base64 not UTF-8: {e}")))?;
        Ok(ActionResult::from(Value::String(out)))
    }
}

pub struct UuidAction;
#[async_trait]
impl Action for UuidAction {
    fn id(&self) -> &'static str {
        "util.uuid"
    }
    fn summary(&self) -> &'static str {
        "Generate a random UUID v4"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            })
        });
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        Ok(ActionResult::from(Value::String(
            uuid::Uuid::new_v4().to_string(),
        )))
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}
