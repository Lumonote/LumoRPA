//! LumoRPA 文档解析引擎 —— 把 DOCX / XLSX / PDF / 图片解析为统一的 [`Document`]
//! (DocIR)。
//!
//! **本 crate 不碰文件系统、不做能力门禁**:入口一律吃 `&[u8]`,产物里的图片以
//! 内存字节返回。落盘与 `fs.read`/`fs.write`/`llm` 门禁由动作层
//! (`lumo-actions/src/doc.rs`)负责。这样解析逻辑可以脱离 `StepCtx` 直接单测,
//! 也便于未来 `lumo-agent` 做 RAG 时直接复用。
//!
//! 设计文档:`.omc/plans/doc-parsing-skill.md`
//!
//! 里程碑状态:M1 覆盖 DOCX / XLSX,M2 覆盖 PDF 文本层(版面重建 + 有线表格)。
//! 图片 OCR 与扫描件 PDF(M3)目前返回 [`DocError::Unsupported`] 或在
//! `diagnostics` 里点名,**不静默返回空结果**。

pub mod docx;
pub mod error;
pub mod ir;
pub mod markdown;
pub mod ocr;
pub mod pdf;
pub mod xlsx;

pub use error::DocError;
pub use ir::*;
pub use markdown::to_markdown;

use sha2::{Digest, Sha256};

/// 识别出的容器格式。按**魔数**判定而非扩展名 —— RPA 的输入常来自下载/邮件
/// 附件,扩展名不可信。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Format {
    Docx,
    Xlsx,
    Pdf,
    Image(&'static str),
    Unknown,
}

impl Format {
    pub fn as_str(&self) -> &str {
        match self {
            Format::Docx => "docx",
            Format::Xlsx => "xlsx",
            Format::Pdf => "pdf",
            Format::Image(kind) => kind,
            Format::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParseOptions {
    /// 收集内嵌图片字节(动作层再按 `fs.write` 门禁落盘)。
    pub collect_images: bool,
    /// 块数上界 —— 契约要求集合动作有界。
    pub limit: usize,
    /// 计算源文件 sha256(大文件有成本,默认开,便于产物溯源)。
    pub hash_source: bool,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self {
            collect_images: false,
            limit: 1000,
            hash_source: true,
        }
    }
}

#[derive(Debug)]
pub struct Parsed {
    pub document: Document,
    /// (image id, 原始字节)
    pub media: Vec<(String, Vec<u8>)>,
}

/// 按魔数嗅探格式。zip 系(DOCX/XLSX)需要进一步看内部条目才能区分。
pub fn sniff(bytes: &[u8]) -> Format {
    if bytes.starts_with(b"%PDF-") {
        return Format::Pdf;
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Format::Image("png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Format::Image("jpeg");
    }
    if bytes.starts_with(b"BM") {
        return Format::Image("bmp");
    }
    if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        return Format::Image("tiff");
    }
    // OOXML 都是 zip:靠内部条目区分 Word / Excel
    if bytes.starts_with(b"PK\x03\x04") {
        if let Ok(zip) = zip::ZipArchive::new(std::io::Cursor::new(bytes)) {
            let names: Vec<&str> = zip.file_names().collect();
            if names.iter().any(|n| n.starts_with("word/")) {
                return Format::Docx;
            }
            if names.iter().any(|n| n.starts_with("xl/")) {
                return Format::Xlsx;
            }
        }
        return Format::Unknown;
    }
    // 老 Office 二进制(CFB/OLE2):.doc / .xls
    if bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
        return Format::Unknown;
    }
    Format::Unknown
}

/// 主入口:字节 → DocIR。`path` 只用于产物里的溯源信息,不做任何 IO。
pub fn parse_bytes(bytes: &[u8], path: &str, opts: &ParseOptions) -> Result<Parsed, DocError> {
    let format = sniff(bytes);
    let source = SourceInfo {
        path: path.to_string(),
        format: format.as_str().to_string(),
        bytes: bytes.len() as u64,
        sha256: opts.hash_source.then(|| {
            let mut hasher = Sha256::new();
            hasher.update(bytes);
            format!("{:x}", hasher.finalize())
        }),
    };

    let mut parsed = match format {
        Format::Docx => {
            let out = docx::parse(bytes, source, opts.collect_images)?;
            Parsed {
                document: out.document,
                media: out.media,
            }
        }
        Format::Xlsx => Parsed {
            document: xlsx::parse(bytes, source)?,
            media: Vec::new(),
        },
        Format::Pdf => {
            let out = pdf::parse(bytes, source, opts.limit, opts.collect_images)?;
            Parsed {
                document: out.document,
                media: out.media,
            }
        }
        Format::Image(kind) => {
            return Err(DocError::unsupported(format!(
                "图片({kind})解析需要 OCR,在 M3 交付;当前可用 `image.ocr`(需 AI provider 与 llm 能力)"
            )))
        }
        Format::Unknown => {
            return Err(DocError::unsupported(
                "无法识别的格式。老版 .doc/.xls(OLE2 二进制)不受支持,请先用 LibreOffice 转为 .docx/.xlsx",
            ))
        }
    };

    parsed.document.apply_limit(opts.limit);
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_numbers_beat_file_extensions() {
        assert_eq!(sniff(b"%PDF-1.7\n..."), Format::Pdf);
        assert_eq!(sniff(&[0x89, b'P', b'N', b'G', 0x0D]), Format::Image("png"));
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Format::Image("jpeg"));
        assert_eq!(sniff(b"random bytes"), Format::Unknown);
    }

    #[test]
    fn legacy_ole2_is_detected_as_unknown_not_misparsed() {
        // .doc/.xls 的 CFB 头。必须落到 Unknown 分支给出转换建议,
        // 而不是被当成别的格式硬解出垃圾。
        assert_eq!(
            sniff(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1]),
            Format::Unknown
        );
    }

    #[test]
    fn unsupported_formats_explain_the_alternative_path() {
        // 图片仍待 M3 —— 错误必须指出当前能用什么
        let err = parse_bytes(
            &[0xFF, 0xD8, 0xFF, 0xE0],
            "/tmp/a.jpg",
            &ParseOptions::default(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("image.ocr"), "got: {err}");

        // 老二进制格式要给出转换建议,而不是硬解出垃圾
        let err = parse_bytes(
            &[0xD0, 0xCF, 0x11, 0xE0],
            "/tmp/a.doc",
            &ParseOptions::default(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("LibreOffice"), "got: {err}");
    }

    #[test]
    fn a_malformed_pdf_fails_loudly_instead_of_returning_an_empty_document() {
        let err = parse_bytes(
            b"%PDF-1.7\nnot really a pdf",
            "/tmp/a.pdf",
            &ParseOptions::default(),
        )
        .unwrap_err();
        assert!(
            matches!(err, DocError::Container { .. } | DocError::Malformed { .. }),
            "got: {err:?}"
        );
    }
}
