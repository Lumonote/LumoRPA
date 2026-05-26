//! Per-step execution context.

use crate::action::ActionRef;
use crate::registry::ActionRegistry;
use lumo_dsl::{Step, TemplateCtx};
use lumo_storage::Repo;
use parking_lot::Mutex;
use serde_json::{Map, Value};
use std::sync::Arc;

#[derive(Clone)]
pub struct StepCtx {
    pub run_id: String,
    pub flow_id: String,
    pub registry: ActionRegistry,
    repo: Option<Repo>,
    inner: Arc<Mutex<CtxInner>>,
}

struct CtxInner {
    inputs: Value,
    steps: Map<String, Value>,
    vars: Map<String, Value>,
    bindings: Map<String, Value>,
    log_buffer: Vec<String>,
}

impl StepCtx {
    pub fn new(
        run_id: String,
        flow_id: String,
        registry: ActionRegistry,
        repo: Option<Repo>,
        inputs: Value,
    ) -> Self {
        Self {
            run_id,
            flow_id,
            registry,
            repo,
            inner: Arc::new(Mutex::new(CtxInner {
                inputs,
                steps: Map::new(),
                vars: Map::new(),
                bindings: Map::new(),
                log_buffer: Vec::new(),
            })),
        }
    }

    pub fn template_ctx(&self) -> TemplateCtx {
        let g = self.inner.lock();
        TemplateCtx {
            inputs:   g.inputs.clone(),
            steps:    Value::Object(g.steps.clone()),
            vars:     Value::Object(g.vars.clone()),
            bindings: Value::Object(g.bindings.clone()),
            env:      env_snapshot(),
            vault:    Vec::new(),
        }
    }

    pub fn record_step_output(&self, step_id: &str, output: &Value) {
        let mut g = self.inner.lock();
        g.steps.insert(step_id.to_string(), serde_json::json!({ "result": output }));
    }

    pub fn set_var(&self, key: &str, value: Value) {
        self.inner.lock().vars.insert(key.to_string(), value);
    }

    pub fn vars_snapshot(&self) -> Value {
        Value::Object(self.inner.lock().vars.clone())
    }

    pub fn outputs_snapshot(&self) -> Value {
        Value::Object(self.inner.lock().steps.clone())
    }

    pub fn push_binding(&self, key: &str, value: Value) {
        self.inner.lock().bindings.insert(key.into(), value);
    }

    pub fn clear_binding(&self, key: &str) {
        self.inner.lock().bindings.remove(key);
    }

    pub fn log(&self, line: impl Into<String>) {
        let line = line.into();
        tracing::info!(target: "lumo.flow", "{}", line);
        self.inner.lock().log_buffer.push(line);
    }

    pub fn lookup_action(&self, id: &str) -> Option<ActionRef> {
        self.registry.get(id)
    }

    pub fn run_id(&self) -> &str { &self.run_id }
    pub fn flow_id(&self) -> &str { &self.flow_id }
    pub fn repo(&self) -> Option<&Repo> { self.repo.as_ref() }

    pub async fn run_block(
        &mut self,
        steps: &[Step],
    ) -> Result<(), crate::ExecError> {
        crate::vm::run_block_inline(self, steps).await
    }
}

fn env_snapshot() -> Value {
    let mut m = Map::new();
    for (k, v) in std::env::vars() {
        if k.starts_with("LUMO_") || matches!(k.as_str(), "HOME" | "USER" | "USERNAME" | "PATH") {
            m.insert(k, Value::String(v));
        }
    }
    Value::Object(m)
}
