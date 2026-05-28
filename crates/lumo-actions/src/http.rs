//! HTTP request action.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

pub fn register(r: &mut ActionRegistry) {
    r.register(RequestAction);
}

pub struct RequestAction;

#[derive(Deserialize)]
struct ReqIn {
    #[serde(default = "default_method")]
    method: String,
    url: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    query: HashMap<String, String>,
    #[serde(default)]
    body: Option<Value>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}
fn default_method() -> String {
    "GET".into()
}
fn default_timeout_ms() -> u64 {
    30_000
}

#[async_trait]
impl Action for RequestAction {
    fn id(&self) -> &'static str {
        "http.request"
    }
    fn summary(&self) -> &'static str {
        "Make an HTTP request and return status/body/headers"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["url"],
                "properties": {
                    "method": { "type": "string" },
                    "url": { "type": "string" },
                    "headers": { "type": "object" },
                    "query": { "type": "object" },
                    "body": {},
                    "timeout_ms": { "type": "integer" }
                },
                "additionalProperties": false
            })
        });
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ReqIn {
            method,
            url,
            headers,
            query,
            body,
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("http.request input invalid: {e}")))?;
        ctx.ensure_network_url(&url)?;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(timeout_ms))
            .build()
            .map_err(|e| StepError::msg(format!("http client: {e}")))?;

        let mut req = client
            .request(
                method
                    .parse()
                    .map_err(|e| StepError::msg(format!("bad method: {e}")))?,
                &url,
            )
            .query(&query);

        for (k, v) in &headers {
            req = req.header(k, v);
        }

        if let Some(body) = body {
            req = match body {
                Value::String(s) => req.body(s),
                other => req.json(&other),
            };
        }

        let resp = req
            .send()
            .await
            .map_err(|e| StepError::msg(format!("http send: {e}")))?;
        let status = resp.status().as_u16();
        let resp_headers: HashMap<_, _> = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let text = resp
            .text()
            .await
            .map_err(|e| StepError::msg(format!("http body: {e}")))?;
        let body_json: Option<Value> = serde_json::from_str(&text).ok();

        Ok(ActionResult::from(serde_json::json!({
            "status": status,
            "headers": resp_headers,
            "text": text,
            "json": body_json,
        })))
    }
}
