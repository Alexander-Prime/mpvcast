use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Serialize)]
pub struct Command {
    pub command: Vec<Value>,
    pub request_id: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Reply {
    pub request_id: u64,
    pub error: String,
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Event {
    pub event: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Message {
    Reply(Reply),
    Event(Event),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn command_serializes_to_canonical_json() {
        let cmd = Command {
            command: vec![json!("get_property"), json!("pause")],
            request_id: 42,
        };
        let actual = serde_json::to_string(&cmd).unwrap();
        assert_eq!(
            actual,
            r#"{"command":["get_property","pause"],"request_id":42}"#
        );
    }

    #[test]
    fn reply_success_with_data() {
        let reply: Reply =
            serde_json::from_str(r#"{"error":"success","data":42,"request_id":1}"#).unwrap();
        assert_eq!(reply.request_id, 1);
        assert_eq!(reply.error, "success");
        assert_eq!(reply.data, Some(json!(42)));
    }

    #[test]
    fn reply_success_without_data() {
        let reply: Reply = serde_json::from_str(r#"{"error":"success","request_id":7}"#).unwrap();
        assert_eq!(reply.request_id, 7);
        assert_eq!(reply.error, "success");
        assert_eq!(reply.data, None);
    }

    #[test]
    fn reply_error() {
        let reply: Reply =
            serde_json::from_str(r#"{"error":"property unavailable","request_id":3}"#).unwrap();
        assert_eq!(reply.error, "property unavailable");
        assert_eq!(reply.data, None);
    }

    #[test]
    fn event_with_extras() {
        let event: Event = serde_json::from_str(r#"{"event":"end-file","reason":"eof"}"#).unwrap();
        assert_eq!(event.event, "end-file");
        assert_eq!(event.extra.get("reason"), Some(&json!("eof")));
    }

    #[test]
    fn event_without_extras() {
        let event: Event = serde_json::from_str(r#"{"event":"idle"}"#).unwrap();
        assert_eq!(event.event, "idle");
        assert!(event.extra.is_empty());
    }

    #[test]
    fn message_demuxes_reply() {
        let msg: Message =
            serde_json::from_str(r#"{"error":"success","data":true,"request_id":9}"#).unwrap();
        match msg {
            Message::Reply(r) => {
                assert_eq!(r.request_id, 9);
                assert_eq!(r.data, Some(json!(true)));
            }
            Message::Event(_) => panic!("expected Reply, got Event"),
        }
    }

    #[test]
    fn message_demuxes_event() {
        let msg: Message = serde_json::from_str(r#"{"event":"playback-restart"}"#).unwrap();
        match msg {
            Message::Event(e) => assert_eq!(e.event, "playback-restart"),
            Message::Reply(_) => panic!("expected Event, got Reply"),
        }
    }
}
