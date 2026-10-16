//! OCR 引擎与路由。
//!
//! 三类引擎,按「能不用就不用、要用先用本地、出网必须显式授权」的顺序编排:
//!
//! | 档 | 引擎 | 出网 | 门禁 |
//! | --- | --- | --- | --- |
//! | 0 | 文本层 | 否 | —— (`lumo-doc` 直接解析,根本不进本模块) |
//! | 1 | `native` 本地小模型 | 否 | 无(模型在 `$LUMO_HOME`,非用户路径) |
//! | 2 | `remote` 远程接口 | **是** | `network` + `allow_cloud` |
//! | 3 | `vlm` 云视觉模型 | **是** | `llm` + `allow_cloud` |
//!
//! `allow_cloud` 默认 `false`,且与能力门禁**双重**把关:光声明了 `network`/`llm`
//! 不足以让文档内容出网,必须两个开关同时打开。文档解析的输入常是合同、财报、
//! 证件,隐式外发的风险不可接受 —— 这比现有 `image.ocr`(有 `llm` 就发)更严。

pub mod native;
pub mod remote;

use lumo_core::error::StepError;
use lumo_core::StepCtx;
use lumo_doc::ocr::{OcrLine, OcrPageResult};
use lumo_doc::Source;
use schemars::JsonSchema;
use serde::Deserialize;

/// 一次识别请求。`image` 是编码后的图像字节(PDF 里抽出的 JPEG、或用户给的图片)。
pub struct OcrRequest<'a> {
    pub image: &'a [u8],
    pub media_type: &'a str,
    pub page: usize,
    pub languages: &'a [String],
}

/// 引擎选择。`auto` = 按可用性自动降级,是默认值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    #[default]
    Auto,
    Native,
    Remote,
    Vlm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// 完全不做 OCR。
    Off,
    /// 只对没有文本层的页做 OCR。
    #[default]
    Auto,
    /// 所有页都做(即使已有文本层,结果也不会覆盖文本层,只用于比对)。
    Force,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OcrOptions {
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub engine: Engine,
    /// **默认 false**:不把文档内容发往任何远端。与 `network`/`llm` 能力共同
    /// 构成双重门禁。
    #[serde(default)]
    pub allow_cloud: bool,
    /// 远程 OCR 接口地址(`engine: remote` 时必填)。
    #[serde(default)]
    pub endpoint: Option<String>,
    /// 远程接口鉴权头的值(建议走 Vault 引用而非明文)。
    #[serde(default)]
    pub auth_header: Option<String>,
    /// VLM 模型名(留空用 provider 默认)。
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default = "default_languages")]
    pub lang: Vec<String>,
    /// 低于此置信度的块标记为待复核(写进 diagnostics)。
    #[serde(default = "default_min_confidence")]
    pub min_confidence: f64,
    /// 单页 OCR 超时。
    #[serde(default = "default_ocr_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_languages() -> Vec<String> {
    vec!["zh".to_string(), "en".to_string()]
}
fn default_min_confidence() -> f64 {
    0.6
}
fn default_ocr_timeout_ms() -> u64 {
    30_000
}

impl Default for OcrOptions {
    fn default() -> Self {
        Self {
            mode: Mode::default(),
            engine: Engine::default(),
            allow_cloud: false,
            endpoint: None,
            auth_header: None,
            model: None,
            lang: default_languages(),
            min_confidence: default_min_confidence(),
            timeout_ms: default_ocr_timeout_ms(),
        }
    }
}

/// 解析出的实际执行档位。`Auto` 在这里被解成具体引擎,决策过程可单测。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    Native,
    Remote,
    Vlm,
    /// 不可用及原因 —— 必须能说清为什么,不能静默返回空结果。
    Unavailable(String),
}

/// 引擎选择的**纯决策**:不碰网络、不加载模型,只根据配置与可用性推导。
/// 抽成纯函数是为了让「云端默认关闭」这条安全规则可以被测试钉死。
pub fn resolve_engine(
    options: &OcrOptions,
    native_available: bool,
    has_provider: bool,
) -> Resolved {
    match options.engine {
        Engine::Native => {
            if native_available {
                Resolved::Native
            } else {
                Resolved::Unavailable(
                    "本地 OCR 不可用:未启用 `ocr-native` 特性或模型/运行时缺失".into(),
                )
            }
        }
        Engine::Remote => {
            if !options.allow_cloud {
                return Resolved::Unavailable(
                    "远程 OCR 会把文档内容发出本机,需显式设置 `ocr.allow_cloud: true`".into(),
                );
            }
            match &options.endpoint {
                Some(url) if !url.trim().is_empty() => Resolved::Remote,
                _ => Resolved::Unavailable("`engine: remote` 需要同时给出 `ocr.endpoint`".into()),
            }
        }
        Engine::Vlm => {
            if !options.allow_cloud {
                return Resolved::Unavailable(
                    "云端 OCR 会把文档内容发出本机,需显式设置 `ocr.allow_cloud: true`".into(),
                );
            }
            if !has_provider {
                return Resolved::Unavailable("`engine: vlm` 需要已配置 AI provider".into());
            }
            Resolved::Vlm
        }
        // auto:本地优先;只有在明确允许出网时才考虑远端
        Engine::Auto => {
            if native_available {
                return Resolved::Native;
            }
            if options.allow_cloud {
                if options
                    .endpoint
                    .as_deref()
                    .is_some_and(|u| !u.trim().is_empty())
                {
                    return Resolved::Remote;
                }
                if has_provider {
                    return Resolved::Vlm;
                }
            }
            Resolved::Unavailable(
                "没有可用的 OCR 引擎:本地模型未安装,且未允许出网(`ocr.allow_cloud`)".into(),
            )
        }
    }
}

