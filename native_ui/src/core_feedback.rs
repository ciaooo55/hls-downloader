use hls_client::{CoreEvent, CorePipeResponse, EventEnvelope};

pub fn response_error(response: &CorePipeResponse) -> Option<&str> {
    match response {
        CorePipeResponse::Error { message, .. } => Some(message),
        CorePipeResponse::Events { events, .. } => event_error(events),
        _ => None,
    }
}

fn event_error(events: &[EventEnvelope]) -> Option<&str> {
    events.iter().find_map(|event| match &event.event {
        CoreEvent::Error { message, .. } => Some(message.as_str()),
        _ => None,
    })
}

pub fn command_result(events: Vec<EventEnvelope>) -> Result<(), String> {
    event_error(&events).map_or(Ok(()), |message| Err(message.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_feedback_accepts_empty_success_and_reports_errors() {
        assert_eq!(command_result(Vec::new()), Ok(()));
        let response: CorePipeResponse = serde_json::from_value(serde_json::json!({
            "type": "error", "request_id": 1, "code": "test", "message": "failed"
        }))
        .unwrap();
        assert_eq!(response_error(&response), Some("failed"));
        let events = vec![EventEnvelope {
            sequence: 1,
            event: CoreEvent::Error {
                code: "test".into(),
                message: "rejected".into(),
            },
        }];
        assert_eq!(command_result(events.clone()), Err("rejected".into()));
        assert_eq!(
            response_error(&CorePipeResponse::Events {
                request_id: 1,
                events
            }),
            Some("rejected")
        );
        assert_eq!(
            response_error(&CorePipeResponse::Events {
                request_id: 1,
                events: Vec::new()
            }),
            None
        );
    }
}
