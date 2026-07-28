//! 跨平台系统 TTS：macOS 走 AVSpeech（macos_tts），Windows 走 PowerShell
//! System.Speech，Linux 走 spd-say（回退 espeak）。命令/脚本构造为纯函数，
//! 任意平台均可单测；进程执行支持取消（kill）。

use crate::provider::ProviderError;
use tokio_util::sync::CancellationToken;

/// 统一 TTS 选项。rate 取 0.0..=1.0，0.5 为各平台默认语速。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SystemTtsOptions {
    /// 音色：macOS 为 AVSpeech identifier 或语言代码（如 `zh-CN`）；
    /// Windows 为已安装音色名；Linux 为 spd-say/espeak 语言代码。
    pub voice: Option<String>,
    pub rate: Option<f32>,
}

/// Windows System.Speech Rate（-10..=10）。
pub fn windows_rate(rate: Option<f32>) -> i32 {
    (((rate.unwrap_or(0.5) - 0.5) * 20.0).round() as i32).clamp(-10, 10)
}

/// PowerShell 单引号转义 + 脚本构造。
pub fn windows_tts_script(text: &str, options: &SystemTtsOptions) -> String {
    let escape = |value: &str| value.replace('\'', "''");
    let mut script = String::from(
        "Add-Type -AssemblyName System.Speech; \
         $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; ",
    );
    if let Some(voice) = options.voice.as_deref().filter(|voice| !voice.is_empty()) {
        script.push_str(&format!(
            "try {{ $s.SelectVoice('{}') }} catch {{ }}; ",
            escape(voice)
        ));
    }
    script.push_str(&format!("$s.Rate = {}; ", windows_rate(options.rate)));
    script.push_str(&format!("$s.Speak('{}');", escape(text)));
    script
}

/// espeak 语速（words per minute，80..=320）。
pub fn espeak_wpm(rate: Option<f32>) -> u32 {
    (80.0 + rate.unwrap_or(0.5).clamp(0.0, 1.0) * 240.0).round() as u32
}

pub fn espeak_args(text: &str, options: &SystemTtsOptions) -> Vec<String> {
    let mut args = vec!["-s".into(), espeak_wpm(options.rate).to_string()];
    if let Some(voice) = options.voice.as_deref().filter(|voice| !voice.is_empty()) {
        args.push("-v".into());
        args.push(voice.into());
    }
    args.push(text.into());
    args
}

/// spd-say 语速（-100..=100）。
pub fn spd_rate(rate: Option<f32>) -> i32 {
    (((rate.unwrap_or(0.5) - 0.5) * 200.0).round() as i32).clamp(-100, 100)
}

pub fn spd_say_args(text: &str, options: &SystemTtsOptions) -> Vec<String> {
    let mut args = vec!["-w".into(), "-r".into(), spd_rate(options.rate).to_string()];
    if let Some(voice) = options.voice.as_deref().filter(|voice| !voice.is_empty()) {
        args.push("-l".into());
        args.push(voice.into());
    }
    args.push(text.into());
    args
}

#[cfg(not(target_os = "macos"))]
async fn run_tts_process(
    program: &str,
    args: &[String],
    cancel: CancellationToken,
) -> Result<(), ProviderError> {
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| {
            ProviderError::Other(format!("failed to launch system TTS `{program}`: {error}"))
        })?;
    tokio::select! {
        _ = cancel.cancelled() => {
            let _ = child.kill().await;
            Err(ProviderError::Cancelled)
        }
        status = child.wait() => match status {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(ProviderError::Other(format!(
                "system TTS `{program}` exited with {status}"
            ))),
            Err(error) => Err(ProviderError::Other(format!(
                "system TTS `{program}` failed: {error}"
            ))),
        },
    }
}

/// 播报一段文本；quiet/截断等策略由调用方决定。
pub async fn speak(
    text: &str,
    options: &SystemTtsOptions,
    cancel: CancellationToken,
) -> Result<(), ProviderError> {
    if text.trim().is_empty() {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        use crate::provider::TtsProvider;
        let backend = crate::macos_tts::native_macos_tts_backend()?;
        let provider = crate::macos_tts::MacOsTtsProvider::new(
            backend,
            crate::macos_tts::MacOsTtsConfig {
                voice: options.voice.clone(),
                rate: options.rate.unwrap_or(0.5),
                ..crate::macos_tts::MacOsTtsConfig::default()
            },
        );
        provider.speak(text, cancel).await
    }
    #[cfg(target_os = "windows")]
    {
        let script = windows_tts_script(text, options);
        run_tts_process(
            "powershell",
            &[
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                script,
            ],
            cancel,
        )
        .await
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let spd_args = spd_say_args(text, options);
        match run_tts_process("spd-say", &spd_args, cancel.clone()).await {
            Err(ProviderError::Other(message)) if message.contains("failed to launch") => {
                run_tts_process("espeak", &espeak_args(text, options), cancel).await
            }
            result => result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_script_escapes_quotes_and_maps_rate() {
        let options = SystemTtsOptions {
            voice: Some("Microsoft Huihui".into()),
            rate: Some(0.75),
        };
        let script = windows_tts_script("it's done", &options);
        assert!(script.contains("$s.Speak('it''s done');"));
        assert!(script.contains("SelectVoice('Microsoft Huihui')"));
        assert!(script.contains("$s.Rate = 5;"));
        assert_eq!(windows_rate(Some(0.0)), -10);
        assert_eq!(windows_rate(None), 0);
        assert_eq!(windows_rate(Some(1.0)), 10);
    }

    #[test]
    fn linux_args_map_rate_and_optional_voice() {
        let options = SystemTtsOptions {
            voice: Some("zh".into()),
            rate: Some(0.5),
        };
        assert_eq!(
            spd_say_args("你好", &options),
            vec!["-w", "-r", "0", "-l", "zh", "你好"]
        );
        assert_eq!(espeak_wpm(None), 200);
        assert_eq!(espeak_wpm(Some(0.0)), 80);
        let no_voice = SystemTtsOptions::default();
        assert_eq!(espeak_args("hi", &no_voice), vec!["-s", "200", "hi"]);
    }
}
