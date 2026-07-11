use async_trait::async_trait;
use lumo_voice::macos_tts::{
    native_macos_tts_backend, MacOsTtsConfig, MacOsTtsProvider, SystemTtsBackend,
};
use lumo_voice::provider::{ProviderError, TtsProvider};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct FixtureTtsBackend {
    spoken: Mutex<Vec<String>>,
    stops: Mutex<usize>,
    hang: bool,
}

#[async_trait]
impl SystemTtsBackend for FixtureTtsBackend {
    async fn speak(
        &self,
        text: &str,
        _voice: Option<&str>,
        _rate: f32,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError> {
        self.spoken.lock().unwrap().push(text.to_string());
        if self.hang {
            cancel.cancelled().await;
            return Err(ProviderError::Cancelled);
        }
        Ok(())
    }

    async fn stop(&self) {
        *self.stops.lock().unwrap() += 1;
    }
}

fn config() -> MacOsTtsConfig {
    MacOsTtsConfig {
        quiet: false,
        max_chars: 12,
        voice: Some("Fixture Voice".into()),
        rate: 0.5,
    }
}

#[test]
fn unavailable_native_avspeech_backend_is_typed() {
    let err = match native_macos_tts_backend() {
        Ok(_) => panic!("AVSpeechSynthesizer is not linked in this build"),
        Err(err) => err,
    };
    assert!(matches!(
        err,
        ProviderError::NativeUnavailable { ref backend } if backend == "AVSpeechSynthesizer"
    ));
}

#[tokio::test]
async fn contract_enforces_quiet_mode_and_short_result_limit() {
    let backend = Arc::new(FixtureTtsBackend::default());
    let provider = MacOsTtsProvider::new(backend.clone(), config());
    provider
        .speak("short result", CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(backend.spoken.lock().unwrap().as_slice(), ["short result"]);

    let err = provider
        .speak("this result is too long", CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::InvalidInput { .. }));

    let mut quiet = config();
    quiet.quiet = true;
    let quiet_provider = MacOsTtsProvider::new(backend.clone(), quiet);
    quiet_provider
        .speak("not spoken", CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(backend.spoken.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn cancellation_stops_backend_and_returns_cancelled() {
    let backend = Arc::new(FixtureTtsBackend {
        hang: true,
        ..Default::default()
    });
    let provider = Arc::new(MacOsTtsProvider::new(backend.clone(), config()));
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let task_provider = provider.clone();
    let task = tokio::spawn(async move { task_provider.speak("listening", task_cancel).await });
    tokio::task::yield_now().await;
    cancel.cancel();

    assert!(matches!(task.await.unwrap(), Err(ProviderError::Cancelled)));
    assert_eq!(*backend.stops.lock().unwrap(), 1);
}
