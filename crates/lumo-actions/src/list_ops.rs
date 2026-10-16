//! List actions (`list.*`).

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;

pub fn register(r: &mut ActionRegistry) {
    r.register(LengthAction);
    r.register(AppendAction);
    r.register(SortAction);
    r.register(UniqueAction);
    r.register(RangeAction);
    r.register(ContainsAction);
    r.register(GetAction);
    r.register(SliceAction);
    r.register(ReverseAction);
    r.register(PluckAction);

    // ---- 兼容族:13 条 list.* 扩展 ----
    r.register(CreateListAction);
    r.register(GetElemAction);
    r.register(SetElemAction);
    r.register(GetLenAction);
    r.register(GetIndexAction);
    r.register(AppendOrInsertAction);
    r.register(ExtendAction);
    r.register(RemoveAction);
    r.register(RemoveListAction);
    r.register(ClearAction);
    r.register(RemoveDuplicateAction);
    r.register(GetDuplicateAction);
    r.register(ShuffleAction);
}

fn list_schema() -> &'static Value {
    static S: Lazy<Value> = Lazy::new(crate::schema::derive::<ItemsIn>);
    &S
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ItemsIn {
    items: Vec<Value>,
}

pub struct LengthAction;
#[async_trait]
impl Action for LengthAction {
    fn id(&self) -> &'static str {
        "list.length"
    }
    fn summary(&self) -> &'static str {
        "Number of elements"
    }
    fn schema(&self) -> &'static Value {
        list_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ItemsIn { items } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.length invalid: {e}")))?;
        Ok(ActionResult::from(Value::from(items.len() as u64)))
    }
}

pub struct AppendAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AppendIn {
    items: Vec<Value>,
    value: Value,
}
#[async_trait]
impl Action for AppendAction {
    fn id(&self) -> &'static str {
        "list.append"
    }
    fn summary(&self) -> &'static str {
        "Return `items` with `value` appended"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<AppendIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let AppendIn { mut items, value } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.append invalid: {e}")))?;
        items.push(value);
        Ok(ActionResult::from(Value::Array(items)))
    }
}

pub struct SortAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SortIn {
    items: Vec<Value>,
    #[serde(default)]
    desc: bool,
    #[serde(default)]
    by: Option<String>,
}
#[async_trait]
impl Action for SortAction {
    fn id(&self) -> &'static str {
        "list.sort"
    }
    fn summary(&self) -> &'static str {
        "Sort items; supports `by: <key>` for arrays of objects"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<SortIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SortIn {
            mut items,
            desc,
            by,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.sort invalid: {e}")))?;
        items.sort_by(|a, b| {
            let av = by.as_ref().and_then(|k| a.get(k)).unwrap_or(a);
            let bv = by.as_ref().and_then(|k| b.get(k)).unwrap_or(b);
            cmp_value(av, bv)
        });
        if desc {
            items.reverse();
        }
        Ok(ActionResult::from(Value::Array(items)))
    }
}

pub(crate) fn cmp_value(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x
            .as_f64()
            .unwrap_or(0.0)
            .partial_cmp(&y.as_f64().unwrap_or(0.0))
            .unwrap_or(Ordering::Equal),
        (Value::String(x), Value::String(y)) => x.cmp(y),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Less,
        (_, Value::Null) => Ordering::Greater,
        _ => a.to_string().cmp(&b.to_string()),
    }
}

pub struct UniqueAction;
#[async_trait]
impl Action for UniqueAction {
    fn id(&self) -> &'static str {
        "list.unique"
    }
    fn summary(&self) -> &'static str {
        "De-duplicate items preserving order"
    }
    fn schema(&self) -> &'static Value {
        list_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ItemsIn { items } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.unique invalid: {e}")))?;
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<Value> = Vec::with_capacity(items.len());
        for v in items {
            let k = v.to_string();
            if seen.insert(k) {
                out.push(v);
            }
        }
        Ok(ActionResult::from(Value::Array(out)))
    }
}

pub struct RangeAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RangeIn {
    end: i64,
    #[serde(default)]
    start: i64,
    #[serde(default = "default_step")]
    step: i64,
}
fn default_step() -> i64 {
    1
}

#[async_trait]
impl Action for RangeAction {
    fn id(&self) -> &'static str {
        "list.range"
    }
    fn summary(&self) -> &'static str {
        "Generate [start, end) array with step"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<RangeIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let RangeIn { start, end, step } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.range invalid: {e}")))?;
        if step == 0 {
            return Err(StepError::msg("list.range: step must not be 0"));
        }
        let mut out = Vec::new();
        let ascending = step > 0;
        let mut i = start;
        // Hard cap so a misconfigured range can't OOM the worker.
        for _ in 0..1_000_000 {
            if (ascending && i >= end) || (!ascending && i <= end) {
                break;
            }
            out.push(Value::from(i));
            i += step;
        }
        Ok(ActionResult::from(Value::Array(out)))
    }
}

