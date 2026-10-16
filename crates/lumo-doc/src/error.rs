//! 解析错误。分类对齐动作层的 retry kind:`Io` → `io`(不重试)、
//! `Unsupported` → `io` + 明确 message、`Encrypted` → 需要 password。
//!
//! **不静默降级**:不支持的编码/格式一律显式报错或进 `diagnostics.warnings`,
//! 绝不返回空文本假装成功 —— 静默的空结果在 RPA 里比失败更危险。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum DocError {
    #[error("read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("open {format} container: {message}")]
    Container { format: String, message: String },

    #[error("malformed {part}: {message}")]
    Malformed { part: String, message: String },

    /// 已识别格式但本版不支持(如 `.doc` 二进制、JBIG2 编码的扫描页)。
    /// message 里必须写明**替代路径**,不能只说"不支持"。
    #[error("unsupported: {message}")]
    Unsupported { message: String },

    #[error("document is encrypted; supply `password`")]
    Encrypted,

    /// 解析炸弹防护触发(zip 解压比、对象递归深度、页数上限)。
    #[error("resource limit exceeded: {message}")]
    LimitExceeded { message: String },
}

impl DocError {
    pub fn malformed(part: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Malformed {
            part: part.into(),
            message: message.into(),
        }
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::Unsupported {
            message: message.into(),
        }
    }

    pub fn container(format: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Container {
            format: format.into(),
            message: message.into(),
        }
    }
}
