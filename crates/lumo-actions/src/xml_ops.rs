//! XML 指令族(`xml.*`):parse / build / xpath —— 补齐 SOAP/WebService、
//! 政企对接、电子发票 XML 等场景的纯数据变换能力。与 `json.*` 同族:无副作用、
//! 不碰文件系统/网络,因此与 json 族一样**不进 capability gate**。
//!
//! ## XML ⇄ JSON 映射约定(parse 与 build 互为逆映射,共用同一约定)
//!
//! - 属性 → `@attr` 键(值为字符串);
//! - 文本内容 → `#text` 键;混合内容(文本与子元素交错)把所有文本片段按出现
//!   顺序拼成一个 `#text`(有损:不保留文本与子元素的相对位置);
//! - 叶子元素(无属性、无子元素)折叠为纯字符串;空元素 `<a/>` → `null`;
//! - 重复同名子元素聚成数组(出现第二个同名子元素时把已有值升格为数组);
//! - CDATA 原样并入文本(不做实体转义还原);
//! - 命名空间前缀**原样保留**为键名的一部分(`soap:Body` 即键 `"soap:Body"`),
//!   `xmlns`/`xmlns:*` 声明按普通属性处理(`@xmlns:soap`),保证 round-trip;
//! - 根元素包一层:`<a>x</a>` → `{"a": "x"}`。
//!
//! round-trip 语义:`parse(build(parse(x))) == parse(x)`(build 时数字/布尔会
//! 写成文本,再 parse 回来即字符串,故等价以 parse 输出为基准)。
//!
//! ## 安全(XXE)
//!
//! quick-xml 只替换五个预定义实体(`&lt; &gt; &amp; &apos; &quot;`)与数字字符
//! 引用,**从不**解析 DOCTYPE 内部子集、从不展开自定义实体、从不加载外部实体/
//! 外部 DTD(没有任何 IO 路径)——XXE 在解析层面不可能发生。本模块对 DocType
//! 事件直接忽略。另设输入大小上限 `max_bytes`(默认 10 MiB)防超大报文打爆内存。
//!
//! ## XPath 选型
//!
//! `xml.xpath` 用 sxd-document + sxd-xpath:纯 Rust(无 C 依赖,信创红线),
//! 完整 XPath 1.0(轴/谓词/函数库),edition 2018 在 rustc 1.83(本仓 MSRV)
//! 可编译。两 crate 自 2020 年起 API 冻结但实现完整稳定,远优于自实现路径子集
//! (降级方案:`/a/b[2]/@id` 简化语法,仅在 sxd 不可用时启用)。注意 sxd 的
//! XPath 求值对带命名空间的文档需要显式传 `namespaces` 前缀映射,否则可用
//! `local-name()` 绕过;元素匹配结果序列化为 XML 片段时不重建 xmlns 声明
//! (前缀原样保留),适合提值与链回 `xml.parse`。

use std::collections::BTreeMap;

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use quick_xml::escape::escape;
use quick_xml::events::Event;
use quick_xml::Reader;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Map, Value};

pub fn register(r: &mut ActionRegistry) {
    r.register(ParseAction);
    r.register(BuildAction);
    r.register(XpathAction);
}

/// 默认输入上限 10 MiB:覆盖绝大多数 SOAP 报文/电子发票,又能挡住误传的大文件。
const DEFAULT_MAX_BYTES: u64 = 10 * 1024 * 1024;

fn default_max_bytes() -> u64 {
    DEFAULT_MAX_BYTES
}