pub struct ContainsAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ContainsIn {
    items: Vec<Value>,
    value: Value,
}
#[async_trait]
impl Action for ContainsAction {
    fn id(&self) -> &'static str {
        "list.contains"
    }
    fn summary(&self) -> &'static str {
        "Whether `items` contains `value` (deep eq via JSON)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<ContainsIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ContainsIn { items, value } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.contains invalid: {e}")))?;
        Ok(ActionResult::from(Value::Bool(
            items.iter().any(|v| v == &value),
        )))
    }
}

pub struct GetAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetIn {
    items: Vec<Value>,
    index: i64,
}
#[async_trait]
impl Action for GetAction {
    fn id(&self) -> &'static str {
        "list.get"
    }
    fn summary(&self) -> &'static str {
        "Element at index (negatives count from end); null if out of range"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<GetIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let GetIn { items, index } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.get invalid: {e}")))?;
        let len = items.len() as i64;
        let idx = if index < 0 { len + index } else { index };
        let out = if idx >= 0 && idx < len {
            items[idx as usize].clone()
        } else {
            Value::Null
        };
        Ok(ActionResult::from(out))
    }
}

pub struct SliceAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SliceIn {
    items: Vec<Value>,
    start: i64,
    #[serde(default)]
    end: Option<i64>,
}
#[async_trait]
impl Action for SliceAction {
    fn id(&self) -> &'static str {
        "list.slice"
    }
    fn summary(&self) -> &'static str {
        "Slice items[start:end]; negatives count from end"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<SliceIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SliceIn { items, start, end } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.slice invalid: {e}")))?;
        let len = items.len() as i64;
        let norm = |i: i64| {
            if i < 0 {
                (len + i).max(0) as usize
            } else {
                i.min(len).max(0) as usize
            }
        };
        let s = norm(start);
        let e = end.map(norm).unwrap_or(items.len());
        let lo = s.min(e);
        let hi = s.max(e);
        Ok(ActionResult::from(Value::Array(items[lo..hi].to_vec())))
    }
}

pub struct ReverseAction;
#[async_trait]
impl Action for ReverseAction {
    fn id(&self) -> &'static str {
        "list.reverse"
    }
    fn summary(&self) -> &'static str {
        "Reverse the array"
    }
    fn schema(&self) -> &'static Value {
        list_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ItemsIn { mut items } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.reverse invalid: {e}")))?;
        items.reverse();
        Ok(ActionResult::from(Value::Array(items)))
    }
}

pub struct PluckAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PluckIn {
    items: Vec<Value>,
    key: String,
}
#[async_trait]
impl Action for PluckAction {
    fn id(&self) -> &'static str {
        "list.pluck"
    }
    fn summary(&self) -> &'static str {
        "Extract `key` field from every object element"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<PluckIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let PluckIn { items, key } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.pluck invalid: {e}")))?;
        let out: Vec<Value> = items
            .iter()
            .map(|v| v.get(&key).cloned().unwrap_or(Value::Null))
            .collect();
        Ok(ActionResult::from(Value::Array(out)))
    }
}
// ---------------------------------------------------------------------------
// 兼容族:list.* 扩展
// ---------------------------------------------------------------------------
//
// 「列表操作」的 13 条指令。与上面 LumoRPA 原生 list.* 并存 —— 原生族保持
// 零回归,本族是同名/同义门面,让流程可零改造迁移。命名重叠处(如
// `list.get_len` vs 原生 `list.length`)在 summary 里注明等价关系。

/// 按长度/初值创建列表。
pub struct CreateListAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateListIn {
    /// Number of elements to create.
    #[serde(default)]
    length: u64,
    /// Fill value for every element (default `null`).
    #[serde(default)]
    value: Value,
}
#[async_trait]
impl Action for CreateListAction {
    fn id(&self) -> &'static str {
        "list.create"
    }
    fn summary(&self) -> &'static str {
        "Create a list of `length` elements, each set to `value`"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<CreateListIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let CreateListIn { length, value } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.create invalid: {e}")))?;
        let out = vec![value; length as usize];
        Ok(ActionResult::from(Value::Array(out)))
    }
}

/// 按下标取元素(负下标从尾部数);越界返回 `default`。
pub struct GetElemAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetElemIn {
    items: Vec<Value>,
    index: i64,
    #[serde(default)]
    default: Value,
}
#[async_trait]
impl Action for GetElemAction {
    fn id(&self) -> &'static str {
        "list.get_elem"
    }
    fn summary(&self) -> &'static str {
        "Element at `index` (negative counts from the end); out of range gives `default`"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<GetElemIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let GetElemIn {
            items,
            index,
            default,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.get_elem invalid: {e}")))?;
        let at = resolve_index(index, items.len());
        let out = at.and_then(|i| items.get(i).cloned()).unwrap_or(default);
        Ok(ActionResult::from(out))
    }
}

