//! Cross-platform contract for the macOS system TTS adapter.
//!
//! A host can supply an AVSpeechSynthesizer-backed [`SystemTtsBackend`] on
//! macOS. Keeping the backend injected lets cancellation, quiet mode and
//! response limits remain deterministic on every CI platform.

use crate::provider::{ProviderError, TtsProvider};
use async_trait::async_trait;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq)]
pub struct MacOsTtsConfig {
    pub quiet: bool,
    pub max_chars: usize,
    pub voice: Option<String>,
    pub rate: f32,
}

impl Default for MacOsTtsConfig {
    fn default() -> Self {
        Self {
            quiet: false,
            max_chars: 280,
            voice: None,
            rate: 0.5,
        }
    }
}

#[async_trait]
pub trait SystemTtsBackend: Send + Sync {
    async fn speak(
        &self,
        text: &str,
        voice: Option<&str>,
        rate: f32,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError>;

    async fn stop(&self);
}

#[cfg(not(target_os = "macos"))]
pub fn native_macos_tts_backend() -> Result<Arc<dyn SystemTtsBackend>, ProviderError> {
    Err(ProviderError::NativeUnavailable {
        backend: "AVSpeechSynthesizer".into(),
    })
}

#[cfg(target_os = "macos")]
pub fn native_macos_tts_backend() -> Result<Arc<dyn SystemTtsBackend>, ProviderError> {
    Ok(Arc::new(AvSpeechSynthesizerBackend::new()?))
}

#[cfg(target_os = "macos")]
pub struct AvSpeechSynthesizerBackend {
    active: Arc<std::sync::atomic::AtomicUsize>,
    operation: tokio::sync::Mutex<()>,
}

#[cfg(target_os = "macos")]
impl AvSpeechSynthesizerBackend {
    pub fn new() -> Result<Self, ProviderError> {
        avspeech::ensure_available()?;
        Ok(Self {
            active: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            operation: tokio::sync::Mutex::new(()),
        })
    }
}

#[cfg(target_os = "macos")]
#[async_trait]
impl SystemTtsBackend for AvSpeechSynthesizerBackend {
    async fn speak(
        &self,
        text: &str,
        voice: Option<&str>,
        rate: f32,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError> {
        let _operation = self.operation.lock().await;
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        let text = text.to_string();
        let voice = voice.map(str::to_string);
        let active = self.active.clone();
        tokio::task::spawn_blocking(move || {
            avspeech::speak(&text, voice.as_deref(), rate, &cancel, &active)
        })
        .await
        .map_err(|error| ProviderError::Other(format!("AVSpeechSynthesizer task: {error}")))?
    }

    async fn stop(&self) {
        avspeech::stop_active(&self.active);
    }
}

#[cfg(target_os = "macos")]
mod avspeech {
    use super::ProviderError;
    use std::ffi::{c_char, c_void, CString};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    type ObjcId = *mut c_void;
    type Selector = *mut c_void;

    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> ObjcId;
        fn sel_registerName(name: *const c_char) -> Selector;
        fn objc_msgSend();
    }

    #[link(name = "Foundation", kind = "framework")]
    unsafe extern "C" {}

    #[link(name = "AVFoundation", kind = "framework")]
    unsafe extern "C" {}

    unsafe fn msg_send_id(receiver: ObjcId, selector: Selector) -> ObjcId {
        let send: unsafe extern "C" fn(ObjcId, Selector) -> ObjcId =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        unsafe { send(receiver, selector) }
    }

    unsafe fn msg_send_id_id(receiver: ObjcId, selector: Selector, value: ObjcId) -> ObjcId {
        let send: unsafe extern "C" fn(ObjcId, Selector, ObjcId) -> ObjcId =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        unsafe { send(receiver, selector, value) }
    }

    unsafe fn msg_send_id_cstr(
        receiver: ObjcId,
        selector: Selector,
        value: *const c_char,
    ) -> ObjcId {
        let send: unsafe extern "C" fn(ObjcId, Selector, *const c_char) -> ObjcId =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        unsafe { send(receiver, selector, value) }
    }

    unsafe fn msg_send_void(receiver: ObjcId, selector: Selector) {
        let send: unsafe extern "C" fn(ObjcId, Selector) =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        unsafe { send(receiver, selector) }
    }

    unsafe fn msg_send_void_id(receiver: ObjcId, selector: Selector, value: ObjcId) {
        let send: unsafe extern "C" fn(ObjcId, Selector, ObjcId) =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        unsafe { send(receiver, selector, value) }
    }

    unsafe fn msg_send_void_float(receiver: ObjcId, selector: Selector, value: f32) {
        let send: unsafe extern "C" fn(ObjcId, Selector, f32) =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        unsafe { send(receiver, selector, value) }
    }

    unsafe fn msg_send_void_integer(receiver: ObjcId, selector: Selector, value: isize) {
        let send: unsafe extern "C" fn(ObjcId, Selector, isize) =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        unsafe { send(receiver, selector, value) }
    }

    unsafe fn msg_send_bool(receiver: ObjcId, selector: Selector) -> i8 {
        let send: unsafe extern "C" fn(ObjcId, Selector) -> i8 =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        unsafe { send(receiver, selector) }
    }

    pub fn ensure_available() -> Result<(), ProviderError> {
        if class("AVSpeechSynthesizer").is_null()
            || class("AVSpeechUtterance").is_null()
            || class("NSString").is_null()
        {
            return Err(ProviderError::NativeUnavailable {
                backend: "AVSpeechSynthesizer".into(),
            });
        }
        Ok(())
    }

