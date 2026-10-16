//! 远程 OCR 接口引擎。
//!
//! 面向「自建 PaddleOCR / 第三方 OCR 服务」这类 HTTP 端点,契约刻意做得宽松,
//! 以便对接不同实现:
//!
//! * 请求:`POST {endpoint}`,JSON body `{"image": "<base64>", "lang": [...]}`;
//!   可选鉴权头 `Authorization: {auth_header}`。
//! * 响应:接受两种形状 —— `{"text": "..."}`(整页纯文本),或
//!   `{"lines": [{"text": "...", "confidence": 0.97, "bbox": [x0,y0,x1,y1]}]}`
//!   (带坐标与置信度,信息更全)。
//!
//! **门禁**:`ensure_network_url` 逐 URL 授权(与 `http.*` 族一致),外加上层的
//! `allow_cloud` 开关。两道都过才会真正发出请求。

use super::{OcrOptions, OcrRequest};
use base64::Engine as _;
use lumo_core::error::StepError;
use lumo_core::StepCtx;
use lumo_doc::ocr::{OcrLine, OcrPageResult};
use lumo_doc::{BBox, Source};
use serde::Deserialize;
use serde_json::json;

/// 远程返回的一行(字段全部可选,尽量兼容不同服务端)。
#[derive(Debug, Deserialize)]
struct RemoteLine {
    #[serde(default)]
    text: String,
    #[serde(default)]
    confidence: Option<f64>,
    /// `[x0, y0, x1, y1]`,左上原点。
    #[serde(default)]
    bbox: Option<Vec<f64>>,
}

#[derive(Debug, Deserialize)]
struct RemoteResponse {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    lines: Option<Vec<RemoteLine>>,
    /// 服务端报错时的说明,用于把失败原因如实透出。
    #[serde(default)]
    error: Option<String>,
}

pub async fn recognize(
    ctx: &mut StepCtx,
    options: &OcrOptions,
    request: &OcrRequest<'_>,
) -> Result<OcrPageResult, StepError> {
    let endpoint = options
        .endpoint
        .as_deref()
        .ok_or_else(|| StepError::msg("OCR remote 档需要 `ocr.endpoint`"))?;
    // 与 http.* 族同一把闸:逐 URL 授权
    ctx.ensure_network_url(endpoint)?;

    // 复用 http 族的受闸客户端 —— 重定向逐跳重审,不会被 3xx 带到未授权主机
    let client = crate::http::build_gated_client(&ctx.capabilities().network, options.timeout_ms)?;
    let payload = json!({
        "image": base64::engine::general_purpose::STANDARD.encode(request.image),
        "media_type": request.media_type,
        "lang": request.languages,
        "page": request.page,
    });

    let mut builder = client.post(endpoint).json(&payload);
    if let Some(auth) = options.auth_header.as_deref().filter(|v| !v.is_empty()) {
        builder = builder.header(reqwest::header::AUTHORIZATION, auth);
    }
    let response = builder
        .send()
        .await
        .map_err(|e| StepError::network(format!("OCR remote request failed: {e}")))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| StepError::network(format!("OCR remote read body: {e}")))?;
    if !status.is_success() {
        // 带上服务端原文,便于定位是鉴权、配额还是图片格式问题
        return Err(StepError::network(format!(
            "OCR remote returned {status}: {}",
            body.chars().take(300).collect::<String>()
        )));
    }

    let mut parsed: RemoteResponse = serde_json::from_str(&body).map_err(|e| {
        StepError::msg(format!(
            "OCR remote response is not the expected JSON shape: {e}; body={}",
            body.chars().take(200).collect::<String>()
        ))
    })?;
    if let Some(error) = parsed.error.take().filter(|e| !e.is_empty()) {
        return Err(StepError::msg(format!("OCR remote error: {error}")));
    }

    Ok(OcrPageResult {
        page: request.page,
        lines: to_lines(parsed),
        source: Source::OcrRemote,
    })
}

/// 两种响应形状归一到 `OcrLine`。带坐标的优先,纯文本按行拆。
fn to_lines(response: RemoteResponse) -> Vec<OcrLine> {
    if let Some(lines) = response.lines {
        return lines
            .into_iter()
            .filter(|line| !line.text.trim().is_empty())
            .map(|line| OcrLine {
                text: line.text.trim().to_string(),
                bbox: line
                    .bbox
                    .as_ref()
                    .and_then(|b| (b.len() == 4).then(|| BBox::new(b[0], b[1], b[2], b[3]))),
                // 服务端不给置信度时给保守标称值,不假装是 1.0
                confidence: line.confidence.unwrap_or(0.8).clamp(0.0, 1.0),
            })
            .collect();
    }
    response
        .text
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| OcrLine {
            text: line.trim().to_string(),
            bbox: None,
            confidence: 0.8,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: &str) -> Vec<OcrLine> {
        to_lines(serde_json::from_str(body).unwrap())
    }

    #[test]
    fn structured_lines_keep_bbox_and_confidence() {
        let lines =
            parse(r#"{"lines":[{"text":"合同总额","confidence":0.97,"bbox":[10,20,120,44]}]}"#);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "合同总额");
        assert!((lines[0].confidence - 0.97).abs() < 1e-9);
        let bbox = lines[0].bbox.unwrap();
        assert_eq!((bbox.x0, bbox.y1), (10.0, 44.0));
    }

    #[test]
    fn plain_text_responses_are_split_per_line() {
        let lines = parse(r#"{"text":"第一行\n\n第二行\n"}"#);
        assert_eq!(lines.len(), 2, "空行应被丢弃");
        assert_eq!(lines[1].text, "第二行");
        assert!(lines[1].bbox.is_none());
    }

    #[test]
    fn missing_confidence_gets_a_conservative_value_not_a_fake_one() {
        let lines = parse(r#"{"lines":[{"text":"x"}]}"#);
        assert!(lines[0].confidence < 1.0, "服务端没给置信度时不能假装满分");
    }

    #[test]
    fn out_of_range_confidence_is_clamped() {
        let lines = parse(r#"{"lines":[{"text":"x","confidence":9.5}]}"#);
        assert!(lines[0].confidence <= 1.0);
    }

    #[test]
    fn malformed_bbox_is_dropped_rather_than_guessed() {
        let lines = parse(r#"{"lines":[{"text":"x","bbox":[1,2]}]}"#);
        assert!(lines[0].bbox.is_none(), "长度不对的 bbox 不该被脑补");
    }
}
