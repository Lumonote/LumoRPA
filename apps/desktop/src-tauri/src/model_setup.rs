use super::{looks_like_env_var_name, ProviderInput, ProviderProfile, ProvidersConfig};

fn nonblank(value: Option<String>) -> Option<String> {
    value.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

fn endpoint(profile: &ProviderProfile) -> Result<reqwest::Url, String> {
    let default = match profile.kind.as_str() {
        "openai" => "https://api.openai.com/v1",
        "anthropic" => "https://api.anthropic.com",
        _ => return Err("此模型源不支持文本对话，请选择支持文本分析的模型源。".into()),
    };
    let raw = profile.base_url.as_deref().unwrap_or(default);
    let url = reqwest::Url::parse(raw.trim_end_matches('/')).map_err(|_| "模型服务地址无效。")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("模型服务地址应为 HTTP(S) 基础地址，不能包含凭据、查询参数或片段。".into());
    }
    Ok(url)
}

pub(super) fn chat_model(p: &ProviderProfile) -> Result<&str, String> {
    configured_model(p, false)
}

pub(super) fn configured_model(p: &ProviderProfile, vision: bool) -> Result<&str, String> {
    endpoint(p)?;
    if p.kind == "openai" && !matches!(p.wire_api.as_deref(), None | Some("chat" | "responses")) {
        return Err("不支持此请求协议，请选择 chat 或 responses。".into());
    }
    let model = (if vision {
        &p.vision_model
    } else {
        &p.default_model
    })
    .as_deref()
    .map(str::trim)
    .filter(|m| !m.is_empty())
    .ok_or(if vision {
        "请在「模型」页配置视觉模型。"
    } else {
        "请在「模型」页配置默认文本模型。"
    })?;
    if p.resolve_api_key().is_none() {
        return Err("当前模型源缺少可用的 API 密钥，请检查「模型」页中的密钥或环境变量。".into());
    }
    Ok(model)
}

// Solid 64×64 PNGs. No file, screen capture or customer data is accessed.
const COLOR_SAMPLES: [(&str,&str);3] = [
    ("red","iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAIAAAAlC+aJAAAAS0lEQVR42u3PQQkAAAgAsetfWiP4FgYrsKZeS0BAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEDgsqnc8OJg6Ln3AAAAAElFTkSuQmCC"),
    ("blue","iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAIAAAAlC+aJAAAAS0lEQVR42u3PQQkAAAgAsetfWiP4FgYrsGqeExAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBA4LMf88OL0EKXAAAAAAElFTkSuQmCC"),
    ("green","iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAIAAAAlC+aJAAAAS0lEQVR42u3PQQkAAAgAsetfWiP4FgYrsJp+ExAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBC4LLjs8OJxKlMxAAAAAElFTkSuQmCC"),
];
pub(super) fn vision_probe() -> (super::ChatMessage, &'static str) {
    let (color, png) = COLOR_SAMPLES[(ulid::Ulid::new().0 % COLOR_SAMPLES.len() as u128) as usize];
    let mut message = super::ChatMessage::text(
        super::Role::User,
        "What color fills the attached image? Reply with one English color word.",
    );
    message
        .attachments
        .push(lumo_ai::provider::ImageAttachment::base64("image/png", png));
    (message, color)
}
pub(super) fn probe_reply_valid(content: &str, expected: Option<&str>) -> bool {
    let text = content.trim();
    match expected {
        Some(color) => text.trim_end_matches('.').eq_ignore_ascii_case(color),
        None => !text.is_empty(),
    }
}

