//! P1-7:步级静态校验(action 存在性 + `with:` schema + capability 声明 +
//! skill 引用)的**单一实现**。
//!
//! 此前 desktop(`apps/desktop/src-tauri/src/lib.rs`)与 CLI
//! (`crates/lumo-cli/src/cmd/validate.rs`)各维护一份几乎相同的
//! `validate_steps`,行为已经漂移(desktop 把缺省 `with:` 当硬错误,CLI 视为
//! 空对象)。上移到 lumo-core——它同时能看到 `ActionRegistry` 与 lumo-dsl 的
//! AST——两端共用。
//!
//! skill 存在性检查通过 `skill_exists` 闭包注入:lumo-skills 依赖 lumo-core,
//! lumo-core 不能反向依赖它(循环依赖),调用方传
//! `&|name| skills.get(name).is_some()` 即可。

use crate::registry::ActionRegistry;
use lumo_dsl::{Capabilities, Step};
use serde_json::Value;

/// 递归校验一棵步骤树:
/// 1. `action` 必须已在 registry 注册;
/// 2. 需要 capability 的动作必须有对应的 `spec.capabilities` 声明(空 = 未授权);
///    需求清单从 lumo-dsl 的 `caps::ACTION_CAPS` 单一声明表派生(P1-2,与运行期
///    `StepCtx::ensure_*` 门禁及 lint 同源,不再各自硬编码);
/// 3. `with:` 按动作 JSON schema 做静态校验(required / additionalProperties / 类型);
/// 4. `skill.invoke` 的字面量 `name` 必须能在 skill 注册表里找到;
/// 5. P2-6:`spec.capabilities.network` 的 grant 必须是裸 host 形状 —— 运行期
///    门禁只拿 `extract_host` 剥出的裸 host 比对,带 scheme/path/port 的 grant
///    永远匹配不上,是静默死配置,直接拒绝并给出正确写法。
pub fn validate_steps(
    steps: &[Step],
    capabilities: &Capabilities,
    registry: &ActionRegistry,
    skill_exists: &dyn Fn(&str) -> bool,
) -> anyhow::Result<()> {
    for grant in &capabilities.network {
        if let Some(msg) = lumo_dsl::caps::network_grant_shape_error(grant) {
            anyhow::bail!("spec.capabilities.network: {msg}");
        }
    }
    validate_steps_inner(steps, capabilities, registry, skill_exists)
}

fn validate_steps_inner(
    steps: &[Step],
    capabilities: &Capabilities,
    registry: &ActionRegistry,
    skill_exists: &dyn Fn(&str) -> bool,
) -> anyhow::Result<()> {
    for step in steps {
        let action = registry.get(&step.action).ok_or_else(|| {
            anyhow::anyhow!("unknown action `{}` in step `{}`", step.action, step.id)
        })?;
        let input = serde_json::to_value(&step.with).unwrap_or(Value::Null);
        validate_capability_declaration(step, &input, capabilities)?;
        validate_schema(&step.id, &step.action, &input, action.schema())?;
        validate_skill_reference(step, &input, skill_exists)?;
        for children in step.children() {
            validate_steps_inner(children, capabilities, registry, skill_exists)?;
        }
    }
    Ok(())
}

fn validate_capability_declaration(
    step: &Step,
    input: &Value,
    capabilities: &Capabilities,
) -> anyhow::Result<()> {
    let Some(spec) = lumo_dsl::caps::action_caps(&step.action) else {
        return Ok(());
    };
    for cap in spec.required {
        if !cap.is_granted(capabilities) {
            anyhow::bail!(
                "step `{}` action `{}` requires spec.capabilities.{}",
                step.id,
                step.action,
                cap.spec_field()
            );
        }
    }
    for (key, cap) in spec.conditional {
        if lumo_dsl::caps::conditional_key_engaged(input, key) && !cap.is_granted(capabilities) {
            anyhow::bail!(
                "step `{}` action `{}` requires spec.capabilities.{} when with.{key} is set",
                step.id,
                step.action,
                cap.spec_field()
            );
        }
    }
    Ok(())
}

fn validate_skill_reference(
    step: &Step,
    input: &Value,
    skill_exists: &dyn Fn(&str) -> bool,
) -> anyhow::Result<()> {
    if step.action != "skill.invoke" {
        return Ok(());
    }
    let Some(name) = input.get("name").and_then(Value::as_str) else {
        return Ok(());
    };
    if is_template_string(name) {
        return Ok(());
    }
    if !skill_exists(name) {
        anyhow::bail!("step `{}` invokes unknown skill `{name}`", step.id);
    }
    Ok(())
}