/// 把负下标归一化到 `[0, len)`;越界返回 `None`。
fn resolve_index(index: i64, len: usize) -> Option<usize> {
    let n = len as i64;
    let i = if index < 0 { n + index } else { index };
    if i < 0 || i >= n {
        None
    } else {
        Some(i as usize)
    }
}

/// 按下标设置元素;返回修改后的列表。
pub struct SetElemAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetElemIn {
    items: Vec<Value>,
    index: i64,
    value: Value,
    /// Append when `index` is out of range instead of failing.
    #[serde(default)]
    extend: bool,
}
#[async_trait]
impl Action for SetElemAction {
    fn id(&self) -> &'static str {
        "list.set_elem"
    }
    fn summary(&self) -> &'static str {
        "Set the element at `index`; pass `extend: true` to append when out of range"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<SetElemIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SetElemIn {
            mut items,
            index,
            value,
            extend,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.set_elem invalid: {e}")))?;
        match resolve_index(index, items.len()) {
            Some(i) => items[i] = value,
            None if extend => items.push(value),
            None => {
                return Err(StepError::msg(format!(
                    "list.set_elem: index {index} is out of range for a list of {} elements (pass `extend: true` to append)",
                    items.len()
                )))
            }
        }
        Ok(ActionResult::from(Value::Array(items)))
    }
}

/// 列表长度(等价于原生 `list.length`,命名)。
pub struct GetLenAction;
#[async_trait]
impl Action for GetLenAction {
    fn id(&self) -> &'static str {
        "list.get_len"
    }
    fn summary(&self) -> &'static str {
        "Number of elements （同 `list.length`）"
    }
    fn schema(&self) -> &'static Value {
        list_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ItemsIn { items } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.get_len invalid: {e}")))?;
        Ok(ActionResult::from(Value::from(items.len() as u64)))
    }
}

/// 按值查下标;不存在返回 `-1`(语义)。
pub struct GetIndexAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetIndexIn {
    items: Vec<Value>,
    value: Value,
}
#[async_trait]
impl Action for GetIndexAction {
    fn id(&self) -> &'static str {
        "list.get_index"
    }
    fn summary(&self) -> &'static str {
        "Index of the first `value`, or -1 when absent"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<GetIndexIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let GetIndexIn { items, value } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.get_index invalid: {e}")))?;
        let idx = items
            .iter()
            .position(|v| *v == value)
            .map(|i| i as i64)
            .unwrap_or(-1);
        Ok(ActionResult::from(Value::from(idx)))
    }
}

/// 追加,或在指定位置插入。
pub struct AppendOrInsertAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AppendOrInsertIn {
    items: Vec<Value>,
    value: Value,
    /// Insert position. Omit to append at the end.
    #[serde(default)]
    position: Option<i64>,
}
#[async_trait]
impl Action for AppendOrInsertAction {
    fn id(&self) -> &'static str {
        "list.append_or_insert"
    }
    fn summary(&self) -> &'static str {
        "Append `value`, or insert it at `position` when given"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<AppendOrInsertIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let AppendOrInsertIn {
            mut items,
            value,
            position,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.append_or_insert invalid: {e}")))?;
        match position {
            None => items.push(value),
            Some(p) => {
                let len = items.len();
                // 插入位置钳到 [0, len]:超出尾部即追加,负数从尾部数。
                let at = if p < 0 {
                    (len as i64 + p).max(0) as usize
                } else {
                    (p as usize).min(len)
                };
                items.insert(at, value);
            }
        }
        Ok(ActionResult::from(Value::Array(items)))
    }
}

/// 追接另一个列表。
pub struct ExtendAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExtendIn {
    items: Vec<Value>,
    other: Vec<Value>,
}
#[async_trait]
impl Action for ExtendAction {
    fn id(&self) -> &'static str {
        "list.extend"
    }
    fn summary(&self) -> &'static str {
        "Append every element of `other` to `items`"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<ExtendIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ExtendIn { mut items, other } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.extend invalid: {e}")))?;
        items.extend(other);
        Ok(ActionResult::from(Value::Array(items)))
    }
}

