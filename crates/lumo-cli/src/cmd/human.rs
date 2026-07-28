//! Human-in-the-loop 的 CLI 宿主实现（P1）。
//!
//! TTY 上以 stderr 提示 + stdin 行读回答 `human.*` 提示：input 取整行原文
//! （空行回落 `default`），confirm / approve 解析 y/n（空行回落 `default`，
//! 非法输入重新提问）。非 TTY（管道 / CI / cron）立刻报错 —— 没有人在场就
//! 不该假装等待。读取走 `tokio::io::stdin()`（内部 spawn_blocking），prompt
//! future 被动作侧 `select!` drop（取消 / 超时）时等待立即结束，残留的后台
//! 读线程只会吞掉下一行输入，不阻塞运行收尾。
//!
//! 提示打到 stderr：stdout 留给 run 报告 / outputs，管道场景不被污染。

use async_trait::async_trait;
use lumo_core::{HumanPromptKind, HumanPromptRequest, HumanPrompter, HumanResponse, StepError};
use serde_json::Value;
use std::io::{IsTerminal, Write};
use tokio::io::{AsyncBufReadExt, BufReader, Stdin};

pub struct CliPrompter {
    /// 复用同一个 BufReader：多次提示间不丢缓冲行。tokio Mutex 串行化并发
    /// 提示（parallel 分支同时问两个问题时逐个来，避免行交错）。
    stdin: tokio::sync::Mutex<BufReader<Stdin>>,
}

impl Default for CliPrompter {
    fn default() -> Self {
        Self::new()
    }
}

impl CliPrompter {
    pub fn new() -> Self {
        Self {
            stdin: tokio::sync::Mutex::new(BufReader::new(tokio::io::stdin())),
        }
    }
}

/// y/n 回答解析：空行回落 `default`，非法输入返回 `None`（调用方重新提问）。
fn parse_yes_no(answer: &str, default: Option<bool>) -> Option<bool> {
    match answer.trim().to_ascii_lowercase().as_str() {
        "" => default,
        "y" | "yes" => Some(true),
        "n" | "no" => Some(false),
        _ => None,
    }
}

/// 提示行：`? message` + kind 相关的答案提示。
fn render_prompt(req: &HumanPromptRequest) -> String {
    match req.kind {
        HumanPromptKind::Input => match req.default.as_ref().and_then(Value::as_str) {
            Some(d) if !d.is_empty() => format!("? {} (default: {d}): ", req.message),
            _ => format!("? {}: ", req.message),
        },
        HumanPromptKind::Confirm | HumanPromptKind::Approve => {
            let hint = match req.default.as_ref().and_then(Value::as_bool) {
                Some(true) => "[Y/n]",
                Some(false) => "[y/N]",
                None => "[y/n]",
            };
            let label = if req.kind == HumanPromptKind::Approve {
                "approve "
            } else {
                ""
            };
            format!("? {label}{} {hint}: ", req.message)
        }
    }
}

#[async_trait]
impl HumanPrompter for CliPrompter {
    async fn prompt(&self, req: HumanPromptRequest) -> Result<HumanResponse, StepError> {
        if !std::io::stdin().is_terminal() {
            return Err(StepError::msg(
                "human interaction requires an interactive terminal \
                 (stdin is not a TTY); run from a shell or remove the human.* step",
            ));
        }
        let mut stdin = self.stdin.lock().await;
        loop {
            eprint!("{}", render_prompt(&req));
            let _ = std::io::stderr().flush();
            let mut line = String::new();
            let n = stdin
                .read_line(&mut line)
                .await
                .map_err(|e| StepError::msg(format!("read stdin: {e}")))?;
            if n == 0 {
                return Err(StepError::msg(
                    "stdin closed (EOF) while waiting for human response",
                ));
            }
            let answer = line.trim();
            match req.kind {
                HumanPromptKind::Input => {
                    let value = if answer.is_empty() {
                        req.default
                            .as_ref()
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string()
                    } else {
                        answer.to_string()
                    };
                    return Ok(HumanResponse {
                        value: Value::String(value),
                        by: None,
                        comment: None,
                    });
                }
                HumanPromptKind::Confirm | HumanPromptKind::Approve => {
                    let default = req.default.as_ref().and_then(Value::as_bool);
                    match parse_yes_no(answer, default) {
                        Some(b) => {
                            return Ok(HumanResponse {
                                value: Value::Bool(b),
                                by: None,
                                comment: None,
                            })
                        }
                        None => {
                            eprintln!("  ! please answer y or n");
                            continue;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! 只测纯函数：交互路径依赖真实 TTY，`prompt()` 在测试进程里要么因
    //! 非 TTY 报错、要么（交互式跑 cargo test 时）真的等键盘 —— 都不适合
    //! 自动化断言，TTY 门禁语义由 docs 的宿主支持矩阵约定。
    use super::*;

    #[test]
    fn parse_yes_no_accepts_variants_and_falls_back_to_default() {
        assert_eq!(parse_yes_no("y", None), Some(true));
        assert_eq!(parse_yes_no("YES", None), Some(true));
        assert_eq!(parse_yes_no(" n ", None), Some(false));
        assert_eq!(parse_yes_no("no", None), Some(false));
        assert_eq!(parse_yes_no("", Some(true)), Some(true));
        assert_eq!(parse_yes_no("", None), None);
        assert_eq!(parse_yes_no("maybe", Some(true)), None);
    }

    #[test]
    fn render_prompt_shows_default_hints() {
        let req = |kind, default| HumanPromptRequest {
            kind,
            message: "继续?".into(),
            default,
            timeout_ms: 1000,
            run_id: "r".into(),
            step_path: "s".into(),
        };
        assert!(
            render_prompt(&req(HumanPromptKind::Confirm, Some(Value::Bool(true))))
                .contains("[Y/n]")
        );
        assert!(
            render_prompt(&req(HumanPromptKind::Confirm, Some(Value::Bool(false))))
                .contains("[y/N]")
        );
        assert!(render_prompt(&req(HumanPromptKind::Approve, None)).contains("approve"));
        assert!(render_prompt(&req(
            HumanPromptKind::Input,
            Some(Value::String("bob".into()))
        ))
        .contains("default: bob"));
    }
}