    pub fn speak(
        text: &str,
        voice: Option<&str>,
        rate: f32,
        cancel: &CancellationToken,
        active: &AtomicUsize,
    ) -> Result<(), ProviderError> {
        ensure_available()?;
        let text = CString::new(text).map_err(|_| ProviderError::InvalidInput {
            message: "TTS text contains a NUL character".into(),
        })?;
        let voice =
            voice
                .map(CString::new)
                .transpose()
                .map_err(|_| ProviderError::InvalidInput {
                    message: "TTS voice contains a NUL character".into(),
                })?;

        // All autoreleased Objective-C objects and the synthesizer are created,
        // used and released on this dedicated blocking thread.
        let pool = unsafe { allocate("NSAutoreleasePool") };
        if pool.is_null() {
            return Err(ProviderError::NativeUnavailable {
                backend: "Foundation autorelease pool".into(),
            });
        }
        let synthesizer = unsafe { allocate("AVSpeechSynthesizer") };
        if synthesizer.is_null() {
            unsafe { msg_send_void(pool, selector("drain")) };
            return Err(ProviderError::NativeUnavailable {
                backend: "AVSpeechSynthesizer".into(),
            });
        }

        active.store(synthesizer as usize, Ordering::Release);
        let result = unsafe {
            let string = msg_send_id_cstr(
                class("NSString"),
                selector("stringWithUTF8String:"),
                text.as_ptr(),
            );
            let utterance = msg_send_id_id(
                class("AVSpeechUtterance"),
                selector("speechUtteranceWithString:"),
                string,
            );
            if utterance.is_null() {
                Err(ProviderError::Other(
                    "AVSpeechUtterance creation failed".into(),
                ))
            } else {
                msg_send_void_float(utterance, selector("setRate:"), rate.clamp(0.0, 1.0));
                if let Some(voice) = voice.as_ref() {
                    let voice_name = msg_send_id_cstr(
                        class("NSString"),
                        selector("stringWithUTF8String:"),
                        voice.as_ptr(),
                    );
                    let voice_class = class("AVSpeechSynthesisVoice");
                    let mut selected =
                        msg_send_id_id(voice_class, selector("voiceWithIdentifier:"), voice_name);
                    if selected.is_null() {
                        selected =
                            msg_send_id_id(voice_class, selector("voiceWithLanguage:"), voice_name);
                    }
                    if !selected.is_null() {
                        msg_send_void_id(utterance, selector("setVoice:"), selected);
                    }
                }
                msg_send_void_id(synthesizer, selector("speakUtterance:"), utterance);
                loop {
                    if cancel.is_cancelled() {
                        msg_send_void_integer(synthesizer, selector("stopSpeakingAtBoundary:"), 0);
                        break Err(ProviderError::Cancelled);
                    }
                    if msg_send_bool(synthesizer, selector("isSpeaking")) == 0 {
                        break Ok(());
                    }
                    std::thread::sleep(Duration::from_millis(15));
                }
            }
        };
        active.store(0, Ordering::Release);
        unsafe {
            msg_send_void(synthesizer, selector("release"));
            msg_send_void(pool, selector("drain"));
        }
        result
    }

    pub fn stop_active(active: &AtomicUsize) {
        let synthesizer = active.load(Ordering::Acquire) as ObjcId;
        if synthesizer.is_null() {
            return;
        }
        unsafe {
            msg_send_void_integer(synthesizer, selector("stopSpeakingAtBoundary:"), 0);
        }
    }

    fn class(name: &str) -> ObjcId {
        let name = CString::new(name).expect("Objective-C class names are static and NUL-free");
        unsafe { objc_getClass(name.as_ptr()) }
    }

    fn selector(name: &str) -> Selector {
        let name = CString::new(name).expect("Objective-C selectors are static and NUL-free");
        unsafe { sel_registerName(name.as_ptr()) }
    }

    unsafe fn allocate(class_name: &str) -> ObjcId {
        let allocated = unsafe { msg_send_id(class(class_name), selector("alloc")) };
        if allocated.is_null() {
            allocated
        } else {
            unsafe { msg_send_id(allocated, selector("init")) }
        }
    }
}

pub struct MacOsTtsProvider {
    backend: Arc<dyn SystemTtsBackend>,
    config: MacOsTtsConfig,
}

impl MacOsTtsProvider {
    pub fn new(backend: Arc<dyn SystemTtsBackend>, config: MacOsTtsConfig) -> Self {
        Self { backend, config }
    }
}

#[async_trait]
impl TtsProvider for MacOsTtsProvider {
    async fn speak(&self, text: &str, cancel: CancellationToken) -> Result<(), ProviderError> {
        if self.config.quiet || text.is_empty() {
            return Ok(());
        }
        let char_count = text.chars().count();
        if char_count > self.config.max_chars {
            return Err(ProviderError::InvalidInput {
                message: format!(
                    "TTS result has {char_count} characters; maximum is {}",
                    self.config.max_chars
                ),
            });
        }
        if cancel.is_cancelled() {
            self.backend.stop().await;
            return Err(ProviderError::Cancelled);
        }

        let operation_cancel = cancel.child_token();
        let operation = self.backend.speak(
            text,
            self.config.voice.as_deref(),
            self.config.rate,
            operation_cancel.clone(),
        );
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                operation_cancel.cancel();
                self.backend.stop().await;
                Err(ProviderError::Cancelled)
            }
            result = operation => result,
        }
    }
}