/// 按解析出的档位执行一次识别。能力门禁在各引擎内部再过一遍 —— 决策函数只管
/// 「选谁」,授权由真正发起 IO 的地方负责。
pub async fn recognize(
    ctx: &mut StepCtx,
    resolved: &Resolved,
    options: &OcrOptions,
    request: OcrRequest<'_>,
) -> Result<OcrPageResult, StepError> {
    match resolved {
        Resolved::Native => native::recognize(options, &request),
        Resolved::Remote => remote::recognize(ctx, options, &request).await,
        Resolved::Vlm => recognize_vlm(ctx, options, &request).await,
        Resolved::Unavailable(reason) => Err(StepError::msg(format!("OCR 不可用:{reason}"))),
    }
}

/// 云视觉模型:复用既有 `image.ocr` 的 AI provider 通路。
async fn recognize_vlm(
    ctx: &mut StepCtx,
    options: &OcrOptions,
    request: &OcrRequest<'_>,
) -> Result<OcrPageResult, StepError> {
    ctx.ensure_llm(options.model.as_deref().unwrap_or(""))?;
    let provider = ctx
        .ai_provider()
        .cloned()
        .ok_or_else(|| StepError::msg("OCR vlm 档需要已配置 AI provider"))?;
    let prompt = format!(
        "Extract all text from this document image in reading order. \
         Languages: {}. Return plain text only, preserving line breaks.",
        options.lang.join(", ")
    );
    let text = provider
        .ocr_image(
            bytes::Bytes::copy_from_slice(request.image),
            request.media_type,
            &prompt,
            options.model.as_deref(),
        )
        .await?;
    Ok(OcrPageResult {
        page: request.page,
        lines: text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| OcrLine {
                text: line.to_string(),
                bbox: None,
                // VLM 不回置信度,给一个保守的标称值,并如实标注来源
                confidence: 0.75,
            })
            .collect(),
        source: Source::OcrVlm,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(engine: Engine) -> OcrOptions {
        OcrOptions {
            engine,
            ..Default::default()
        }
    }

    #[test]
    fn cloud_engines_stay_shut_until_allow_cloud_is_set() {
        // 这是本模块最重要的一条规则:光有 network/llm 能力不足以让内容出网
        for engine in [Engine::Remote, Engine::Vlm] {
            let mut opts = options(engine);
            opts.endpoint = Some("https://ocr.example.com/v1".into());
            let resolved = resolve_engine(&opts, false, true);
            match resolved {
                Resolved::Unavailable(reason) => {
                    assert!(reason.contains("allow_cloud"), "reason={reason}")
                }
                other => panic!("{engine:?} 在 allow_cloud=false 时不该可用: {other:?}"),
            }
        }
    }

    #[test]
    fn auto_prefers_the_local_model_even_when_cloud_is_allowed() {
        let mut opts = options(Engine::Auto);
        opts.allow_cloud = true;
        opts.endpoint = Some("https://ocr.example.com/v1".into());
        assert_eq!(resolve_engine(&opts, true, true), Resolved::Native);
    }

    #[test]
    fn auto_falls_back_to_remote_then_vlm_only_with_permission() {
        let mut opts = options(Engine::Auto);
        opts.allow_cloud = true;
        opts.endpoint = Some("https://ocr.example.com/v1".into());
        assert_eq!(resolve_engine(&opts, false, true), Resolved::Remote);

        opts.endpoint = None;
        assert_eq!(resolve_engine(&opts, false, true), Resolved::Vlm);

        // 没有 provider 也没有 endpoint → 说清楚为什么不可用
        assert!(matches!(
            resolve_engine(&opts, false, false),
            Resolved::Unavailable(_)
        ));
    }

    #[test]
    fn auto_without_permission_reports_why_instead_of_silently_doing_nothing() {
        let opts = options(Engine::Auto); // allow_cloud 默认 false
        match resolve_engine(&opts, false, true) {
            Resolved::Unavailable(reason) => {
                assert!(reason.contains("本地模型未安装"), "reason={reason}");
                assert!(reason.contains("allow_cloud"), "reason={reason}");
            }
            other => panic!("不该可用: {other:?}"),
        }
    }

    #[test]
    fn remote_requires_an_endpoint() {
        let mut opts = options(Engine::Remote);
        opts.allow_cloud = true;
        match resolve_engine(&opts, false, true) {
            Resolved::Unavailable(reason) => assert!(reason.contains("endpoint"), "{reason}"),
            other => panic!("缺 endpoint 不该可用: {other:?}"),
        }
    }

    #[test]
    fn explicit_native_does_not_silently_fall_back_to_the_cloud() {
        // 明确要本地却不可用时,必须报错 —— 静默改走云端等于绕过用户的隐私意图
        let mut opts = options(Engine::Native);
        opts.allow_cloud = true;
        opts.endpoint = Some("https://ocr.example.com/v1".into());
        assert!(matches!(
            resolve_engine(&opts, false, true),
            Resolved::Unavailable(_)
        ));
    }

    #[test]
    fn defaults_are_conservative() {
        let opts = OcrOptions::default();
        assert_eq!(opts.mode, Mode::Auto);
        assert_eq!(opts.engine, Engine::Auto);
        assert!(!opts.allow_cloud, "默认必须不允许内容出网");
    }
}
