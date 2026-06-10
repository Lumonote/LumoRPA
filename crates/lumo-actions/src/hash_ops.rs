//! Hash + encoding actions (`hash.*`, `util.*`).

use async_trait::async_trait;
use base64::Engine;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use md5::Digest as Md5Digest;
use once_cell::sync::Lazy;
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use schemars::JsonSchema;
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
    r.register(UrlEncodeAction);
    r.register(UrlDecodeAction);
    r.register(UuidAction);
}

fn text_schema() -> &'static Value {
    static S: Lazy<Value> = Lazy::new(crate::schema::derive::<TextIn>);
    &S
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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

// ─── util.url_encode / util.url_decode ──────────────────────────────────────

/// `encodeURIComponent` 语义的编码集:`NON_ALPHANUMERIC` 去掉 JS 不转义的
/// `- _ . ! ~ * ' ( )`(RFC 3986 unreserved + mark 子集),与浏览器行为逐字符
/// 对齐 —— 这是拼 query 参数值时最常用、也最不易踩坑的语义,故为默认。
const COMPONENT_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

/// `encodeURI` 语义:在 component 集之上再保留 URI 结构保留字
/// `; , / ? : @ & = + $ #`,用于编码**整条 URL** 而不破坏其结构。
const URI_SET: &AsciiSet = &COMPONENT_SET
    .remove(b';')
    .remove(b',')
    .remove(b'/')
    .remove(b'?')
    .remove(b':')
    .remove(b'@')
    .remove(b'&')
    .remove(b'=')
    .remove(b'+')
    .remove(b'$')
    .remove(b'#');

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UrlEncodeIn {
    text: String,
    /// 编码语义开关(默认 `true`):`true` = `encodeURIComponent` 语义,
    /// `/ ? & = #` 等结构字符全部转义,适合编码单个参数值;`false` =
    /// `encodeURI` 语义,保留 URL 结构保留字,适合编码整条 URL。两种语义下
    /// 空格都编码为 `%20`(不是表单语义的 `+`)。
    #[serde(default = "default_component")]
    component: bool,
}
fn default_component() -> bool {
    true
}

pub struct UrlEncodeAction;
#[async_trait]
impl Action for UrlEncodeAction {
    fn id(&self) -> &'static str {
        "util.url_encode"
    }
    fn summary(&self) -> &'static str {
        "Percent-encode text (component: true = encodeURIComponent, false = encodeURI)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<UrlEncodeIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let UrlEncodeIn { text, component } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("util.url_encode invalid: {e}")))?;
        let set = if component { COMPONENT_SET } else { URI_SET };
        Ok(ActionResult::from(Value::String(
            utf8_percent_encode(&text, set).to_string(),
        )))
    }
}

pub struct UrlDecodeAction;
#[async_trait]
impl Action for UrlDecodeAction {
    fn id(&self) -> &'static str {
        "util.url_decode"
    }
    fn summary(&self) -> &'static str {
        "Percent-decode text → UTF-8 (`+` is left as-is, not a space)"
    }
    fn schema(&self) -> &'static Value {
        text_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let TextIn { text } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("util.url_decode invalid: {e}")))?;
        // 纯 percent 解码:`+` 原样保留 —— 把 `+` 当空格是
        // application/x-www-form-urlencoded 的表单语义,不属于 URL 解码本身。
        let out = percent_decode_str(&text)
            .decode_utf8()
            .map_err(|e| StepError::msg(format!("util.url_decode: not UTF-8 after decode: {e}")))?;
        Ok(ActionResult::from(Value::String(out.into_owned())))
    }
}

pub struct UuidAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UuidIn {}
#[async_trait]
impl Action for UuidAction {
    fn id(&self) -> &'static str {
        "util.uuid"
    }
    fn summary(&self) -> &'static str {
        "Generate a random UUID v4"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<UuidIn>);
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
