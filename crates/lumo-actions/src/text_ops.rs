//! Text actions (`text.*`) — 文本操作。
//!
//! 与既有 `string.*` 族**并存**:`string.*` 是原生命名,本族是同名门面指令,
//! 语义对齐,便于流程零改造成本迁移。两者都是纯数据变换,无 capability 门禁。
//!
//! 长度语义:全部按 **Unicode 字符数**(`chars().count()`)而非字节数 ——
//! 面向中文用户,「文本长度」是字符数(`abc`=3、`中文`=2)。若按字节数,
//! `中文` 会得到 6,与用户预期不符。

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use regex::Regex;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

pub fn register(r: &mut ActionRegistry) {
    r.register(AppendTextInNewlineAction);
    r.register(ChangeTextCaseAction);
    r.register(ExtractContentAction);
    r.register(GetTextLengthAction);
    r.register(JoinListToTextAction);
    r.register(PadTextAction);
    r.register(ReplaceContentAction);
    r.register(SplitTextAction);
    r.register(SubTextAction);
    r.register(TrimTextAction);
}

/// 编译正则并把失败转成可读的 action 错误(而不是 panic)。
fn compile(pattern: &str, action: &str) -> Result<Regex, StepError> {
    Regex::new(pattern)
        .map_err(|e| StepError::msg(format!("{action}: invalid regular expression: {e}")))
}

/// 追加文本并在末尾补换行符。
pub struct AppendTextInNewlineAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AppendTextInNewlineIn {
    text: String,
    append: String,
}
#[async_trait]
impl Action for AppendTextInNewlineAction {
    fn id(&self) -> &'static str {
        "text.append_text_in_newline"
    }
    fn summary(&self) -> &'static str {
        "Append `append` to `text` and terminate the result with a newline"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<AppendTextInNewlineIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let AppendTextInNewlineIn { text, append } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("text.append_text_in_newline invalid: {e}")))?;
        let mut out = text;
        out.push_str(&append);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        Ok(ActionResult::from(Value::String(out)))
    }
}

/// 转换文本大小写。
pub struct ChangeTextCaseAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ChangeTextCaseIn {
    text: String,
    /// One of `uppercase`, `lowercase`, `titlecase`, `capitalize`, `swapcase`.
    case: String,
}
#[async_trait]
impl Action for ChangeTextCaseAction {
    fn id(&self) -> &'static str {
        "text.change_text_case"
    }
    fn summary(&self) -> &'static str {
        "Change case: uppercase | lowercase | titlecase | capitalize | swapcase"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<ChangeTextCaseIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ChangeTextCaseIn { text, case } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("text.change_text_case invalid: {e}")))?;
        let out = match case.as_str() {
            "uppercase" | "upper" => text.to_uppercase(),
            "lowercase" | "lower" => text.to_lowercase(),
            // 逐词首字母大写,其余小写 —— 与「标题格式」一致。
            "titlecase" | "title" => text
                .split_inclusive(' ')
                .map(|w| {
                    let mut chars = w.chars();
                    match chars.next() {
                        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
                        None => String::new(),
                    }
                })
                .collect(),
            // 仅整串首字符大写,其余保持原样。
            "capitalize" => {
                let mut chars = text.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            }
            "swapcase" => text
                .chars()
                .map(|c| {
                    if c.is_uppercase() {
                        c.to_lowercase().collect::<String>()
                    } else if c.is_lowercase() {
                        c.to_uppercase().collect::<String>()
                    } else {
                        c.to_string()
                    }
                })
                .collect(),
            other => {
                return Err(StepError::msg(format!(
                    "text.change_text_case: unknown case `{other}` (expected uppercase | lowercase | titlecase | capitalize | swapcase)"
                )))
            }
        };
        Ok(ActionResult::from(Value::String(out)))
    }
}