pub(super) fn from_editor(
    cfg: &ProvidersConfig,
    input: ProviderInput,
) -> Result<ProviderProfile, String> {
    let name = input.name.trim().to_owned();
    if name.is_empty() || name.contains('/') {
        return Err("模型源名称不能为空或包含 /。".into());
    }
    let api_key_env = nonblank(input.api_key_env);
    if api_key_env
        .as_deref()
        .is_some_and(|s| !looks_like_env_var_name(s))
    {
        return Err("api_key_env 只填写环境变量名；密钥请填入密码框。".into());
    }
    let mut p = ProviderProfile {
        name,
        kind: input.kind,
        wire_api: nonblank(input.wire_api),
        base_url: nonblank(input.base_url),
        api_key: nonblank(input.api_key),
        api_key_env,
        default_model: nonblank(input.default_model),
        vision_model: nonblank(input.vision_model),
        ocr_model: nonblank(input.ocr_model),
        models: input
            .models
            .into_iter()
            .filter_map(|s| nonblank(Some(s)))
            .collect(),
        headers: input.headers,
        reasoning_effort: nonblank(input.reasoning_effort),
        notes: nonblank(input.notes),
    };
    if p.api_key.is_none() && input.keep_inline_key {
        let old = cfg
            .get(&p.name)
            .ok_or("原模型源已不存在，请重新输入密钥。")?;
        if old.kind != p.kind || endpoint(old)? != endpoint(&p)? {
            return Err(
                "服务地址或模型源类型已更改。请重新输入密钥，或勾选清除已保存密钥。".into(),
            );
        }
        p.api_key = Some(
            old.api_key
                .clone()
                .ok_or("已保存密钥发生变化，请重新打开模型源。")?,
        );
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn draft() -> ProviderInput {
        serde_json::from_value(
            json!({"name":"test","kind":"openai","defaultModel":" draft-model ",
        "baseUrl":"https://example.test/v1/","keepInlineKey":true}),
        )
        .unwrap()
    }
    fn saved() -> ProvidersConfig {
        let mut initial = draft();
        initial.keep_inline_key = false;
        initial.api_key = Some("fixture".into());
        initial.default_model = Some("saved".into());
        let mut cfg = ProvidersConfig::default();
        cfg.upsert(from_editor(&cfg, initial).unwrap());
        cfg
    }

    #[test]
    fn vision_probe_targets_explicit_vision_model_and_checks_the_image_answer() {
        let mut p = from_editor(&saved(), draft()).unwrap();
        assert!(configured_model(&p, true).is_err());
        p.vision_model = Some("image-model".into());
        assert_eq!(configured_model(&p, true).unwrap(), "image-model");
        let (message, color) = vision_probe();
        assert_eq!(message.attachments.len(), 1);
        assert!(!message.attachments[0].is_url);
        assert_eq!(message.attachments[0].media_type, "image/png");
        assert!(probe_reply_valid(
            &format!(" {}. ", color.to_uppercase()),
            Some(color)
        ));
        assert!(!probe_reply_valid("pong", Some(color)));
        assert!(!probe_reply_valid(" ", None));
    }

    #[test]
    fn preview_and_save_preserve_replace_or_remove_without_mutating_original() {
        let cfg = saved();
        let preview = from_editor(&cfg, draft()).unwrap();
        assert_eq!(preview.api_key.as_deref(), Some("fixture"));
        assert_eq!(chat_model(&preview).unwrap(), "draft-model");
        assert_eq!(
            cfg.get("test").unwrap().default_model.as_deref(),
            Some("saved")
        );
        let mut replace = draft();
        replace.api_key = Some("replacement".into());
        assert_eq!(
            from_editor(&cfg, replace).unwrap().api_key.as_deref(),
            Some("replacement")
        );
        let mut clear = draft();
        clear.keep_inline_key = false;
        clear.api_key_env = Some("LUMO_MISSING_MODEL_SETUP_FIXTURE".into());
        let cleared = from_editor(&cfg, clear).unwrap();
        assert!(cleared.api_key.is_none());
        assert!(chat_model(&cleared).unwrap_err().contains("密钥"));
    }
    #[test]
    fn saved_key_is_bound_to_the_original_service_and_protocol() {
        let cfg = saved();
        for (kind, address) in [
            ("openai", "https://different.test/v1"),
            ("openai", "https://example.test/other"),
            ("anthropic", "https://example.test/v1"),
        ] {
            let mut input = draft();
            input.kind = kind.into();
            input.base_url = Some(address.into());
            assert!(from_editor(&cfg, input).is_err());
        }
    }
    #[test]
    fn preflight_rejects_ocr_missing_key_bad_endpoint_and_unknown_wire() {
        let mut p = from_editor(&saved(), draft()).unwrap();
        p.api_key = Some("  ".into());
        assert!(chat_model(&p).unwrap_err().contains("密钥"));
        p.api_key = Some("fixture".into());
        p.wire_api = Some("typo".into());
        assert!(chat_model(&p).unwrap_err().contains("协议"));
        p.wire_api = None;
        p.base_url = Some("not a url".into());
        assert!(chat_model(&p).unwrap_err().contains("地址"));
        p.kind = "local".into();
        assert!(chat_model(&p).unwrap_err().contains("文本"));
    }
}
