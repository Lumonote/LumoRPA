//! Dictionary actions (`dictionary.*`) — 字典操作。
//!
//! 创建、取值、取键、批量取值、赋值、弹出。数据模型直接复用 JSON object
//! (`serde_json::Map`),与既有的 `json.*` 族同一表示,可以互相传递,
//! 不引入第二套字典类型。
//!
//! 语义对齐说明:
//! * `dictionary.create`   「创建字典」——键值对初始化。
//! * `dictionary.get_value` 「获取字典值」——缺键返回 `default`(`null`)。
//! * `dictionary.get_keys`  「获取字典所有键」。
//! * `dictionary.get_values` 「获取字典所有值」;另支持按 `keys` 批量取。
//! * `dictionary.set`      「设置字典值」——键存在则覆盖,不存在则新增。
//! * `dictionary.pop`      「弹出字典值」——取值并从字典删除该键。
//!
//! 键序:全域保留插入序(JSON object 保序),与「按添加顺序」一致。

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value};

pub fn register(r: &mut ActionRegistry) {
    r.register(CreateAction);
    r.register(GetValueAction);
    r.register(GetKeysAction);
    r.register(GetValuesAction);
    r.register(SetAction);
    r.register(PopAction);
}

/// 把入参解析成 JSON object;非 object 时给出可读错误(而不是 panic)。
fn as_object(v: &Value, action: &str) -> Result<Map<String, Value>, StepError> {
    match v {
        Value::Object(m) => Ok(m.clone()),
        other => Err(StepError::msg(format!(
            "{action}: expected a dictionary (JSON object), got {}",
            type_name(other)
        ))),
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// 从 `entries`(键值对数组)构造字典。
pub struct CreateAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateIn {
    /// Key/value pairs; later duplicates overwrite earlier ones.
    #[serde(default)]
    entries: Vec<Entry>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Entry {
    key: String,
    #[serde(default)]
    value: Value,
}
#[async_trait]
impl Action for CreateAction {
    fn id(&self) -> &'static str {
        "dictionary.create"
    }
    fn summary(&self) -> &'static str {
        "Create a dictionary from key/value `entries`"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<CreateIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let CreateIn { entries } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dictionary.create invalid: {e}")))?;
        let mut map = Map::new();
        for Entry { key, value } in entries {
            map.insert(key, value);
        }
        Ok(ActionResult::from(Value::Object(map)))
    }
}

/// 取单个键的值;缺键返回 `default`(缺省 `null`),不报错。
pub struct GetValueAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetValueIn {
    dictionary: Value,
    key: String,
    /// Returned when `key` is absent. Defaults to `null`.
    #[serde(default)]
    default: Value,
}
#[async_trait]
impl Action for GetValueAction {
    fn id(&self) -> &'static str {
        "dictionary.get_value"
    }
    fn summary(&self) -> &'static str {
        "Get `key` from a dictionary; missing keys return `default` (null)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<GetValueIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let GetValueIn {
            dictionary,
            key,
            default,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dictionary.get_value invalid: {e}")))?;
        let map = as_object(&dictionary, "dictionary.get_value")?;
        Ok(ActionResult::from(
            map.get(&key).cloned().unwrap_or(default),
        ))
    }
}

/// 返回全部键(插入序)。
pub struct GetKeysAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DictIn {
    dictionary: Value,
}
#[async_trait]
impl Action for GetKeysAction {
    fn id(&self) -> &'static str {
        "dictionary.get_keys"
    }
    fn summary(&self) -> &'static str {
        "List all keys of a dictionary, in insertion order"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<DictIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let DictIn { dictionary } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dictionary.get_keys invalid: {e}")))?;
        let map = as_object(&dictionary, "dictionary.get_keys")?;
        let keys: Vec<Value> = map.keys().map(|k| Value::String(k.clone())).collect();
        Ok(ActionResult::from(Value::Array(keys)))
    }
}

/// 返回全部值;给出 `keys` 时按该顺序批量取(缺失键给 `null`)。
pub struct GetValuesAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetValuesIn {
    dictionary: Value,
    /// Select these keys, in this order. Omit for every value in key order.
    #[serde(default)]
    keys: Option<Vec<String>>,
}
#[async_trait]
impl Action for GetValuesAction {
    fn id(&self) -> &'static str {
        "dictionary.get_values"
    }
    fn summary(&self) -> &'static str {
        "List dictionary values; pass `keys` to select a subset in order"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<GetValuesIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let GetValuesIn { dictionary, keys } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dictionary.get_values invalid: {e}")))?;
        let map = as_object(&dictionary, "dictionary.get_values")?;
        let values: Vec<Value> = match keys {
            Some(ks) => ks
                .into_iter()
                .map(|k| map.get(&k).cloned().unwrap_or(Value::Null))
                .collect(),
            None => map.values().cloned().collect(),
        };
        Ok(ActionResult::from(Value::Array(values)))
    }
}

/// 设置键值(存在则覆盖,不存在则新增);返回更新后的字典。
pub struct SetAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetIn {
    dictionary: Value,
    key: String,
    #[serde(default)]
    value: Value,
}
#[async_trait]
impl Action for SetAction {
    fn id(&self) -> &'static str {
        "dictionary.set"
    }
    fn summary(&self) -> &'static str {
        "Set `key` to `value` in a dictionary (insert or overwrite)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<SetIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SetIn {
            dictionary,
            key,
            value,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dictionary.set invalid: {e}")))?;
        let mut map = as_object(&dictionary, "dictionary.set")?;
        map.insert(key, value);
        Ok(ActionResult::from(Value::Object(map)))
    }
}

/// 弹出:取值并删除该键。返回 `{ value, dictionary }`。
///
/// 缺键时 `value` 为 `default`(缺省 `null`),`dictionary` 原样返回 ——
/// 与「弹出字典值」一致(缺键不抛错,便于做可选消费)。
pub struct PopAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PopIn {
    dictionary: Value,
    key: String,
    #[serde(default)]
    default: Value,
}
#[async_trait]
impl Action for PopAction {
    fn id(&self) -> &'static str {
        "dictionary.pop"
    }
    fn summary(&self) -> &'static str {
        "Remove `key` from a dictionary and return `{ value, dictionary }`"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<PopIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let PopIn {
            dictionary,
            key,
            default,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dictionary.pop invalid: {e}")))?;
        let mut map = as_object(&dictionary, "dictionary.pop")?;
        let value = map.remove(&key).unwrap_or(default);
        let mut out = Map::new();
        out.insert("value".into(), value);
        out.insert("dictionary".into(), Value::Object(map));
        Ok(ActionResult::from(Value::Object(out)))
    }
}