/// 正则提取文本内容。
pub struct ExtractContentAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExtractContentIn {
    text: String,
    pattern: String,
    /// Return every match instead of only the first.
    #[serde(default)]
    all: bool,
    /// Return capture group N instead of the whole match.
    #[serde(default)]
    group: Option<usize>,
}
#[async_trait]
impl Action for ExtractContentAction {
    fn id(&self) -> &'static str {
        "text.extract_content_from_text"
    }
    fn summary(&self) -> &'static str {
        "Extract text by regular expression; `all` returns every match"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<ExtractContentIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ExtractContentIn {
            text,
            pattern,
            all,
            group,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("text.extract_content_from_text invalid: {e}")))?;
        let re = compile(&pattern, "text.extract_content_from_text")?;
        let pick = |caps_match: &str, caps: Option<&regex::Captures>| -> Value {
            match group {
                None => Value::String(caps_match.to_string()),
                Some(g) => match caps.and_then(|c| c.get(g)) {
                    Some(m) => Value::String(m.as_str().to_string()),
                    None => Value::Null,
                },
            }
        };
        if all {
            let out: Vec<Value> = re
                .captures_iter(&text)
                .map(|c| {
                    let whole = c.get(0).map(|m| m.as_str()).unwrap_or_default().to_string();
                    pick(&whole, Some(&c))
                })
                .collect();
            return Ok(ActionResult::from(Value::Array(out)));
        }
        let out = match re.captures(&text) {
            Some(c) => {
                let whole = c.get(0).map(|m| m.as_str()).unwrap_or_default().to_string();
                pick(&whole, Some(&c))
            }
            // 无匹配 → null(而不是空串),让调用方能用 `control.if` 区分
            // 「匹配到空串」与「没匹配到」。
            None => Value::Null,
        };
        Ok(ActionResult::from(out))
    }
}

/// 文本长度(按字符数)。
pub struct GetTextLengthAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetTextLengthIn {
    text: String,
}
#[async_trait]
impl Action for GetTextLengthAction {
    fn id(&self) -> &'static str {
        "text.get_text_length"
    }
    fn summary(&self) -> &'static str {
        "Text length in Unicode characters (`中文` is 2)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<GetTextLengthIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let GetTextLengthIn { text } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("text.get_text_length invalid: {e}")))?;
        Ok(ActionResult::from(Value::from(text.chars().count() as u64)))
    }
}

/// 把列表拼成文本。
pub struct JoinListToTextAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct JoinListToTextIn {
    items: Vec<Value>,
    #[serde(default)]
    separator: String,
}
#[async_trait]
impl Action for JoinListToTextAction {
    fn id(&self) -> &'static str {
        "text.join_list_to_text"
    }
    fn summary(&self) -> &'static str {
        "Join a list into text with `separator` (default empty)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<JoinListToTextIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let JoinListToTextIn { items, separator } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("text.join_list_to_text invalid: {e}")))?;
        let parts: Vec<String> = items
            .iter()
            .map(|v| match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect();
        Ok(ActionResult::from(Value::String(parts.join(&separator))))
    }
}

/// 填充文本到指定长度。
pub struct PadTextAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PadTextIn {
    text: String,
    /// Target length in Unicode characters. Already-longer text is returned as-is.
    length: u64,
    /// `left` (default) | `right` | `both`.
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    fill: Option<String>,
}
#[async_trait]
impl Action for PadTextAction {
    fn id(&self) -> &'static str {
        "text.pad_text"
    }
    fn summary(&self) -> &'static str {
        "Pad text to `length` characters: direction left | right | both"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<PadTextIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let PadTextIn {
            text,
            length,
            direction,
            fill,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("text.pad_text invalid: {e}")))?;
        let target = length as usize;
        let cur = text.chars().count();
        if cur >= target {
            return Ok(ActionResult::from(Value::String(text)));
        }
        let pad_len = target - cur;
        let fill_ch = fill
            .as_deref()
            .and_then(|f| f.chars().next())
            .unwrap_or(' ');
        let dir = direction.as_deref().unwrap_or("left");
        let make = |n: usize| -> String { std::iter::repeat_n(fill_ch, n).collect() };
        let out = match dir {
            "left" => {
                let mut s = make(pad_len);
                s.push_str(&text);
                s
            }
            "right" => {
                let mut s = text;
                s.push_str(&make(pad_len));
                s
            }
            "both" => {
                let left = pad_len / 2;
                let right = pad_len - left;
                let mut s = make(left);
                s.push_str(&text);
                s.push_str(&make(right));
                s
            }
            other => {
                return Err(StepError::msg(format!(
                    "text.pad_text: unknown direction `{other}` (expected left | right | both)"
                )))
            }
        };
        Ok(ActionResult::from(Value::String(out)))
    }
}