fn validate_schema(
    step_id: &str,
    action_id: &str,
    input: &Value,
    schema: &Value,
) -> anyhow::Result<()> {
    if schema.get("type").and_then(Value::as_str) == Some("object") {
        // 缺省的 `with:`(如把配置放 `branches:` 的 control.parallel)反序列化为
        // Null——按空对象处理:schema 没有 required 时干净通过,有 required 仍报错。
        // (合一前 desktop 把 Null 当硬错误,会误杀合法 flow;以 CLI 行为为准。)
        // 真正写成标量/数组的 `with:` 依旧报错。
        let empty = serde_json::Map::new();
        let input_obj = match input {
            Value::Null => &empty,
            Value::Object(map) => map,
            _ => anyhow::bail!("step `{step_id}` action `{action_id}` with: must be an object"),
        };
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for key in required.iter().filter_map(Value::as_str) {
                if !input_obj.contains_key(key) {
                    anyhow::bail!(
                        "step `{step_id}` action `{action_id}` missing required with.{key}"
                    );
                }
            }
        }
        let properties = schema.get("properties").and_then(Value::as_object);
        if schema.get("additionalProperties").and_then(Value::as_bool) == Some(false) {
            for key in input_obj.keys() {
                if !properties
                    .map(|props| props.contains_key(key))
                    .unwrap_or(false)
                {
                    anyhow::bail!("step `{step_id}` action `{action_id}` has unknown with.{key}");
                }
            }
        }
        if let Some(properties) = properties {
            for (key, value) in input_obj {
                if let Some(prop_schema) = properties.get(key) {
                    validate_value_type(step_id, action_id, key, value, prop_schema)?;
                }
            }
        }
    }
    Ok(())
}

fn validate_value_type(
    step_id: &str,
    action_id: &str,
    key: &str,
    value: &Value,
    schema: &Value,
) -> anyhow::Result<()> {
    // 模板字符串运行期才知道实际类型,静态校验放行。
    if value.as_str().map(is_template_string).unwrap_or(false) {
        return Ok(());
    }
    let Some(expected) = schema.get("type") else {
        return Ok(());
    };
    let ok = match expected {
        Value::String(s) => json_type_matches(s, value),
        Value::Array(types) => types
            .iter()
            .filter_map(Value::as_str)
            .any(|s| json_type_matches(s, value)),
        _ => true,
    };
    if !ok {
        anyhow::bail!(
            "step `{step_id}` action `{action_id}` with.{key} expected {}, got {}",
            expected,
            json_kind(value)
        );
    }
    Ok(())
}

