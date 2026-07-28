use lumo_core::StepEvent;
use serde_json::Value;

pub(crate) fn payload(event: &StepEvent) -> Value {
    match event {
        StepEvent::StepStarted {
            run_id,
            path,
            step_id,
            action,
        } => serde_json::json!({
            "type": "step_started", "runId": run_id, "path": path,
            "stepId": step_id, "action": action,
        }),
        StepEvent::StepFinished {
            run_id,
            path,
            step_id,
            state,
            attempt,
            error,
        } => serde_json::json!({
            "type": "step_finished", "runId": run_id, "path": path,
            "stepId": step_id, "state": state, "attempt": attempt, "error": error,
        }),
        StepEvent::Log {
            run_id,
            step_path,
            line,
        } => serde_json::json!({
            "type": "log", "runId": run_id, "stepPath": step_path, "line": line,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_events_map_to_stable_camel_case_payloads() {
        let payload = payload(&StepEvent::StepStarted {
            run_id: "run-1".into(),
            path: "group/click".into(),
            step_id: "click".into(),
            action: "browser.click".into(),
        });
        assert_eq!(payload["type"], "step_started");
        assert_eq!(payload["runId"], "run-1");
        assert_eq!(payload["stepId"], "click");
        assert_eq!(payload["action"], "browser.click");
    }
}
