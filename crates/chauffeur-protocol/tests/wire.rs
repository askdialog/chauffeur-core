use chauffeur_protocol::client::{ClientCall, CreateWorkspaceParams};
use chauffeur_protocol::events::{ChatEvent, Event, EventEnvelope};
use chauffeur_protocol::rpc::{Incoming, Request};
use serde_json::json;

#[test]
fn a_client_call_serializes_to_json_rpc_method_and_params() {
    let request = Request::new(
        7,
        ClientCall::WorkspaceCreate(CreateWorkspaceParams {
            repos: vec!["web".into()],
            ..Default::default()
        }),
    );
    let value = serde_json::to_value(&request).unwrap();
    assert_eq!(value["jsonrpc"], "2.0");
    assert_eq!(value["id"], 7);
    assert_eq!(value["method"], "workspace.create");
    assert_eq!(value["params"]["repos"], json!(["web"]));
}

#[test]
fn an_incoming_request_rebuilds_into_its_call() {
    let incoming: Incoming = serde_json::from_value(json!({
        "jsonrpc": "2.0", "id": 1, "method": "workspace.list", "params": {}
    }))
    .unwrap();
    let call: ClientCall = serde_json::from_value(incoming.call_value()).unwrap();
    assert!(matches!(call, ClientCall::WorkspaceList(p) if !p.include_archived));
}

#[test]
fn a_chat_event_is_tagged_by_scope_and_type() {
    let envelope = EventEnvelope {
        seq: Some(3),
        ts: "2026-10-13T00:00:00Z".into(),
        event: Event::Chat {
            workspace_id: "w".into(),
            chat_id: "c".into(),
            event: ChatEvent::TurnStarted,
        },
    };
    let value = serde_json::to_value(&envelope).unwrap();
    assert_eq!(value["event"]["scope"], "chat");
    assert_eq!(value["event"]["event"]["type"], "turn_started");
}

#[test]
fn new_chats_default_to_auto_mode_and_stored_modes_still_read() {
    use chauffeur_protocol::model::PermissionMode;
    assert_eq!(PermissionMode::default(), PermissionMode::Auto);
    assert_eq!(
        serde_json::to_value(PermissionMode::Auto).unwrap(),
        json!("auto")
    );
    for mode in [
        PermissionMode::Auto,
        PermissionMode::Default,
        PermissionMode::AcceptEdits,
        PermissionMode::Plan,
        PermissionMode::BypassPermissions,
    ] {
        assert_eq!(PermissionMode::parse(mode.as_str()), Some(mode));
        assert_eq!(serde_json::to_value(mode).unwrap(), json!(mode.as_str()));
    }
}

#[test]
fn a_question_option_carries_its_preview_only_when_it_has_one() {
    use chauffeur_protocol::model::QuestionOption;
    let plain: QuestionOption =
        serde_json::from_value(json!({ "label": "A", "description": "first" })).unwrap();
    assert_eq!(plain.preview, None);
    assert_eq!(
        serde_json::to_value(&plain).unwrap(),
        json!({ "label": "A", "description": "first" })
    );
    let shown: QuestionOption = serde_json::from_value(
        json!({ "label": "B", "description": "", "preview": "```rust\nfn b() {}\n```" }),
    )
    .unwrap();
    assert_eq!(shown.preview.as_deref(), Some("```rust\nfn b() {}\n```"));
    assert_eq!(
        serde_json::to_value(&shown).unwrap()["preview"],
        "```rust\nfn b() {}\n```"
    );
}

#[test]
fn a_prompt_suggestion_is_ephemeral() {
    let event = ChatEvent::PromptSuggestion {
        text: "Run the tests".into(),
    };
    assert!(event.is_ephemeral());
    assert_eq!(
        serde_json::to_value(&event).unwrap(),
        json!({ "type": "prompt_suggestion", "text": "Run the tests" })
    );
}

#[test]
fn a_permission_check_is_ephemeral() {
    let event = ChatEvent::PermissionCheck {
        tool_use_id: "toolu_1".into(),
        checking: true,
    };
    assert!(event.is_ephemeral());
    assert_eq!(
        serde_json::to_value(&event).unwrap(),
        json!({ "type": "permission_check", "tool_use_id": "toolu_1", "checking": true })
    );
}

#[test]
fn a_subagents_text_and_thinking_are_ephemeral_and_the_agents_own_are_kept() {
    let sub: ChatEvent = serde_json::from_value(json!({
        "type": "assistant_text", "message_id": "m:1", "text": "hi", "parent_tool_use_id": "toolu_1"
    }))
    .unwrap();
    assert!(sub.is_ephemeral());
    let thought: ChatEvent = serde_json::from_value(json!({
        "type": "thinking", "message_id": "m:0", "text": "hmm", "parent_tool_use_id": "toolu_1"
    }))
    .unwrap();
    assert!(thought.is_ephemeral());

    let main: ChatEvent = serde_json::from_value(json!({
        "type": "assistant_text", "message_id": "m:1", "text": "hi", "parent_tool_use_id": null
    }))
    .unwrap();
    assert!(!main.is_ephemeral());
    // Thinking stored before subagents' thinking existed still reads, and is written the same.
    let older: ChatEvent =
        serde_json::from_value(json!({ "type": "thinking", "message_id": "m:0", "text": "hmm" }))
            .unwrap();
    assert!(!older.is_ephemeral());
    assert_eq!(
        serde_json::to_value(&older).unwrap(),
        json!({ "type": "thinking", "message_id": "m:0", "text": "hmm" })
    );
}

#[test]
fn the_effective_permission_mode_is_ephemeral_and_optional_on_a_chat() {
    use chauffeur_protocol::model::{ChatInfo, PermissionMode};
    let event = ChatEvent::PermissionModeEffective {
        mode: PermissionMode::Default,
    };
    assert!(event.is_ephemeral());
    assert_eq!(
        serde_json::to_value(&event).unwrap(),
        json!({ "type": "permission_mode_effective", "mode": "default" })
    );
    // A chat from a daemon that predates the field, or whose agent has not reported it yet.
    let mut chat: ChatInfo = serde_json::from_value(json!({
        "id": "c", "workspace_id": "w", "agent": "claude", "model": "claude-haiku-4-5",
        "effort": null, "permission_mode": "auto", "title": null, "status": "idle",
        "agent_session_id": null, "created_at": "2026-10-18T00:00:00Z"
    }))
    .unwrap();
    assert_eq!(chat.permission_mode_effective, None);
    assert!(
        serde_json::to_value(&chat)
            .unwrap()
            .get("permission_mode_effective")
            .is_none()
    );
    chat.permission_mode_effective = Some(PermissionMode::Default);
    assert_eq!(
        serde_json::to_value(&chat).unwrap()["permission_mode_effective"],
        "default"
    );
}
