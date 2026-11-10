use chauffeur_protocol::events::{ChatEvent, Event, EventEnvelope, RegistryEvent};
use chauffeurd::store::{EventFilter, Store};

fn append(store: &Store, workspace: &str, chat: &str, registry: bool) -> i64 {
    let event = if registry {
        Event::Registry {
            event: RegistryEvent::ChatRemoved {
                workspace_id: workspace.into(),
                chat_id: chat.into(),
            },
        }
    } else {
        Event::Chat {
            workspace_id: workspace.into(),
            chat_id: chat.into(),
            event: ChatEvent::UserMessage {
                text: "message".into(),
                queued: false,
                attachments: None,
                message_id: None,
                suggested_by: None,
                delegated_answer: None,
            },
        }
    };
    let envelope = EventEnvelope {
        seq: None,
        ts: "2026-10-18T00:00:00Z".into(),
        event,
    };
    store
        .append_event(
            &envelope.ts,
            Some(workspace),
            Some(chat),
            registry,
            &serde_json::to_string(&envelope).unwrap(),
        )
        .unwrap()
}

#[test]
fn history_pages_count_matching_events_and_resume_across_other_chats() {
    let store = Store::open_in_memory().unwrap();
    for _ in 0..30 {
        append(&store, "other", "other-chat", false);
    }
    let first = append(&store, "wanted", "wanted-chat", false);
    append(&store, "wanted", "wanted-chat", true);
    append(&store, "other", "other-chat", false);
    let second = append(&store, "wanted", "wanted-chat", false);
    append(&store, "other", "other-chat", false);
    let third = append(&store, "wanted", "wanted-chat", false);
    let filter = EventFilter {
        chats: vec!["wanted-chat".into()],
        limit: Some(2),
        ..Default::default()
    };
    let page = store.events_after(0, &filter).unwrap();
    assert_eq!(
        page.iter()
            .map(|event| event.seq.unwrap())
            .collect::<Vec<_>>(),
        vec![first, second]
    );
    let page = store.events_after(second, &filter).unwrap();
    assert_eq!(
        page.iter()
            .map(|event| event.seq.unwrap())
            .collect::<Vec<_>>(),
        vec![third]
    );
    assert!(store.events_after(third, &filter).unwrap().is_empty());
    assert!(
        store
            .events_after(
                0,
                &EventFilter {
                    limit: Some(0),
                    ..filter
                }
            )
            .unwrap()
            .is_empty()
    );
}

#[test]
fn persisted_filters_match_live_filters_including_registry_exclusion() {
    let store = Store::open_in_memory().unwrap();
    let records = [
        ("a", "one", false),
        ("a", "one", true),
        ("b", "two", false),
        ("b", "two", true),
    ];
    let seqs: Vec<_> = records
        .iter()
        .map(|(w, c, r)| append(&store, w, c, *r))
        .collect();
    for filter in [
        EventFilter::default(),
        EventFilter {
            all: true,
            ..Default::default()
        },
        EventFilter {
            registry: true,
            ..Default::default()
        },
        EventFilter {
            chats: vec!["one".into(), "two".into()],
            ..Default::default()
        },
        EventFilter {
            workspaces: vec!["a".into()],
            ..Default::default()
        },
        EventFilter {
            registry: true,
            chats: vec!["two".into()],
            workspaces: vec!["a".into()],
            ..Default::default()
        },
    ] {
        let expected: Vec<_> = records
            .iter()
            .zip(&seqs)
            .filter_map(|((w, c, r), seq)| filter.matches(Some(w), Some(c), *r).then_some(*seq))
            .collect();
        let actual: Vec<_> = store
            .events_after(0, &filter)
            .unwrap()
            .iter()
            .map(|event| event.seq.unwrap())
            .collect();
        assert_eq!(actual, expected, "{filter:?}");
    }
}