/// 按值删除。
pub struct RemoveAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RemoveIn {
    items: Vec<Value>,
    value: Value,
    /// Remove at most this many occurrences (default: every one).
    #[serde(default)]
    count: Option<u64>,
}
#[async_trait]
impl Action for RemoveAction {
    fn id(&self) -> &'static str {
        "list.remove"
    }
    fn summary(&self) -> &'static str {
        "Remove occurrences of `value` (all by default, or at most `count`)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<RemoveIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let RemoveIn {
            items,
            value,
            count,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.remove invalid: {e}")))?;
        let limit = count.unwrap_or(u64::MAX);
        let mut removed = 0u64;
        let out: Vec<Value> = items
            .into_iter()
            .filter(|v| {
                if removed < limit && *v == value {
                    removed += 1;
                    false
                } else {
                    true
                }
            })
            .collect();
        Ok(ActionResult::from(Value::Array(out)))
    }
}

/// 按值列表批量删除。
pub struct RemoveListAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RemoveListIn {
    items: Vec<Value>,
    values: Vec<Value>,
}
#[async_trait]
impl Action for RemoveListAction {
    fn id(&self) -> &'static str {
        "list.remove_list"
    }
    fn summary(&self) -> &'static str {
        "Remove every element found in `values`"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<RemoveListIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let RemoveListIn { items, values } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.remove_list invalid: {e}")))?;
        let out: Vec<Value> = items.into_iter().filter(|v| !values.contains(v)).collect();
        Ok(ActionResult::from(Value::Array(out)))
    }
}

/// 清空列表。
pub struct ClearAction;
#[async_trait]
impl Action for ClearAction {
    fn id(&self) -> &'static str {
        "list.clear"
    }
    fn summary(&self) -> &'static str {
        "Return an empty list (discards `items`)"
    }
    fn schema(&self) -> &'static Value {
        list_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        // 解析入参只为 schema 校验与错误信息一致;结果恒为空数组。
        let ItemsIn { items } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.clear invalid: {e}")))?;
        let _ = items;
        Ok(ActionResult::from(Value::Array(Vec::new())))
    }
}

/// 去重,保留首次出现顺序。
pub struct RemoveDuplicateAction;
#[async_trait]
impl Action for RemoveDuplicateAction {
    fn id(&self) -> &'static str {
        "list.remove_duplicate"
    }
    fn summary(&self) -> &'static str {
        "Drop duplicates, keeping first-seen order (same as `list.unique`)"
    }
    fn schema(&self) -> &'static Value {
        list_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ItemsIn { items } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.remove_duplicate invalid: {e}")))?;
        // 用 JSON 字面量做去重键:1 与 1.0 视为不同(JSON 语义),与一致。
        let mut seen: HashSet<String> = HashSet::new();
        let out: Vec<Value> = items
            .into_iter()
            .filter(|v| seen.insert(v.to_string()))
            .collect();
        Ok(ActionResult::from(Value::Array(out)))
    }
}

/// 取出重复出现的元素。
pub struct GetDuplicateAction;
#[async_trait]
impl Action for GetDuplicateAction {
    fn id(&self) -> &'static str {
        "list.get_duplicate"
    }
    fn summary(&self) -> &'static str {
        "Elements that appear more than once, listed once in first-seen order"
    }
    fn schema(&self) -> &'static Value {
        list_schema()
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ItemsIn { items } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.get_duplicate invalid: {e}")))?;
        let mut count: HashSet<String> = HashSet::new();
        let mut dupes: Vec<Value> = Vec::new();
        let mut emitted: HashSet<String> = HashSet::new();
        for v in items {
            let key = v.to_string();
            if count.contains(&key) {
                if emitted.insert(key) {
                    dupes.push(v);
                }
            } else {
                count.insert(key);
            }
        }
        Ok(ActionResult::from(Value::Array(dupes)))
    }
}

/// 随机打乱;给 `seed` 时结果可复现(便于测试与重放)。
pub struct ShuffleAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ShuffleIn {
    items: Vec<Value>,
    /// Deterministic seed. Omit for a time-seeded shuffle.
    #[serde(default)]
    seed: Option<u64>,
}
#[async_trait]
impl Action for ShuffleAction {
    fn id(&self) -> &'static str {
        "list.shuffle"
    }
    fn summary(&self) -> &'static str {
        "Shuffle elements; pass `seed` for a reproducible order"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<ShuffleIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ShuffleIn { mut items, seed } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("list.shuffle invalid: {e}")))?;
        // xorshift64*:自带的确定性伪随机,避免为一次洗牌引入 rand 依赖树。
        let mut state = match seed {
            Some(s) => s | 1,
            None => {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0x9E37_79B9_7F4A_7C15)
                    | 1
            }
        };
        let next = |s: &mut u64| -> u64 {
            let mut x = *s;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            *s = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        // Fisher-Yates
        for i in (1..items.len()).rev() {
            let j = (next(&mut state) % (i as u64 + 1)) as usize;
            items.swap(i, j);
        }
        Ok(ActionResult::from(Value::Array(items)))
    }
}
