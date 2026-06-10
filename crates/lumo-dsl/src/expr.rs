//! B1 (F-14): boolean predicate evaluation for `when` / `control.if`.
//!
//! [`eval_predicate`] picks one of two modes from the RAW (pre-render) source:
//!
//!   * **Template mode** — the source contains `{{ }}` or `{% %}`: it is
//!     rendered with minijinja exactly as before, then the result's truthiness
//!     is returned. This keeps every existing `{{ ... }}`-based condition
//!     working unchanged.
//!   * **Expression mode** — otherwise: the source is parsed as a small boolean
//!     expression and evaluated against the context. Grammar (lowest → highest
//!     precedence):
//!
//!     ```text
//!     or    := and ('||' and)*
//!     and   := cmp ('&&' cmp)*
//!     cmp   := unary ( ('=='|'!='|'<'|'<='|'>'|'>='|'in') unary )?
//!     unary := '!' unary | atom
//!     atom  := NUMBER | STRING | true | false | null | IDENT_PATH | '(' or ')'
//!     ```
//!
//!     Identifier paths (`inputs.x`, `steps.y.result`, `vars.z`, loop bindings)
//!     resolve against the namespaces; a bare token that resolves to nothing is
//!     treated as a string literal, so plain truthy strings (`yes`) still work.
//!     A syntactically malformed expression falls back to string truthiness
//!     rather than erroring, so a condition that worked before keeps working.

use crate::template::lookup_path;
use crate::{render, TemplateCtx, TemplateError};
use serde_json::Value as Json;

/// Evaluate a `when` / `control.if` condition string to a boolean.
///
/// Errors only propagate from template mode (a minijinja render error);
/// expression mode never errors — an unparseable expression degrades to string
/// truthiness.
pub fn eval_predicate(raw: &str, ctx: &TemplateCtx) -> Result<bool, TemplateError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        // An empty condition is falsy (matches the old `is_truthy_str("")`).
        return Ok(false);
    }
    if trimmed.contains("{{") || trimmed.contains("{%") {
        // Template mode: render with the full template engine, then truthiness.
        let rendered = render(&Json::String(raw.to_string()), ctx)?;
        return Ok(truthy(&rendered));
    }
    // Expression mode (with a forgiving fallback for non-expressions).
    match eval_expr(trimmed, ctx) {
        Some(v) => Ok(truthy(&v)),
        None => Ok(is_truthy_str(trimmed)),
    }
}

/// JSON truthiness, matching the VM's historical `is_truthy`.
fn truthy(v: &Json) -> bool {
    match v {
        Json::Bool(b) => *b,
        Json::Null => false,
        Json::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Json::String(s) => is_truthy_str(s),
        Json::Array(a) => !a.is_empty(),
        Json::Object(o) => !o.is_empty(),
    }
}

/// String truthiness, matching the VM's historical `is_truthy_str`.
fn is_truthy_str(s: &str) -> bool {
    !matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "" | "false" | "0" | "null" | "none" | "no"
    )
}

/// Resolve a dotted identifier path against the context namespaces. A path
/// whose head is not a namespace/binding resolves to a string literal of its
/// own text (so `yes` / `done` behave as plain strings).
fn resolve_ident(ctx: &TemplateCtx, ident: &str) -> Json {
    let path: Vec<String> = ident.split('.').map(|s| s.to_string()).collect();
    lookup_path(ctx, &path).unwrap_or_else(|| Json::String(ident.to_string()))
}

// ─── tokenizer ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    EqEq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    In,
    And,
    Or,
    Bang,
    LParen,
    RParen,
}

