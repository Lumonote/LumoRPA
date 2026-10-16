//! LumoFlow AST: the in-memory representation produced by `parse_str`.
//!
//! YAML schema (excerpt):
//! ```yaml
//! apiVersion: lumorpa.io/v1
//! kind: Flow
//! metadata:
//!   id: hello
//!   version: 0.1.0
//!   name: Hello
//! spec:
//!   inputs:  [{ name: x, type: string, default: world }]
//!   outputs: [{ name: greeting, type: string }]
//!   steps:
//!     - id: greet
//!       action: control.log
//!       with: { message: "hello {{ inputs.x }}" }
//! ```

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flow {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: Metadata,
    pub spec: FlowSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub id: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub ai: Option<FlowAi>,
}

fn default_version() -> String {
    "0.1.0".into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowSpec {
    #[serde(default)]
    pub inputs: Vec<IoDecl>,
    #[serde(default)]
    pub outputs: Vec<IoDecl>,
    #[serde(default)]
    pub vault: Vec<String>,
    #[serde(default)]
    pub triggers: Vec<Trigger>,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub resources: BTreeMap<String, ResourceDecl>,
    #[serde(default)]
    pub steps: Vec<Step>,
}

/// A flow-level resource declaration (`spec.resources.<name>`): an external
/// dependency — browser, db, http session, ftp/smtp connection — that the VM
/// instantiates once per run and reuses across every step referencing it via
/// `Step.resource`, then tears down at run end (T3). `profile` is a resource
/// sub-field (e.g. a browser stealth profile); kind-specific settings flatten
/// in alongside it.
///
/// Note: no `deny_unknown_fields` here — `#[serde(flatten)]` on `config`
/// deliberately absorbs the kind-specific remainder, and the two are
/// incompatible. Typos in `kind`-specific keys are caught at open time, not parse.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceDecl {
    /// Resource kind selector, e.g. `chromium.cdp`, `sqlite`, `http`, `ftp`, `smtp`.
    pub kind: String,
    /// Optional named profile within the kind (e.g. browser `stealth-default`).
    /// `None` ⇒ the kind's default profile.
    #[serde(default)]
    pub profile: Option<String>,
    /// Kind-specific settings (url, host, headless, …), flattened so they sit
    /// directly under the resource name in YAML next to `kind`/`profile`.
    #[serde(flatten)]
    pub config: serde_yaml::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IoDecl {
    pub name: String,
    #[serde(rename = "type", default = "default_type")]
    pub ty: String,
    #[serde(default)]
    pub default: Option<serde_yaml::Value>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub description: Option<String>,
}

fn default_type() -> String {
    "string".into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    #[serde(default)]
    pub network: Vec<String>,
    #[serde(default, rename = "fs.read")]
    pub fs_read: Vec<String>,
    #[serde(default, rename = "fs.write")]
    pub fs_write: Vec<String>,
    #[serde(default)]
    pub llm: Vec<String>,
    #[serde(default)]
    pub mcp: Vec<String>,
    /// F-1 桌面输入自动化授权(粗粒度类别):`mouse`(移动/点击/滚轮)、`keyboard`
    /// (按键/文本输入),或 `*` 全部;空 = 禁用桌面输入(默认)。桌面输入可驱动整机,
    /// 故须显式授权。
    #[serde(default)]
    pub desktop: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trigger {
    pub kind: String,
    #[serde(default)]
    pub with: serde_yaml::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub id: String,
    pub action: String,
    #[serde(default)]
    pub with: serde_yaml::Value,
    #[serde(default)]
    pub retry: Option<RetrySpec>,
    /// 失败终局处置(终止/忽略/重试)。缺省 ⇒ 终止,与历史行为一致。
    #[serde(default)]
    pub on_error: Option<OnErrorSpec>,
    #[serde(default)]
    pub when: Option<String>,
    #[serde(default)]
    pub bind: Option<String>,
    /// T3: optional reference to a flow-level resource (`spec.resources.<name>`)
    /// whose shared, run-scoped handle this step should reuse. Validated against
    /// declared resources; `None` ⇒ the step opens its own connection as before.
    #[serde(default)]
    pub resource: Option<String>,
    #[serde(default)]
    pub ai: Option<StepAi>,
    /// Nested steps for control-flow actions (for / for_each / if / try / parallel).
    #[serde(default, rename = "do")]
    pub do_: Option<Vec<Step>>,
    #[serde(default, rename = "else")]
    pub else_: Option<Vec<Step>>,
    #[serde(default, rename = "catch")]
    pub catch_: Option<Vec<Step>>,
    #[serde(default, rename = "finally")]
    pub finally_: Option<Vec<Step>>,
    /// Multi-step branches for `control.parallel`. Each entry is a sequence of
    /// steps run concurrently with its siblings; entries within one branch
    /// still execute in declaration order. Mutually exclusive with `do_` for
    /// that action but `do_` is also accepted (each top-level step becomes a
    /// one-step branch).
    #[serde(default)]
    pub branches: Option<Vec<Vec<Step>>>,
}

impl Step {
    /// All children in execution order (do / else / catch / finally / branches).
    pub fn children(&self) -> Vec<&[Step]> {
        let mut v = Vec::new();
        if let Some(d) = &self.do_ {
            v.push(d.as_slice());
        }
        if let Some(e) = &self.else_ {
            v.push(e.as_slice());
        }
        if let Some(c) = &self.catch_ {
            v.push(c.as_slice());
        }
        if let Some(f) = &self.finally_ {
            v.push(f.as_slice());
        }
        if let Some(bs) = &self.branches {
            for b in bs {
                v.push(b.as_slice());
            }
        }
        v
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetrySpec {
    #[serde(default = "default_retry_times")]
    pub times: u32,
    #[serde(default = "default_backoff")]
    pub backoff: String,
    #[serde(default = "default_initial_delay")]
    pub initial_ms: u64,
    #[serde(default)]
    pub on: Vec<String>,
}

fn default_retry_times() -> u32 {
    0
}
/// 步骤级**失败终局处置**。挂在单个 `Step` 上,决定这条步骤失败之后流程怎么走。
///
/// 三种模式,对应三种朴素但都真实存在的诉求:
///
/// * `interrupt`(缺省)—— 出错即终止整个流程。这是最安全、也是历史行为;
/// * `continue`  —— 记下错误继续往下跑。适合「这一步可有可无」的步骤,
///   比如尽力而为的清理、可选的埋点,失败不该拖垮主链路;
/// * `retry`     —— 再试几次。适合抖动型失败(网络、文件被占用、UI 未就绪),
///   这类失败重试一次的收益远大于把流程整个重跑一遍。
///
/// 另有 `output`:失败时给该步骤的输出兜一个默认值。没有它的话,`continue`
/// 之后下游 `{{ steps.x }}` 会引用到一个未定义值 —— 流程带着未定义往下跑,
/// 报错点会漂移到很远的地方,比直接失败更难排查。给出兜底值就让失败**局域化**。
///
/// 与 `retry` 字段的分工:`retry` 是**重试策略**(次数/退避/错误种类过滤),
/// 本结构是**失败终局处置**。两者可叠加:先按 `retry` 重试,策略用尽后按
/// `on_error.mode` 决定终止、继续,还是再用 `retry_count` 试一轮。
/// 缺省(`None`)⇒ `interrupt`,与历史行为逐字节一致,老流程零影响。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnErrorSpec {
    /// `interrupt` (默认) | `continue` | `retry`。
    #[serde(default = "default_on_error_mode")]
    pub mode: String,
    /// `retry` 模式的额外重试次数。
    #[serde(default)]
    pub retry_count: u32,
    /// `retry` 模式的重试间隔(毫秒)。
    #[serde(default = "default_on_error_interval")]
    pub retry_interval_ms: u64,
    /// 失败时该步骤的输出兜底值,`continue`/`retry` 路径会把它交给下游。
    #[serde(default)]
    pub output: Option<serde_yaml::Value>,
}

fn default_on_error_mode() -> String {
    "interrupt".into()
}
fn default_on_error_interval() -> u64 {
    1000
}

impl OnErrorSpec {
    /// 是否需要在失败后继续执行(而不是把错误抛给上层)。
    pub fn continues(&self) -> bool {
        matches!(self.mode.as_str(), "continue" | "retry")
    }
}
fn default_backoff() -> String {
    "fixed".into()
}
fn default_initial_delay() -> u64 {
    500
}

/// Per-step AI augmentation policy. Absent ⇒ `mode: off`, fully back-compat.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepAi {
    #[serde(default)]
    pub mode: AiMode,
    /// Override `metadata.ai.model`. Empty ⇒ inherit.
    #[serde(default)]
    pub model: Option<String>,
    /// Natural-language goal. Empty ⇒ VM auto-constructs from action + with.
    #[serde(default)]
    pub prompt: Option<String>,
}

impl StepAi {
    pub fn is_enabled(&self) -> bool {
        !matches!(self.mode, AiMode::Off)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AiMode {
    #[default]
    Off,
    Fallback,
    Primary,
}

/// Flow-level AI policy. Absent ⇒ all defaults; nodes with `ai: { mode: off }` still skip AI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowAi {
    /// Master switch. `false` ⇒ ignore all step-level `ai.mode`.
    #[serde(default = "default_flow_ai_enabled")]
    pub enabled: bool,
    /// Default model for AI-enabled steps. Empty ⇒ use router active provider.
    #[serde(default)]
    pub model: Option<String>,
    /// On any step failure, attach LLM diagnostic.
    #[serde(default)]
    pub diagnose_on_failure: bool,
    #[serde(default)]
    pub budget: AiBudget,
}

impl Default for FlowAi {
    fn default() -> Self {
        Self {
            enabled: true,
            model: None,
            diagnose_on_failure: false,
            budget: AiBudget::default(),
        }
    }
}

fn default_flow_ai_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiBudget {
    #[serde(default = "default_max_calls_per_run")]
    pub max_calls_per_run: u32,
}

impl Default for AiBudget {
    fn default() -> Self {
        Self {
            max_calls_per_run: default_max_calls_per_run(),
        }
    }
}

fn default_max_calls_per_run() -> u32 {
    100
}

/// Hand-written schema to be returned by `lumo actions --show <id>` etc.
/// Avoids pulling `schemars` derive across all action crates.
pub fn flow_json_schema() -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "LumoFlow",
        "type": "object",
        "required": ["apiVersion", "kind", "metadata", "spec"],
        "properties": {
            "apiVersion": { "type": "string", "const": "lumorpa.io/v1" },
            "kind": { "type": "string", "const": "Flow" },
            "metadata": {
                "type": "object",
                "required": ["id"],
                "properties": {
                    "id":      { "type": "string" },
                    "version": { "type": "string" },
                    "name":    { "type": "string" }
                }
            },
            "spec": { "type": "object" }
        }
    })
}
