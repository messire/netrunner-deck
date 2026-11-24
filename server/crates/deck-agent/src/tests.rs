use super::*;
use serde_json::{json, Value};

/// A real `claude -p --output-format stream-json` run (Claude Code 2.1):
/// AskUserQuestion, then a Write that asked for permission, then the result.
const FIXTURE: &str = include_str!("../fixtures/ask-then-write.jsonl");

fn fixture() -> Vec<Value> {
    FIXTURE
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// The fixture as the host relays it: after each permission request the
/// host's own `deck_resolved` (the client answered).
fn relayed() -> Vec<Value> {
    let mut out = Vec::new();
    for msg in fixture() {
        let id = permission_request_id(&msg).map(str::to_string);
        out.push(msg);
        if let Some(id) = id {
            out.push(proto::resolved(&id));
        }
    }
    out
}

#[test]
fn args_for_new_and_resumed_sessions() {
    let new = proto::claude_args(proto::Session::New("abc"), Some("opus"), None);
    assert!(new.windows(2).any(|w| w == ["--session-id", "abc"]));
    assert!(new.windows(2).any(|w| w == ["--model", "opus"]));
    assert!(new
        .windows(2)
        .any(|w| w == ["--permission-prompt-tool", "stdio"]));
    assert!(new.contains(&"--replay-user-messages".to_string()));
    let resumed = proto::claude_args(proto::Session::Resume("abc"), None, Some("plan"));
    assert!(resumed.windows(2).any(|w| w == ["--resume", "abc"]));
    assert!(resumed
        .windows(2)
        .any(|w| w == ["--permission-mode", "plan"]));
    assert!(!resumed.contains(&"--session-id".to_string()));
}

#[test]
fn user_message_text_and_images() {
    let v: Value = serde_json::from_str(&proto::user_message("hi", &[]).unwrap()).unwrap();
    assert_eq!(v["type"], "user");
    assert_eq!(v["message"]["content"], "hi");
    let img = proto::Image {
        media_type: "image/png".into(),
        data: "AAAA".into(),
    };
    let v: Value = serde_json::from_str(&proto::user_message("look", &[img]).unwrap()).unwrap();
    assert_eq!(
        v["message"]["content"][0]["source"]["media_type"],
        "image/png"
    );
    assert_eq!(v["message"]["content"][1]["text"], "look");
    assert!(proto::user_message("  ", &[]).is_err());
    let bad = proto::Image {
        media_type: "text/html".into(),
        data: "x".into(),
    };
    assert!(proto::user_message("x", &[bad]).is_err());
}

#[test]
fn permission_response_shape() {
    let allow = json!({"behavior":"allow","updatedInput":{"file_path":"/a"}});
    let v: Value =
        serde_json::from_str(&proto::permission_response("r1", &allow).unwrap()).unwrap();
    assert_eq!(v["type"], "control_response");
    assert_eq!(v["response"]["request_id"], "r1");
    assert_eq!(v["response"]["response"]["behavior"], "allow");
    assert!(proto::permission_response("r1", &json!({"behavior":"allow"})).is_err());
    assert!(proto::permission_response("r1", &json!({"behavior":"maybe"})).is_err());
    assert!(proto::permission_response("r1", &json!({"behavior":"deny","message":"no"})).is_ok());
}

#[test]
fn ops_round_trip_through_the_inbox() {
    let ops = [
        Op::from_request("send", &json!({"text":"hello"})).unwrap(),
        Op::from_request(
            "respond",
            &json!({"request_id":"r","response":{"behavior":"deny","message":"no"}}),
        )
        .unwrap(),
        Op::from_request("interrupt", &json!({})).unwrap(),
        Op::from_request("set", &json!({"model":"sonnet","permission_mode":"plan"})).unwrap(),
        Op::from_request("set", &json!({"model":null})).unwrap(),
    ];
    for op in ops {
        assert_eq!(Op::from_line(&op.to_line()).unwrap(), op);
        assert!(!op.stdin_lines(1).unwrap().is_empty());
    }
    assert!(Op::from_request("set", &json!({})).is_err());
    assert!(Op::from_request("set", &json!({"permission_mode":"yolo"})).is_err());
    assert!(Op::from_request("respond", &json!({"request_id":"r"})).is_err());
    assert!(Op::from_request("send", &json!({"text":""})).is_err());
    assert!(Op::from_request("rm", &json!({})).is_err());
    let set = Op::from_request("set", &json!({"model":"sonnet","permission_mode":"plan"})).unwrap();
    let lines = set.stdin_lines(7).unwrap();
    assert!(lines[0].contains("set_model") && lines[0].contains("deck-model-7"));
    assert!(lines[1].contains("set_permission_mode"));
}

#[test]
fn resume_and_restart_are_host_operations() {
    let resume = Op::from_request("resume", &json!({"session_id":"5f0c-ab_9"})).unwrap();
    assert_eq!(
        resume,
        Op::Resume {
            session_id: "5f0c-ab_9".into()
        }
    );
    let restart = Op::from_request("restart", &json!({})).unwrap();
    for op in [resume, restart] {
        assert_eq!(Op::from_line(&op.to_line()).unwrap(), op);
        assert!(op.stdin_lines(1).unwrap().is_empty());
    }
    assert!(Op::from_request("resume", &json!({})).is_err());
    assert!(Op::from_request("resume", &json!({"session_id":"a b"})).is_err());
    assert!(Op::from_request("resume", &json!({"session_id":"--model"})).is_err());
    assert!(!valid_session_id(""));
    assert!(!valid_session_id("x;rm"));
}

#[test]
fn tracker_follows_a_real_session() {
    let mut t = Tracker::default();
    let mut seen = Vec::new();
    for msg in relayed() {
        if let Some(s) = t.observe(&msg) {
            seen.push(s);
        }
    }
    // Text streams (working), each permission request blocks and its answer
    // resumes work, and the result ends the turn.
    assert_eq!(
        seen,
        [
            Status::Working,
            Status::Blocked,
            Status::Working,
            Status::Blocked,
            Status::Working,
            Status::Idle
        ]
    );
    assert_eq!(
        t.session_id.as_deref(),
        Some("60eef6dd-b9b4-4735-8aa2-174874c082f1")
    );
    assert!(t.model.is_some());
    assert!(t.open_requests().is_empty());
}

#[test]
fn tracker_user_turn_and_cancel() {
    let mut t = Tracker::default();
    assert_eq!(
        t.observe(&json!({"type":"user","message":{"role":"user","content":"hi"}})),
        Some(Status::Working)
    );
    // A tool result is not a new turn.
    let mut idle = Tracker::default();
    assert_eq!(
        idle.observe(&json!({"type":"user","message":{"content":[{"type":"tool_result"}]}})),
        None
    );
    let req = json!({"type":"control_request","request_id":"q","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{}}});
    assert_eq!(t.observe(&req), Some(Status::Blocked));
    assert_eq!(
        t.observe(&json!({"type":"control_cancel_request","request_id":"q"})),
        Some(Status::Working)
    );
    assert_eq!(
        t.observe(&proto::host_state("exited", Some("code 1"))),
        Some(Status::Idle)
    );
}

#[test]
fn backlog_keeps_the_turn_meta_and_open_requests() {
    let mut b = Backlog::default();
    let msgs = fixture();
    let first_request = msgs
        .iter()
        .position(|m| permission_request_id(m).is_some())
        .unwrap();
    for (i, m) in msgs.iter().enumerate().take(first_request + 1) {
        b.push(i as u64, m);
    }
    let req_id = permission_request_id(&msgs[first_request])
        .unwrap()
        .to_string();
    assert!(b.is_open(&req_id));
    assert_eq!(b.since(None).len(), first_request + 1);
    assert_eq!(b.since(Some(first_request as u64 - 1)).len(), 1);

    // Answered: the request leaves, the rest of the turn stays.
    b.push(1000, &proto::resolved(&req_id));
    assert!(!b.is_open(&req_id));
    assert!(!b
        .since(None)
        .iter()
        .any(|e| e.msg.contains(&req_id) && e.msg.contains("can_use_tool")));

    // Turn end: only meta (init + initialize response) and the result remain.
    for (i, m) in msgs.iter().enumerate().skip(first_request + 1) {
        if permission_request_id(m).is_some() {
            continue;
        }
        b.push(2000 + i as u64, m);
    }
    let left: Vec<Value> = b
        .since(None)
        .iter()
        .map(|e| serde_json::from_str(&e.msg).unwrap())
        .collect();
    let kinds: Vec<(&str, &str)> = left
        .iter()
        .map(|m| {
            (
                m["type"].as_str().unwrap(),
                m.get("subtype").and_then(Value::as_str).unwrap_or(""),
            )
        })
        .collect();
    assert!(kinds.contains(&("system", "init")), "{kinds:?}");
    assert!(kinds.contains(&("control_response", "")), "{kinds:?}");
    assert!(kinds.iter().any(|k| k.0 == "result"), "{kinds:?}");
    assert!(
        !kinds
            .iter()
            .any(|k| k.0 == "stream_event" || k.0 == "assistant"),
        "{kinds:?}"
    );
    assert_eq!(b.last_seq(), Some(2000 + msgs.len() as u64 - 1));
}

#[test]
fn backlog_open_request_survives_turn_end_and_size_cap() {
    let mut b = Backlog::default();
    let req = json!({"type":"control_request","request_id":"q","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{}}});
    b.push(1, &req);
    let big = "x".repeat(1024 * 1024);
    for i in 0..12 {
        b.push(10 + i, &json!({"type":"assistant","text": big}));
    }
    assert!(b.is_open("q"));
    assert!(b.since(None)[0].msg.contains("can_use_tool"));
    assert!(b.len() < 12);
    b.push(100, &json!({"type":"result"}));
    assert_eq!(b.len(), 2);
}

#[test]
fn event_lines_and_uuid() {
    let line = event_line(5, &json!({"type":"x"}));
    assert_eq!(parse_event_line(&line), Some((5, json!({"type":"x"}))));
    assert_eq!(parse_event_line(r#"{"seq":1,"msg":3}"#), None);
    assert_eq!(parse_event_line("nope"), None);
    let u = uuid_from_seed([0xff; 16]);
    assert_eq!(u.len(), 36);
    assert_eq!(&u[14..15], "4");
}

#[test]
fn settings_are_confirmed_by_claude() {
    let set = Op::from_request("set", &json!({"model":"sonnet","permission_mode":"plan"})).unwrap();
    let reqs = set.settings_requests(3);
    assert_eq!(reqs.len(), 2);
    let lines = set.stdin_lines(3).unwrap();
    assert!(lines[0].contains(&reqs[0].0) && lines[1].contains(&reqs[1].0));
    let ok =
        json!({"type":"control_response","response":{"subtype":"success","request_id":reqs[1].0}});
    assert_eq!(control_success_id(&ok), Some(reqs[1].0.as_str()));
    let mut t = Tracker::default();
    t.observe(&reqs[1].1);
    t.observe(&reqs[0].1);
    assert_eq!(t.permission_mode.as_deref(), Some("plan"));
    assert_eq!(t.model.as_deref(), Some("sonnet"));
    assert!(Op::Interrupt.settings_requests(1).is_empty());
}

#[test]
fn the_running_note_names_the_session_and_stays_for_new_clients() {
    let mut tracker = Tracker::default();
    let mut backlog = Backlog::default();
    backlog.push(1, &proto::running("s-1"));
    tracker.observe(&proto::running("s-1"));
    assert_eq!(tracker.session_id.as_deref(), Some("s-1"));
    backlog.push(2, &json!({"type":"result","subtype":"success"}));
    backlog.push(3, &proto::running("s-2"));
    let kept: Vec<u64> = backlog.since(None).iter().map(|e| e.seq).collect();
    assert_eq!(kept, vec![2, 3], "only the latest running note is pinned");
}