fn json_type_matches(expected: &str, value: &Value) -> bool {
    match expected {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        _ => true,
    }
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn is_template_string(s: &str) -> bool {
    s.contains("{{") || s.contains("{%")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{Action, ActionResult};
    use crate::ctx::StepCtx;
    use crate::error::StepError;
    use serde_json::json;

    fn object_schema() -> Value {
        json!({ "type": "object", "properties": {}, "additionalProperties": true })
    }

    // ---- validate_schema(原 CLI 侧测试随实现一起上移) ----------------------

    #[test]
    fn absent_with_is_valid_against_object_schema() {
        // control.parallel 等动作把配置放 `branches:`,`with` 反序列化为 Null,
        // 必须干净通过(desktop 旧实现在这里误报)。
        assert!(validate_schema("s", "control.parallel", &json!(null), &object_schema()).is_ok());
    }

    #[test]
    fn empty_object_with_is_valid() {
        assert!(validate_schema("s", "control.close", &json!({}), &object_schema()).is_ok());
    }

    #[test]
    fn non_object_with_still_errors() {
        // 标量 / 数组 `with:` 是真实的书写错误,必须拦下。
        let err = validate_schema("s", "browser.open", &json!("oops"), &object_schema());
        assert!(err.is_err(), "scalar with should be rejected");
        let err2 = validate_schema("s", "browser.open", &json!(["a"]), &object_schema());
        assert!(err2.is_err(), "array with should be rejected");
    }

    #[test]
    fn missing_required_key_errors_even_when_absent() {
        let schema = json!({ "type": "object", "properties": { "url": {} }, "required": ["url"] });
        assert!(validate_schema("s", "browser.open", &json!(null), &schema).is_err());
    }

    // ---- validate_steps 端到端 ------------------------------------------------

    struct Noop;
    #[async_trait::async_trait]
    impl Action for Noop {
        fn id(&self) -> &'static str {
            "test.noop"
        }
        async fn execute(
            &self,
            _ctx: &mut StepCtx,
            _input: Value,
        ) -> Result<ActionResult, StepError> {
            Ok(ActionResult::null())
        }
    }

    fn step(id: &str, action: &str) -> Step {
        serde_yaml::from_str(&format!("{{ id: {id}, action: {action} }}")).unwrap()
    }

    #[test]
    fn unknown_action_is_rejected() {
        let registry = ActionRegistry::new();
        let err = validate_steps(
            &[step("a", "no.such_action")],
            &Capabilities::default(),
            &registry,
            &|_| true,
        )
        .expect_err("unknown action must be rejected");
        assert!(err.to_string().contains("unknown action"), "got: {err}");
    }

    #[test]
    fn known_action_passes() {
        let mut registry = ActionRegistry::new();
        registry.register(Noop);
        validate_steps(
            &[step("a", "test.noop")],
            &Capabilities::default(),
            &registry,
            &|_| true,
        )
        .expect("registered action with empty schema must validate");
    }

    #[test]
    fn skill_reference_uses_injected_lookup() {
        struct Invoke;
        #[async_trait::async_trait]
        impl Action for Invoke {
            fn id(&self) -> &'static str {
                "skill.invoke"
            }
            async fn execute(
                &self,
                _ctx: &mut StepCtx,
                _input: Value,
            ) -> Result<ActionResult, StepError> {
                Ok(ActionResult::null())
            }
        }
        let mut registry = ActionRegistry::new();
        registry.register(Invoke);
        let steps: Vec<Step> =
            vec![
                serde_yaml::from_str("{ id: a, action: skill.invoke, with: { name: ghost } }")
                    .unwrap(),
            ];
        // 注入的查找闭包说 skill 不存在 → 报错;说存在 → 通过。
        let err = validate_steps(&steps, &Capabilities::default(), &registry, &|_| false)
            .expect_err("unknown skill must be rejected");
        assert!(err.to_string().contains("unknown skill"), "got: {err}");
        validate_steps(&steps, &Capabilities::default(), &registry, &|n| {
            n == "ghost"
        })
        .expect("existing skill must validate");
    }

    // ---- P1-2 / P2-6:capability 声明表派生 + network grant 形状 ---------------

    /// 任意 id 的空动作桩:capability 检查只看 `caps::ACTION_CAPS` 表,不看实现。
    struct Stub(&'static str);
    #[async_trait::async_trait]
    impl Action for Stub {
        fn id(&self) -> &'static str {
            self.0
        }
        async fn execute(
            &self,
            _ctx: &mut StepCtx,
            _input: Value,
        ) -> Result<ActionResult, StepError> {
            Ok(ActionResult::null())
        }
    }

    fn caps_with_network(grants: &[&str]) -> Capabilities {
        Capabilities {
            network: grants.iter().map(|g| g.to_string()).collect(),
            ..Capabilities::default()
        }
    }

    #[test]
    fn capability_requirements_derive_from_caps_table() {
        // 旧硬编码清单看不见的动作(notify.send / db.postgres_query)现在也拦截。
        for action in ["notify.send", "db.postgres_query"] {
            let mut registry = ActionRegistry::new();
            registry.register(Stub(action));
            let steps = [step("a", action)];
            let err = validate_steps(&steps, &Capabilities::default(), &registry, &|_| true)
                .expect_err("empty network grants must be rejected");
            assert!(
                err.to_string().contains("spec.capabilities.network"),
                "{action}: {err}"
            );
            validate_steps(&steps, &caps_with_network(&["*"]), &registry, &|_| true)
                .unwrap_or_else(|e| panic!("{action} with network grant must pass: {e}"));
        }
    }

    #[test]
    fn conditional_capability_checked_only_when_key_present() {
        let mut registry = ActionRegistry::new();
        registry.register(Stub("human.approve"));
        // 无 notify 字段:human.approve 不需要任何 capability。
        let plain: Vec<Step> = vec![serde_yaml::from_str(
            "{ id: gate, action: human.approve, with: { message: 'ship?' } }",
        )
        .unwrap()];
        validate_steps(&plain, &Capabilities::default(), &registry, &|_| true)
            .expect("human.approve without notify needs no capability");
        // 带 notify 字段:复用 notify.send 的网络门禁 → 必须声明 network。
        let notifying: Vec<Step> = vec![serde_yaml::from_str(
            "{ id: gate, action: human.approve, with: { message: 'ship?', notify: { provider: webhook, url: x } } }",
        )
        .unwrap()];
        let err = validate_steps(&notifying, &Capabilities::default(), &registry, &|_| true)
            .expect_err("human.approve with notify must require network");
        assert!(
            err.to_string().contains("when with.notify is set"),
            "got: {err}"
        );
        validate_steps(&notifying, &caps_with_network(&["*"]), &registry, &|_| true)
            .expect("granted network must pass");
    }

    #[test]
    fn network_grant_shape_is_rejected_with_example() {
        // P2-6:带 scheme/path 的 grant 运行期永不匹配(门禁只比对裸 host)。
        let registry = ActionRegistry::new();
        for bad in ["https://example.com", "example.com/api", "db.internal:5432"] {
            let err = validate_steps(&[], &caps_with_network(&[bad]), &registry, &|_| true)
                .expect_err("dead network grant must be rejected");
            let msg = err.to_string();
            assert!(
                msg.contains("spec.capabilities.network") && msg.contains("instead"),
                "`{bad}` 报错须给正确写法: {msg}"
            );
        }
        // 裸 host / 通配 / 环境变量形状放行。
        validate_steps(
            &[],
            &caps_with_network(&["example.com", "*.corp.cn", "*", "${DB_HOST}"]),
            &registry,
            &|_| true,
        )
        .expect("bare-host grants must pass");
    }
}