/// 正则替换文本。
pub struct ReplaceContentAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReplaceContentIn {
    text: String,
    pattern: String,
    replacement: String,
    /// Replace every occurrence instead of only the first.
    #[serde(default)]
    all: bool,
}
#[async_trait]
impl Action for ReplaceContentAction {
    fn id(&self) -> &'static str {
        "text.replace_content_from_text"
    }
    fn summary(&self) -> &'static str {
        "Replace by regular expression; `replacement` supports $1 capture refs"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<ReplaceContentIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ReplaceContentIn {
            text,
            pattern,
            replacement,
            all,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("text.replace_content_from_text invalid: {e}")))?;
        let re = compile(&pattern, "text.replace_content_from_text")?;
        let out = if all {
            re.replace_all(&text, replacement.as_str()).to_string()
        } else {
            re.replace(&text, replacement.as_str()).to_string()
        };
        Ok(ActionResult::from(Value::String(out)))
    }
}

/// 按分隔符拆分为列表。
pub struct SplitTextAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SplitTextIn {
    text: String,
    #[serde(default)]
    separator: Option<String>,
    /// Drop empty segments from the result.
    #[serde(default)]
    remove_empty: bool,
}
#[async_trait]
impl Action for SplitTextAction {
    fn id(&self) -> &'static str {
        "text.split_text_to_list"
    }
    fn summary(&self) -> &'static str {
        "Split text by `separator` (default: any whitespace) into a list"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<SplitTextIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SplitTextIn {
            text,
            separator,
            remove_empty,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("text.split_text_to_list invalid: {e}")))?;
        let mut parts: Vec<String> = match separator.as_deref() {
            // 无分隔符 → 「按空白拆分」:任意连续空白都算一个边界。
            None => text.split_whitespace().map(|s| s.to_string()).collect(),
            Some(sep) => text.split(sep).map(|s| s.to_string()).collect(),
        };
        if remove_empty {
            parts.retain(|s| !s.is_empty());
        }
        Ok(ActionResult::from(Value::Array(
            parts.into_iter().map(Value::String).collect(),
        )))
    }
}

/// 取子串(按字符)。
pub struct SubTextAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SubTextIn {
    text: String,
    /// Start index in characters. Negative counts from the end.
    #[serde(default)]
    start: i64,
    /// How many characters to take. Omit for the rest of the text.
    #[serde(default)]
    length: Option<u64>,
}
#[async_trait]
impl Action for SubTextAction {
    fn id(&self) -> &'static str {
        "text.sub_text"
    }
    fn summary(&self) -> &'static str {
        "Substring by character; `start` may be negative, out-of-range clamps"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<SubTextIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SubTextIn {
            text,
            start,
            length,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("text.sub_text invalid: {e}")))?;
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len() as i64;
        // 负索引从尾部数;越界钳到合法区间(不 panic)—— 语义:越界取空串。
        let begin = if start < 0 {
            (n + start).max(0)
        } else {
            start.min(n)
        };
        let end = match length {
            Some(l) => (begin + l as i64).min(n),
            None => n,
        };
        let out: String = if begin >= end {
            String::new()
        } else {
            chars[begin as usize..end as usize].iter().collect()
        };
        Ok(ActionResult::from(Value::String(out)))
    }
}

/// 去除首尾空白。
pub struct TrimTextAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TrimTextIn {
    text: String,
    /// `both` (default) | `left` | `right`.
    #[serde(default)]
    mode: Option<String>,
    /// Trim these characters instead of Unicode whitespace.
    #[serde(default)]
    chars: Option<String>,
}
#[async_trait]
impl Action for TrimTextAction {
    fn id(&self) -> &'static str {
        "text.trim_text"
    }
    fn summary(&self) -> &'static str {
        "Trim whitespace (or `chars`) from both | left | right ends"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<TrimTextIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let TrimTextIn { text, mode, chars } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("text.trim_text invalid: {e}")))?;
        let mode = mode.as_deref().unwrap_or("both");
        let out = match mode {
            "both" => match chars.as_deref() {
                Some(set) => text.trim_matches(|c| set.contains(c)).to_string(),
                None => text.trim().to_string(),
            },
            "left" => match chars.as_deref() {
                Some(set) => text.trim_start_matches(|c| set.contains(c)).to_string(),
                None => text.trim_start().to_string(),
            },
            "right" => match chars.as_deref() {
                Some(set) => text.trim_end_matches(|c| set.contains(c)).to_string(),
                None => text.trim_end().to_string(),
            },
            other => {
                return Err(StepError::msg(format!(
                    "text.trim_text: unknown mode `{other}` (expected both | left | right)"
                )))
            }
        };
        Ok(ActionResult::from(Value::String(out)))
    }
}
