use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Debug)]
pub enum SlackCommand {
    Schedule {
        attendee_slack_ids: Vec<String>,
        duration_minutes: u32,
    },
    Status { meeting_id: Uuid },
    Calendars,
    Help,
    Unknown(String),
}

pub fn parse(text: &str) -> SlackCommand {
    let parts: Vec<&str> = text.split_whitespace().collect();
    match parts.as_slice() {
        [subcommand, rest @ ..] => match *subcommand {
            "schedule" | "meet" => {
                let mut attendees = Vec::new();
                let mut duration = 30u32;
                for token in rest {
                    if token.starts_with('@') {
                        attendees.push(token.trim_start_matches('@').to_string());
                    } else if let Some(mins) = parse_duration(token) {
                        duration = mins;
                    }
                }
                SlackCommand::Schedule {
                    attendee_slack_ids: attendees,
                    duration_minutes: duration,
                }
            }
            "status" => {
                if let Some(id_str) = rest.first() {
                    if let Ok(id) = id_str.parse::<Uuid>() {
                        return SlackCommand::Status { meeting_id: id };
                    }
                }
                SlackCommand::Unknown(text.to_string())
            }
            "calendars" | "cal" => SlackCommand::Calendars,
            "help" | "" => SlackCommand::Help,
            _ => SlackCommand::Unknown(text.to_string()),
        },
        [] => SlackCommand::Help,
    }
}

fn parse_duration(s: &str) -> Option<u32> {
    let s = s.to_lowercase();
    if let Some(h) = s.strip_suffix("hour").or_else(|| s.strip_suffix('h')) {
        return h.parse::<u32>().ok().map(|n| n * 60);
    }
    if let Some(m) = s.strip_suffix("min").or_else(|| s.strip_suffix('m')) {
        return m.parse::<u32>().ok();
    }
    s.parse::<u32>().ok()
}

pub fn help_response() -> Value {
    json!({
        "response_type": "ephemeral",
        "text": "*PlanPal* — AI meeting scheduler\n\n*Commands:*\n• `/planpal schedule @alice @bob 30min` — find a slot\n• `/planpal status <meeting-id>` — check proposals\n• `/planpal calendars` — list connected calendars\n• `/planpal help` — this message"
    })
}

pub fn ack_schedule_response(duration: u32, count: usize) -> Value {
    json!({
        "response_type": "ephemeral",
        "text": format!(
            "⏳ Finding a {duration}-minute slot for {count} attendees… I'll post the proposals here shortly."
        )
    })
}

pub fn unknown_response(text: &str) -> Value {
    json!({
        "response_type": "ephemeral",
        "text": format!("Unknown command: `{text}`\nType `/planpal help` for available commands.")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn parse_empty_text() {
        assert!(matches!(parse(""), SlackCommand::Help));
    }

    #[test]
    fn parse_help() {
        assert!(matches!(parse("help"), SlackCommand::Help));
    }

    #[test]
    fn parse_calendars() {
        assert!(matches!(parse("calendars"), SlackCommand::Calendars));
    }

    #[test]
    fn parse_cal_alias() {
        assert!(matches!(parse("cal"), SlackCommand::Calendars));
    }

    #[test]
    fn parse_schedule_basic() {
        match parse("schedule @alice @bob 30min") {
            SlackCommand::Schedule { attendee_slack_ids, duration_minutes } => {
                assert_eq!(attendee_slack_ids.len(), 2);
                assert_eq!(duration_minutes, 30);
            }
            other => panic!("Expected Schedule, got {:?}", other),
        }
    }

    #[test]
    fn parse_schedule_hour_duration() {
        match parse("schedule @alice 1h") {
            SlackCommand::Schedule { duration_minutes, .. } => {
                assert_eq!(duration_minutes, 60);
            }
            other => panic!("Expected Schedule, got {:?}", other),
        }
    }

    #[test]
    fn parse_schedule_default_duration() {
        match parse("schedule @alice") {
            SlackCommand::Schedule { duration_minutes, .. } => {
                assert_eq!(duration_minutes, 30);
            }
            other => panic!("Expected Schedule, got {:?}", other),
        }
    }

    #[test]
    fn parse_meet_alias() {
        assert!(matches!(
            parse("meet @alice 45min"),
            SlackCommand::Schedule { .. }
        ));
    }

    #[test]
    fn parse_status_valid_uuid() {
        let uuid_str = "550e8400-e29b-41d4-a716-446655440000";
        match parse(&format!("status {}", uuid_str)) {
            SlackCommand::Status { meeting_id } => {
                assert_eq!(meeting_id, uuid_str.parse::<Uuid>().unwrap());
            }
            other => panic!("Expected Status, got {:?}", other),
        }
    }

    #[test]
    fn parse_status_invalid_uuid() {
        assert!(matches!(parse("status not-a-uuid"), SlackCommand::Unknown(_)));
    }

    #[test]
    fn parse_unknown_command() {
        assert!(matches!(parse("foobar"), SlackCommand::Unknown(_)));
    }

    #[test]
    fn parse_duration_min_suffix() {
        assert_eq!(parse_duration("45min"), Some(45));
    }

    #[test]
    fn parse_duration_m_suffix() {
        assert_eq!(parse_duration("30m"), Some(30));
    }

    #[test]
    fn parse_duration_h_suffix() {
        assert_eq!(parse_duration("2h"), Some(120));
    }

    #[test]
    fn parse_duration_plain_number() {
        assert_eq!(parse_duration("60"), Some(60));
    }

    #[test]
    fn parse_duration_invalid() {
        assert_eq!(parse_duration("abc"), None);
    }

    #[test]
    fn help_response_has_response_type() {
        assert_eq!(help_response()["response_type"], "ephemeral");
    }

    #[test]
    fn ack_response_contains_duration() {
        let resp = ack_schedule_response(30, 2);
        assert!(resp["text"].as_str().unwrap().contains("30"));
    }

    #[test]
    fn unknown_response_echoes_text() {
        let resp = unknown_response("boom");
        assert!(resp["text"].as_str().unwrap().contains("boom"));
    }
}