fn check_size(action: &str, xml: &str, max_bytes: u64) -> Result<(), StepError> {
    if xml.len() as u64 > max_bytes {
        return Err(StepError::msg(format!(
            "{action}: input is {} bytes, exceeds max_bytes={max_bytes}",
            xml.len()
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// xml.parse
// ---------------------------------------------------------------------------

pub struct ParseAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ParseIn {
    /// XML 文本。
    xml: String,
    /// 输入大小上限(字节),默认 10 MiB。
    #[serde(default = "default_max_bytes")]
    max_bytes: u64,
}

#[async_trait]
impl Action for ParseAction {
    fn id(&self) -> &'static str {
        "xml.parse"
    }
    fn summary(&self) -> &'static str {
        "Parse XML into JSON (`@attr` attributes, `#text` text, repeated children as arrays)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<ParseIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ParseIn { xml, max_bytes } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("xml.parse invalid: {e}")))?;
        check_size("xml.parse", &xml, max_bytes)?;
        let value = parse_xml(&xml).map_err(|e| StepError::msg(format!("xml.parse: {e}")))?;
        Ok(ActionResult::from(value))
    }
}

/// 解析中的一层元素:属性已按 `@k` 预置进 map,文本片段累积在 `text`。
struct Frame {
    name: String,
    map: Map<String, Value>,
    text: String,
}

fn parse_xml(xml: &str) -> Result<Value, String> {
    let mut reader = Reader::from_str(xml);
    // trim_text:剔除元素间的缩进/换行等纯空白文本,避免格式化噪音进 `#text`。
    reader.config_mut().trim_text(true);

    let mut stack: Vec<Frame> = Vec::new();
    let mut root: Option<(String, Value)> = None;

    loop {
        let event = reader
            .read_event()
            .map_err(|e| format!("malformed XML at byte {}: {e}", reader.buffer_position()))?;
        match event {
            Event::Start(ref e) => stack.push(open_frame(e)?),
            Event::Empty(ref e) => {
                let frame = open_frame(e)?;
                close_frame(frame, &mut stack, &mut root)?;
            }
            Event::End(_) => {
                // 标签配对错误由 quick-xml 默认的 end-name 校验在 read_event 处报错。
                let frame = stack.pop().ok_or("unexpected closing tag")?;
                close_frame(frame, &mut stack, &mut root)?;
            }
            Event::Text(t) => {
                let text = t
                    .unescape()
                    .map_err(|e| format!("invalid entity/escape: {e}"))?;
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&text);
                } else if !text.trim().is_empty() {
                    return Err("text content outside of root element".into());
                }
            }
            Event::CData(t) => {
                // CDATA 内容是原始字节,不做实体还原,直接并入文本。
                let text =
                    std::str::from_utf8(t.as_ref()).map_err(|e| format!("invalid UTF-8: {e}"))?;
                match stack.last_mut() {
                    Some(top) => top.text.push_str(text),
                    None => return Err("CDATA outside of root element".into()),
                }
            }
            // 声明/注释/PI 不参与数据映射;DocType 直接忽略(quick-xml 不会展开
            // 任何自定义/外部实体,见模块头注 —— XXE 不可能)。
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) | Event::DocType(_) => {}
            Event::Eof => break,
        }
    }
    if !stack.is_empty() {
        return Err(format!("unclosed element `{}`", stack.last().unwrap().name));
    }
    match root {
        Some((name, value)) => {
            let mut out = Map::new();
            out.insert(name, value);
            Ok(Value::Object(out))
        }
        None => Err("no root element found".into()),
    }
}

fn open_frame(e: &quick_xml::events::BytesStart<'_>) -> Result<Frame, String> {
    let name = std::str::from_utf8(e.name().as_ref())
        .map_err(|e| format!("invalid UTF-8 in tag name: {e}"))?
        .to_string();
    let mut map = Map::new();
    for attr in e.attributes() {
        let attr = attr.map_err(|e| format!("bad attribute in <{name}>: {e}"))?;
        let key = std::str::from_utf8(attr.key.as_ref())
            .map_err(|e| format!("invalid UTF-8 in attribute name: {e}"))?;
        let val = attr
            .unescape_value()
            .map_err(|e| format!("bad attribute value in <{name}>: {e}"))?;
        // 属性名以 `@` 前缀进同一个 map;XML 名字不可能以 @ 开头,故与子元素键不冲突。
        map.insert(format!("@{key}"), Value::String(val.into_owned()));
    }
    Ok(Frame {
        name,
        map,
        text: String::new(),
    })
}

/// 元素收尾:折叠叶子/空元素,然后挂到父级(重复同名 → 数组)或成为根。
fn close_frame(
    frame: Frame,
    stack: &mut [Frame],
    root: &mut Option<(String, Value)>,
) -> Result<(), String> {
    let Frame {
        name,
        mut map,
        text,
    } = frame;
    let value = if map.is_empty() {
        if text.is_empty() {
            Value::Null
        } else {
            Value::String(text)
        }
    } else {
        if !text.is_empty() {
            map.insert("#text".into(), Value::String(text));
        }
        Value::Object(map)
    };
    match stack.last_mut() {
        Some(parent) => attach_child(&mut parent.map, name, value),
        None => {
            if root.is_some() {
                return Err("multiple root elements".into());
            }
            *root = Some((name, value));
        }
    }
    Ok(())
}

fn attach_child(parent: &mut Map<String, Value>, name: String, value: Value) {
    match parent.get_mut(&name) {
        Some(Value::Array(arr)) => arr.push(value),
        Some(existing) => {
            let prev = existing.take();
            *existing = Value::Array(vec![prev, value]);
        }
        None => {
            parent.insert(name, value);
        }
    }
}

// ---------------------------------------------------------------------------
// xml.build
// ---------------------------------------------------------------------------

