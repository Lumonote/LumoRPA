//! 语音包（人格）：结果反馈的话术模板 + TTS 音色。完全本地，不涉网络。
//! `default` 保持现行朴素文案；`lumo` 是内置的本地 Lumo AI 语音包。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PersonaMoment {
    /// 收到指令、开始执行
    Ack,
    /// 执行成功
    Success,
    /// 执行失败
    Failure,
    /// 状态播报
    Status,
    /// 续听窗口开启
    FollowUp,
}

pub(crate) struct VoicePersona {
    pub(crate) id: &'static str,
    pub(crate) display_name: &'static str,
    /// AVSpeech 音色（identifier 或语言代码，见 macos_tts voiceWithLanguage 回退）；
    /// None = 系统默认音色。
    pub(crate) tts_voice: Option<&'static str>,
    ack: &'static str,
    success: &'static str,
    failure: &'static str,
    status: &'static str,
    follow_up: &'static str,
}

impl VoicePersona {
    /// `{}` 占位符替换为 detail；模板为空则原样返回 detail。
    pub(crate) fn render(&self, moment: PersonaMoment, detail: &str) -> String {
        let template = match moment {
            PersonaMoment::Ack => self.ack,
            PersonaMoment::Success => self.success,
            PersonaMoment::Failure => self.failure,
            PersonaMoment::Status => self.status,
            PersonaMoment::FollowUp => self.follow_up,
        };
        if template.is_empty() {
            return detail.to_string();
        }
        if template.contains("{}") {
            template.replace("{}", detail)
        } else {
            template.to_string()
        }
    }
}

pub(crate) const DEFAULT_PERSONA_ID: &str = "default";
pub(crate) const LUMO_PERSONA_ID: &str = "lumo";

const PERSONAS: &[VoicePersona] = &[
    VoicePersona {
        id: DEFAULT_PERSONA_ID,
        display_name: "语音助手",
        tts_voice: None,
        ack: "{}",
        success: "{}",
        failure: "{}",
        status: "{}",
        follow_up: "",
    },
    VoicePersona {
        id: LUMO_PERSONA_ID,
        display_name: "Lumo AI",
        // 走 AVSpeechSynthesisVoice voiceWithLanguage 的中文本地音色
        tts_voice: Some("zh-CN"),
        ack: "好嘞，Lumo 这就去办：{}",
        success: "搞定啦！{}",
        failure: "哎呀，{}。换个说法再试试",
        status: "Lumo 报告：{}",
        follow_up: "Lumo 还在听，继续吩咐～",
    },
];

/// 未知 id 回落 default，保证旧配置/脏数据不致失效。
pub(crate) fn persona(id: &str) -> &'static VoicePersona {
    PERSONAS
        .iter()
        .find(|persona| persona.id == id)
        .unwrap_or(&PERSONAS[0])
}

pub(crate) fn is_known_persona(id: &str) -> bool {
    PERSONAS.iter().any(|persona| persona.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_persona_keeps_messages_verbatim() {
        let voice = persona(DEFAULT_PERSONA_ID);
        assert_eq!(voice.render(PersonaMoment::Success, "任务执行完成"), "任务执行完成");
        assert_eq!(voice.render(PersonaMoment::Failure, "任务执行失败"), "任务执行失败");
        assert_eq!(voice.render(PersonaMoment::FollowUp, ""), "");
        assert!(voice.tts_voice.is_none());
    }

    #[test]
    fn lumo_persona_styles_messages_and_uses_local_chinese_voice() {
        let voice = persona(LUMO_PERSONA_ID);
        assert_eq!(
            voice.render(PersonaMoment::Success, "已打开任务中心"),
            "搞定啦！已打开任务中心"
        );
        assert_eq!(
            voice.render(PersonaMoment::Ack, "运行流程「日报」"),
            "好嘞，Lumo 这就去办：运行流程「日报」"
        );
        assert!(voice
            .render(PersonaMoment::Failure, "没有找到流程")
            .contains("没有找到流程"));
        assert_eq!(
            voice.render(PersonaMoment::FollowUp, ""),
            "Lumo 还在听，继续吩咐～"
        );
        assert_eq!(voice.tts_voice, Some("zh-CN"));
    }

    #[test]
    fn unknown_persona_falls_back_to_default() {
        assert_eq!(persona("nope").id, DEFAULT_PERSONA_ID);
        assert!(is_known_persona(LUMO_PERSONA_ID));
        assert!(!is_known_persona("nope"));
    }
}
