use lumo_voice::{VoiceController, VoiceEvent as E, VoiceState as S};

#[test]
fn valid_pipeline_and_recovery() {
    let mut c = VoiceController::new(true);
    for (event, state) in [
        (E::Wake, S::WakeDetected),
        (E::StartListening, S::Listening),
        (E::AudioEnded, S::Transcribing),
        (E::TranscriptReady, S::Routing),
        (E::RouteReady, S::Planning),
        (E::ConfirmationRequired, S::Confirming),
        (E::Confirmed, S::Executing),
        (E::ExecutionFinished, S::Reporting),
        (E::ReportFinished, S::Idle),
    ] {
        assert_eq!(c.transition(event).unwrap(), state);
    }
}
#[test]
fn invalid_transition_does_not_mutate() {
    let mut c = VoiceController::new(true);
    assert!(c.transition(E::Confirmed).is_err());
    assert_eq!(c.state(), S::Idle);
}
#[test]
fn duplicate_wake_is_suppressed() {
    let mut c = VoiceController::new(true);
    c.transition(E::Wake).unwrap();
    assert_eq!(c.transition(E::Wake).unwrap(), S::WakeDetected);
}
#[test]
fn disabled_refuses_wake() {
    let mut c = VoiceController::new(false);
    assert!(c.transition(E::Wake).is_err());
}
#[test]
fn cancel_all_active_states() {
    let states = [
        S::WakeDetected,
        S::Listening,
        S::Transcribing,
        S::Routing,
        S::Planning,
        S::Confirming,
        S::Executing,
        S::Reporting,
    ];
    for state in states {
        let mut c = VoiceController::new(true);
        let path = [
            E::Wake,
            E::StartListening,
            E::AudioEnded,
            E::TranscriptReady,
            E::RouteReady,
            E::ConfirmationRequired,
            E::Confirmed,
            E::ExecutionFinished,
        ];
        for event in path {
            if c.state() == state {
                break;
            }
            c.transition(event).unwrap();
        }
        assert_eq!(c.state(), state);
        assert_eq!(c.transition(E::Cancel).unwrap(), S::Idle);
    }
}
#[test]
fn error_recovers() {
    let mut c = VoiceController::new(true);
    c.transition(E::Wake).unwrap();
    assert_eq!(c.transition(E::Fail).unwrap(), S::Error);
    assert_eq!(c.transition(E::Recover).unwrap(), S::Idle);
}