pub struct BuildAction;

fn default_true() -> bool {
    true
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BuildIn {
    /// 按 `xml.parse` 映射约定组织的 JSON:恰好一个根键。
    value: Value,
    /// 是否输出 `<?xml version="1.0" encoding="UTF-8"?>` 声明,默认 true。
    #[serde(default = "default_true")]
    declaration: bool,
    /// 是否两空格缩进美化,默认 false(紧凑)。
    #[serde(default)]
    indent: bool,
}

#[async_trait]
impl Action for BuildAction {
    fn id(&self) -> &'static str {
        "xml.build"
    }
    fn summary(&self) -> &'static str {
        "Build an XML string from JSON (inverse of `xml.parse` mapping)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<BuildIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let BuildIn {
            value,
            declaration,
            indent,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("xml.build invalid: {e}")))?;
        let obj = value.as_object().filter(|m| m.len() == 1).ok_or_else(|| {
            StepError::msg("xml.build: `value` must be an object with exactly one root key")
        })?;
        let (name, node) = obj.iter().next().unwrap();
        if node.is_array() {
            return Err(StepError::msg(
                "xml.build: root value cannot be an array (XML has a single root element)",
            ));
        }
        let mut out = String::new();
        if declaration {
            out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
            if indent {
                out.push('\n');
            }
        }
        write_element(&mut out, name, node, 0, indent)
            .map_err(|e| StepError::msg(format!("xml.build: {e}")))?;
        Ok(ActionResult::from(Value::String(out)))
    }
}

/// 轻量标签名校验:挡住会破坏 XML 语法的字符,不做完整 NameChar 规范校验。
fn check_name(name: &str) -> Result<(), String> {
    let bad = name.is_empty()
        || name
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '<' | '>' | '&' | '"' | '\'' | '/' | '='));
    if bad {
        return Err(format!("invalid element/attribute name `{name}`"));
    }
    Ok(())
}

/// 标量(属性值 / `#text`)转文本;对象/数组在该位置非法。
fn scalar_text(ctx: &str, v: &Value) -> Result<String, String> {
    match v {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Null => Ok(String::new()),
        _ => Err(format!("{ctx} must be a scalar, got an object/array")),
    }
}

fn write_element(
    out: &mut String,
    name: &str,
    node: &Value,
    depth: usize,
    indent: bool,
) -> Result<(), String> {
    check_name(name)?;
    // 同名重复元素:数组在父级展开成兄弟节点。
    if let Value::Array(items) = node {
        for (i, item) in items.iter().enumerate() {
            if item.is_array() {
                return Err(format!("`{name}`: nested arrays cannot map to XML"));
            }
            if indent && i > 0 {
                out.push('\n');
            }
            write_element(out, name, item, depth, indent)?;
        }
        return Ok(());
    }

    let pad = if indent {
        "  ".repeat(depth)
    } else {
        String::new()
    };
    out.push_str(&pad);

    let mut attrs: Vec<(&String, &Value)> = Vec::new();
    let mut children: Vec<(&String, &Value)> = Vec::new();
    let mut text: Option<String> = None;
    match node {
        Value::Object(map) => {
            for (k, v) in map {
                if let Some(attr_name) = k.strip_prefix('@') {
                    check_name(attr_name)?;
                    attrs.push((k, v));
                } else if k == "#text" {
                    text = Some(scalar_text(&format!("`{name}` #text"), v)?);
                } else {
                    children.push((k, v));
                }
            }
        }
        Value::Null => {}
        other => text = Some(scalar_text(&format!("`{name}` content"), other)?),
    }

    out.push('<');
    out.push_str(name);
    for (k, v) in &attrs {
        let val = scalar_text(&format!("attribute `{k}` of `{name}`"), v)?;
        out.push(' ');
        out.push_str(&k[1..]);
        out.push_str("=\"");
        out.push_str(&escape(val.as_str()));
        out.push('"');
    }

    if children.is_empty() && text.is_none() {
        out.push_str("/>");
        return Ok(());
    }
    out.push('>');

    if let Some(t) = &text {
        out.push_str(&escape(t.as_str()));
    }
    if !children.is_empty() {
        for (k, v) in &children {
            if indent {
                out.push('\n');
            }
            write_element(out, k, v, depth + 1, indent)?;
        }
        if indent {
            out.push('\n');
            out.push_str(&pad);
        }
    }
    out.push_str("</");
    out.push_str(name);
    out.push('>');
    Ok(())
}

// ---------------------------------------------------------------------------
// xml.xpath
// ---------------------------------------------------------------------------