fn tokenize(src: &str) -> Option<Vec<Tok>> {
    let b: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let next = b.get(i + 1).copied();
        match c {
            '=' if next == Some('=') => {
                out.push(Tok::EqEq);
                i += 2;
            }
            '!' if next == Some('=') => {
                out.push(Tok::NotEq);
                i += 2;
            }
            '!' => {
                out.push(Tok::Bang);
                i += 1;
            }
            '<' if next == Some('=') => {
                out.push(Tok::Le);
                i += 2;
            }
            '<' => {
                out.push(Tok::Lt);
                i += 1;
            }
            '>' if next == Some('=') => {
                out.push(Tok::Ge);
                i += 2;
            }
            '>' => {
                out.push(Tok::Gt);
                i += 1;
            }
            '&' if next == Some('&') => {
                out.push(Tok::And);
                i += 2;
            }
            '|' if next == Some('|') => {
                out.push(Tok::Or);
                i += 2;
            }
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            '\'' | '"' => {
                let quote = c;
                let mut s = String::new();
                i += 1;
                let mut closed = false;
                while i < b.len() {
                    if b[i] == quote {
                        closed = true;
                        i += 1;
                        break;
                    }
                    s.push(b[i]);
                    i += 1;
                }
                if !closed {
                    return None; // unterminated string literal
                }
                out.push(Tok::Str(s));
            }
            '0'..='9' => i = scan_number(&b, i, &mut out)?,
            '-' if matches!(next, Some(d) if d.is_ascii_digit()) => {
                i = scan_number(&b, i, &mut out)?
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < b.len() && (b[i].is_alphanumeric() || b[i] == '_' || b[i] == '.') {
                    i += 1;
                }
                let id: String = b[start..i].iter().collect();
                if id == "in" {
                    out.push(Tok::In);
                } else {
                    out.push(Tok::Ident(id));
                }
            }
            _ => return None, // unknown character
        }
    }
    Some(out)
}

fn scan_number(b: &[char], start: usize, out: &mut Vec<Tok>) -> Option<usize> {
    let mut i = start;
    if b[i] == '-' {
        i += 1;
    }
    while i < b.len() && (b[i].is_ascii_digit() || b[i] == '.') {
        i += 1;
    }
    let s: String = b[start..i].iter().collect();
    let n: f64 = s.parse().ok()?;
    out.push(Tok::Num(n));
    Some(i)
}

// ─── parser / evaluator ──────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum Cmp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    In,
}

struct Parser<'a> {
    toks: Vec<Tok>,
    pos: usize,
    ctx: &'a TemplateCtx,
}