pub struct XpathAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct XpathIn {
    /// XML 文本。
    xml: String,
    /// XPath 1.0 表达式。
    expr: String,
    /// 前缀 → 命名空间 URI 映射;查询带命名空间的文档时必须提供
    /// (或在表达式里用 `local-name()` 绕过)。
    #[serde(default)]
    namespaces: BTreeMap<String, String>,
    /// 输入大小上限(字节),默认 10 MiB。
    #[serde(default = "default_max_bytes")]
    max_bytes: u64,
}

#[async_trait]
impl Action for XpathAction {
    fn id(&self) -> &'static str {
        "xml.xpath"
    }
    fn summary(&self) -> &'static str {
        "Evaluate an XPath 1.0 expression against XML; returns `matches` + `count`"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<XpathIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let XpathIn {
            xml,
            expr,
            namespaces,
            max_bytes,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("xml.xpath invalid: {e}")))?;
        check_size("xml.xpath", &xml, max_bytes)?;

        let package = sxd_document::parser::parse(&xml)
            .map_err(|e| StepError::msg(format!("xml.xpath: malformed XML: {e}")))?;
        let document = package.as_document();

        let factory = sxd_xpath::Factory::new();
        let xpath = factory
            .build(&expr)
            .map_err(|e| StepError::msg(format!("xml.xpath: invalid expression `{expr}`: {e}")))?
            .ok_or_else(|| StepError::msg("xml.xpath: empty expression"))?;

        let mut context = sxd_xpath::Context::new();
        for (prefix, uri) in &namespaces {
            context.set_namespace(prefix, uri);
        }

        let result = xpath
            .evaluate(&context, document.root())
            .map_err(|e| StepError::msg(format!("xml.xpath: evaluation failed: {e}")))?;

        // 节点集按文档序输出;标量结果(number/string/boolean)作为单元素 matches。
        let matches: Vec<Value> = match result {
            sxd_xpath::Value::Nodeset(ns) => ns.document_order().iter().map(node_to_json).collect(),
            sxd_xpath::Value::Number(n) => {
                vec![serde_json::Number::from_f64(n)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)]
            }
            sxd_xpath::Value::String(s) => vec![Value::String(s)],
            sxd_xpath::Value::Boolean(b) => vec![Value::Bool(b)],
        };
        let count = matches.len();
        Ok(ActionResult::from(
            json!({ "matches": matches, "count": count }),
        ))
    }
}

/// 节点 → JSON:元素序列化为 XML 片段(可链回 `xml.parse`),属性/文本/注释取
/// 其字符串值。序列化不重建 xmlns 声明 —— 前缀按原文保留,见模块头注。
fn node_to_json(node: &sxd_xpath::nodeset::Node<'_>) -> Value {
    use sxd_xpath::nodeset::Node;
    match node {
        Node::Element(e) => {
            let mut s = String::new();
            serialize_sxd_element(&mut s, e);
            Value::String(s)
        }
        Node::Attribute(a) => Value::String(a.value().to_string()),
        Node::Text(t) => Value::String(t.text().to_string()),
        Node::Comment(c) => Value::String(c.text().to_string()),
        Node::ProcessingInstruction(pi) => {
            Value::String(pi.value().unwrap_or_default().to_string())
        }
        Node::Namespace(ns) => Value::String(ns.uri().to_string()),
        Node::Root(r) => {
            let mut s = String::new();
            for child in r.children() {
                if let sxd_document::dom::ChildOfRoot::Element(e) = child {
                    serialize_sxd_element(&mut s, &e);
                }
            }
            Value::String(s)
        }
    }
}

fn sxd_qname(prefix: Option<&str>, local: &str) -> String {
    match prefix {
        Some(p) => format!("{p}:{local}"),
        None => local.to_string(),
    }
}

fn serialize_sxd_element(out: &mut String, e: &sxd_document::dom::Element<'_>) {
    use sxd_document::dom::ChildOfElement;
    let name = sxd_qname(e.preferred_prefix(), e.name().local_part());
    out.push('<');
    out.push_str(&name);
    for attr in e.attributes() {
        out.push(' ');
        out.push_str(&sxd_qname(
            attr.preferred_prefix(),
            attr.name().local_part(),
        ));
        out.push_str("=\"");
        out.push_str(&escape(attr.value()));
        out.push('"');
    }
    let children = e.children();
    if children.is_empty() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    for child in children {
        match child {
            ChildOfElement::Element(c) => serialize_sxd_element(out, &c),
            ChildOfElement::Text(t) => out.push_str(&escape(t.text())),
            ChildOfElement::Comment(_) | ChildOfElement::ProcessingInstruction(_) => {}
        }
    }
    out.push_str("</");
    out.push_str(&name);
    out.push('>');
}