/// Parse + evaluate `src` as an expression. `None` ⇒ syntax error (caller
/// falls back to string truthiness).
fn eval_expr(src: &str, ctx: &TemplateCtx) -> Option<Json> {
    let toks = tokenize(src)?;
    if toks.is_empty() {
        return None;
    }
    let mut p = Parser { toks, pos: 0, ctx };
    let v = p.parse_or()?;
    if p.pos != p.toks.len() {
        return None; // trailing, unconsumed tokens ⇒ malformed
    }
    Some(v)
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn advance(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn parse_or(&mut self) -> Option<Json> {
        let mut v = self.parse_and()?;
        while matches!(self.peek(), Some(Tok::Or)) {
            self.pos += 1;
            let r = self.parse_and()?;
            v = Json::Bool(truthy(&v) || truthy(&r));
        }
        Some(v)
    }

    fn parse_and(&mut self) -> Option<Json> {
        let mut v = self.parse_cmp()?;
        while matches!(self.peek(), Some(Tok::And)) {
            self.pos += 1;
            let r = self.parse_cmp()?;
            v = Json::Bool(truthy(&v) && truthy(&r));
        }
        Some(v)
    }

    fn parse_cmp(&mut self) -> Option<Json> {
        let l = self.parse_unary()?;
        let op = match self.peek() {
            Some(Tok::EqEq) => Cmp::Eq,
            Some(Tok::NotEq) => Cmp::Ne,
            Some(Tok::Lt) => Cmp::Lt,
            Some(Tok::Le) => Cmp::Le,
            Some(Tok::Gt) => Cmp::Gt,
            Some(Tok::Ge) => Cmp::Ge,
            Some(Tok::In) => Cmp::In,
            _ => return Some(l),
        };
        self.pos += 1;
        let r = self.parse_unary()?;
        Some(Json::Bool(compare(op, &l, &r)))
    }

    fn parse_unary(&mut self) -> Option<Json> {
        if matches!(self.peek(), Some(Tok::Bang)) {
            self.pos += 1;
            let v = self.parse_unary()?;
            return Some(Json::Bool(!truthy(&v)));
        }
        self.parse_atom()
    }

    fn parse_atom(&mut self) -> Option<Json> {
        match self.advance()? {
            Tok::Num(n) => serde_json::Number::from_f64(n).map(Json::Number),
            Tok::Str(s) => Some(Json::String(s)),
            Tok::Ident(id) => Some(match id.as_str() {
                "true" => Json::Bool(true),
                "false" => Json::Bool(false),
                "null" => Json::Null,
                _ => resolve_ident(self.ctx, &id),
            }),
            Tok::LParen => {
                let v = self.parse_or()?;
                if matches!(self.advance(), Some(Tok::RParen)) {
                    Some(v)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

fn compare(op: Cmp, l: &Json, r: &Json) -> bool {
    match op {
        Cmp::Eq => json_eq(l, r),
        Cmp::Ne => !json_eq(l, r),
        Cmp::Lt | Cmp::Le | Cmp::Gt | Cmp::Ge => ordered(op, l, r),
        Cmp::In => is_in(l, r),
    }
}

/// Equality with numeric coercion (so `5 == 5.0` and an int field vs a literal
/// agree); otherwise structural JSON equality.
fn json_eq(l: &Json, r: &Json) -> bool {
    if let (Some(a), Some(b)) = (l.as_f64(), r.as_f64()) {
        return a == b;
    }
    l == r
}

/// Ordered comparison: numeric when both coerce to a number, else lexicographic
/// when both are strings; anything else is not comparable (`false`).
fn ordered(op: Cmp, l: &Json, r: &Json) -> bool {
    let ord = if let (Some(a), Some(b)) = (l.as_f64(), r.as_f64()) {
        a.partial_cmp(&b)
    } else if let (Some(a), Some(b)) = (l.as_str(), r.as_str()) {
        Some(a.cmp(b))
    } else {
        None
    };
    match ord {
        Some(o) => match op {
            Cmp::Lt => o.is_lt(),
            Cmp::Le => o.is_le(),
            Cmp::Gt => o.is_gt(),
            Cmp::Ge => o.is_ge(),
            _ => false,
        },
        None => false,
    }
}

/// Membership: element-of (array, with numeric coercion), substring (string),
/// or key-of (object).
fn is_in(l: &Json, r: &Json) -> bool {
    match r {
        Json::Array(arr) => arr.iter().any(|e| json_eq(e, l)),
        Json::String(s) => l.as_str().map(|x| s.contains(x)).unwrap_or(false),
        Json::Object(o) => l.as_str().map(|k| o.contains_key(k)).unwrap_or(false),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> TemplateCtx {
        TemplateCtx {
            vars: TemplateCtx::ns(json!({ "n": 7 })),
            ..Default::default()
        }
    }

    #[test]
    fn tokenize_handles_quoted_string_with_spaces() {
        // A space inside a quoted literal must not split the token.
        let toks = tokenize("'New York' == name").expect("tokenize");
        assert_eq!(toks[0], Tok::Str("New York".to_string()));
        assert_eq!(toks[1], Tok::EqEq);
    }

    #[test]
    fn eval_expr_returns_none_on_garbage() {
        assert!(eval_expr("a >", &ctx()).is_none());
        assert!(eval_expr("== 3", &ctx()).is_none());
        assert!(eval_expr("(1 > 2", &ctx()).is_none()); // unbalanced paren
    }

    #[test]
    fn negative_number_literal() {
        assert!(eval_predicate("vars.n > -1", &ctx()).unwrap());
        assert!(eval_predicate("-3 < -1", &ctx()).unwrap());
    }
}
