//! The event log: persisted to the store (except ephemeral events) and fanned out live.

use std::sync::{Arc, Mutex};

use chauffeur_protocol::client::PrInfo;
use chauffeur_protocol::events::{ChatEvent, Event, EventEnvelope, RegistryEvent, WorkspaceEvent};
use tokio::sync::broadcast;

use crate::store::{EventFilter, Store, now};

/// What the core does that an optional module (the Office) acts on, told without naming it. Not
/// an event of the log: nothing is stored or sent to clients.
pub enum Activity<'a> {
    ChatCreated,
    /// A chat went from working to idle.
    TurnEnded,
    /// Plan steps a chat completed since its last progress.
    StepsDone(u32),
    /// A workspace's pull requests, read whole (a failed `gh` call reports nothing).
    PrsRead {
        workspace: &'a str,
        prs: &'a [PrInfo],
    },
}

type Hook = Box<dyn Fn(&Activity) + Send + Sync>;

#[derive(Clone)]
pub struct EventBus {
    store: Store,
    /// Shared, not copied, by the receivers: most of them filter out most events, a tool's
    /// output or a process's line included.
    sender: broadcast::Sender<Arc<EventEnvelope>>,
    /// Held from the store's append to the broadcast: live subscribers rely on sequence numbers
    /// arriving in order (they skip anything at or under the last one seen), and two tasks
    /// publishing at once would otherwise send N+1 before N, losing N for good.
    order: Arc<Mutex<()>>,
    hooks: Arc<Mutex<Vec<Hook>>>,
}

impl EventBus {
    pub fn new(store: Store) -> EventBus {
        let (sender, _) = broadcast::channel(8192);
        EventBus {
            store,
            sender,
            order: Arc::new(Mutex::new(())),
            hooks: Default::default(),
        }
    }

    /// Called with every activity reported from then on, on the reporter's thread: keep it short.
    pub fn hook(&self, hook: impl Fn(&Activity) + Send + Sync + 'static) {
        self.hooks.lock().unwrap().push(Box::new(hook));
    }

    pub fn report(&self, activity: Activity) {
        for hook in self.hooks.lock().unwrap().iter() {
            hook(&activity);
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<EventEnvelope>> {
        self.sender.subscribe()
    }

    /// Stores (unless ephemeral) and broadcasts an event. Returns the envelope sent.
    pub fn publish(&self, event: Event) -> EventEnvelope {
        let ephemeral = is_ephemeral(&event);
        let mut envelope = EventEnvelope {
            seq: None,
            ts: now(),
            event,
        };
        if ephemeral {
            // No receivers is fine: nobody is subscribed.
            let _ = self.sender.send(Arc::new(envelope.clone()));
            return envelope;
        }
        let _order = self.order.lock().unwrap();
        let registry = matches!(envelope.event, Event::Registry { .. });
        let payload = serde_json::to_string(&envelope).expect("events serialize");
        match self.store.append_event(
            &envelope.ts,
            envelope.event.workspace_id(),
            if registry {
                None
            } else {
                envelope.event.chat_id()
            },
            registry,
            &payload,
        ) {
            Ok(seq) => envelope.seq = Some(seq),
            Err(error) => tracing::error!(%error, "could not persist an event"),
        }
        let _ = self.sender.send(Arc::new(envelope.clone()));
        envelope
    }

    /// Broadcasts events stored already (a forked chat's copied conversation), in their order:
    /// live subscribers skip what is older than the last event they saw.
    pub fn announce(&self, envelopes: Vec<EventEnvelope>) {
        let _order = self.order.lock().unwrap();
        for envelope in envelopes {
            let _ = self.sender.send(Arc::new(envelope));
        }
    }

    pub fn registry(&self, event: RegistryEvent) -> EventEnvelope {
        self.publish(Event::Registry { event })
    }

    pub fn workspace(&self, workspace_id: &str, event: WorkspaceEvent) -> EventEnvelope {
        self.publish(Event::Workspace {
            workspace_id: workspace_id.to_string(),
            event,
        })
    }

    pub fn chat(&self, workspace_id: &str, chat_id: &str, event: ChatEvent) -> EventEnvelope {
        self.publish(Event::Chat {
            workspace_id: workspace_id.to_string(),
            chat_id: chat_id.to_string(),
            event,
        })
    }
}

pub fn is_ephemeral(event: &Event) -> bool {
    match event {
        Event::Office { .. }
        | Event::Extension { .. }
        | Event::Registry {
            event:
                RegistryEvent::ExtensionsChanged { .. }
                | RegistryEvent::IdentityChanged { .. }
                | RegistryEvent::MentionsChanged { .. },
        } => true,
        Event::Chat { event, .. } => event.is_ephemeral(),
        Event::Workspace {
            event:
                WorkspaceEvent::ProcessOutput { .. }
                | WorkspaceEvent::TerminalOutput { .. }
                | WorkspaceEvent::TerminalActivity { .. }
                // Whole at each step; `cloud.transfer.status` reads the last one.
                | WorkspaceEvent::Transfer { .. }
                | WorkspaceEvent::Share { .. }
                | WorkspaceEvent::FileShare { .. }
                | WorkspaceEvent::Relay { .. }
                | WorkspaceEvent::Replayed,
            ..
        } => true,
        _ => false,
    }
}

/// What a sidebar shows of every workspace beside its upserts: a workspace share's state, its
/// suggestions and its comments (story 53), which badge or are told of a workspace that is not
/// open; the extensions and the identity in force; and the user's mentions. Ephemeral, yet given to registry
/// subscriptions, which take no ephemeral event otherwise.
pub fn for_registry(event: &Event) -> bool {
    matches!(
        event,
        Event::Registry {
            event: RegistryEvent::ExtensionsChanged { .. }
                | RegistryEvent::IdentityChanged { .. }
                | RegistryEvent::MentionsChanged { .. }
        } | Event::Workspace {
            event: WorkspaceEvent::Share { .. },
            ..
        } | Event::Chat {
            event: ChatEvent::Suggestions { .. } | ChatEvent::ShareComments { .. },
            ..
        }
    )
}

/// Whether a live envelope passes a subscription filter.
pub fn matches(filter: &EventFilter, envelope: &EventEnvelope) -> bool {
    // Office events go to Office views only, not even to `all`.
    if matches!(envelope.event, Event::Office { .. }) {
        return filter.office;
    }
    if let Event::Extension { id, .. } = envelope.event {
        return match id {
            chauffeur_protocol::extensions::ExtensionId::Office => filter.office,
            chauffeur_protocol::extensions::ExtensionId::Database => filter.registry,
        };
    }
    if filter.registry && for_registry(&envelope.event) {
        return true;
    }
    let registry = matches!(envelope.event, Event::Registry { .. });
    let chat_id = if registry {
        None
    } else {
        envelope.event.chat_id()
    };
    filter.matches(envelope.event.workspace_id(), chat_id, registry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chauffeur_protocol::events::ChatEvent;

    #[test]
    fn a_sidebar_hears_every_workspace_s_suggestions_and_nothing_else_ephemeral() {
        let registry = EventFilter {
            registry: true,
            ..Default::default()
        };
        let envelope = |event| EventEnvelope {
            seq: None,
            ts: String::new(),
            event,
        };
        let suggestions = envelope(Event::Chat {
            workspace_id: "w".into(),
            chat_id: "c".into(),
            event: ChatEvent::Suggestions {
                suggestions: Vec::new(),
            },
        });
        assert!(for_registry(&suggestions.event));
        assert!(matches(&registry, &suggestions));
        let delta = envelope(Event::Chat {
            workspace_id: "w".into(),
            chat_id: "c".into(),
            event: ChatEvent::TextDelta {
                message_id: "m".into(),
                text: "he".into(),
                parent_tool_use_id: None,
            },
        });
        assert!(!for_registry(&delta.event));
        assert!(!matches(&registry, &delta));
    }

    /// Many tasks publishing at once: a live subscriber sees every persisted event, in sequence
    /// order, which is what lets it skip duplicates by sequence number without losing any.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_publishers_reach_a_subscriber_in_sequence_order() {
        let store = Store::open_in_memory().unwrap();
        let bus = EventBus::new(store);
        let mut live = bus.subscribe();
        let publishers = 16;
        let each = 2000;
        let total = publishers * each;
        let collector = tokio::spawn(async move {
            let mut seqs = Vec::with_capacity(total);
            while seqs.len() < total {
                match live.recv().await {
                    Ok(envelope) => seqs.push(envelope.seq.unwrap()),
                    Err(error) => panic!("the subscriber fell behind: {error}"),
                }
            }
            seqs
        });
        let tasks: Vec<_> = (0..publishers)
            .map(|p| {
                let bus = bus.clone();
                tokio::task::spawn_blocking(move || {
                    for i in 0..each {
                        std::thread::yield_now();
                        bus.workspace(
                            "w",
                            WorkspaceEvent::Progress {
                                message: format!("{p}:{i}"),
                            },
                        );
                    }
                })
            })
            .collect();
        for task in tasks {
            task.await.unwrap();
        }
        let seqs = tokio::time::timeout(std::time::Duration::from_secs(30), collector)
            .await
            .expect("every event is delivered")
            .unwrap();
        assert!(
            seqs.windows(2).all(|w| w[0] < w[1]),
            "sequence numbers arrived out of order"
        );
    }

    #[test]
    fn persisted_events_get_increasing_sequence_numbers_and_deltas_get_none() {
        let store = Store::open_in_memory().unwrap();
        let bus = EventBus::new(store.clone());
        let first = bus.chat("w", "c", ChatEvent::TurnStarted);
        let delta = bus.chat(
            "w",
            "c",
            ChatEvent::TextDelta {
                message_id: "m".into(),
                text: "he".into(),
                parent_tool_use_id: None,
            },
        );
        let second = bus.workspace(
            "w",
            WorkspaceEvent::Progress {
                message: "x".into(),
            },
        );
        assert!(first.seq.unwrap() < second.seq.unwrap());
        assert_eq!(delta.seq, None);

        let only_chat = EventFilter {
            chats: vec!["c".into()],
            ..Default::default()
        };
        let replayed = store.events_after(0, &only_chat).unwrap();
        assert_eq!(replayed.len(), 1);
        assert_eq!(replayed[0].seq, first.seq);
    }

    #[test]
    fn the_tail_and_what_comes_before_it_make_up_the_whole_history() {
        use chauffeur_protocol::client::Replay;
        let store = Store::open_in_memory().unwrap();
        let bus = EventBus::new(store.clone());
        let progress = |n: u32| WorkspaceEvent::Progress {
            message: n.to_string(),
        };
        // Two chats interleaved, and workspace events among them.
        let mut all = Vec::new();
        for n in 0..4 {
            all.push(bus.chat("w", "a", ChatEvent::TurnStarted).seq.unwrap());
            all.push(bus.chat("w", "b", ChatEvent::TurnStarted).seq.unwrap());
            all.push(bus.chat("w", "a", ChatEvent::TurnStarted).seq.unwrap());
            all.push(bus.workspace("w", progress(n)).seq.unwrap());
        }
        bus.chat("other", "c", ChatEvent::TurnStarted);
        let filter = EventFilter {
            workspaces: vec!["w".into()],
            ..Default::default()
        };
        let seqs = |part: Option<Replay>| -> Vec<i64> {
            store
                .events_part(0, &filter, part)
                .unwrap()
                .iter()
                .map(|e| e.seq.unwrap())
                .collect()
        };
        assert_eq!(seqs(None), all);
        let tail = seqs(Some(Replay::Tail { per_chat: 3 }));
        let before = seqs(Some(Replay::BeforeTail { per_chat: 3 }));
        // Chat a has 8 events, b has 4, 4 belong to no chat: 3 + 3 + 4 in the tail.
        assert_eq!(tail.len(), 10);
        assert_eq!(before.len(), 6);
        // Ascending, disjoint, and nothing lost.
        assert!(tail.windows(2).all(|w| w[0] < w[1]));
        assert!(before.windows(2).all(|w| w[0] < w[1]));
        let mut both: Vec<i64> = tail.iter().chain(&before).copied().collect();
        both.sort();
        assert_eq!(both, all);
        // Each chat keeps its newest events, workspace events stay in the tail.
        assert!(before.iter().all(|seq| *seq <= all[8]));
        // After a point, only what is newer.
        let later = store
            .events_part(all[8], &filter, Some(Replay::Tail { per_chat: 1 }))
            .unwrap();
        assert!(later.iter().all(|e| e.seq.unwrap() > all[8]));
    }
    #[test]
    fn recent_history_has_a_stable_exclusive_cursor_for_older_pages() {
        use chauffeur_protocol::client::Replay;
        let store = Store::open_in_memory().unwrap();
        let bus = EventBus::new(store.clone());
        let filter = EventFilter {
            chats: vec!["a".into()],
            ..Default::default()
        };
        for _ in 0..5 {
            bus.chat("w", "a", ChatEvent::TurnStarted);
            bus.chat("other", "b", ChatEvent::TurnStarted);
        }
        let tail = store
            .events_page(0, None, &filter, Some(Replay::Tail { per_chat: 2 }))
            .unwrap();
        assert_eq!(
            tail.iter().map(|e| e.seq.unwrap()).collect::<Vec<_>>(),
            vec![7, 9]
        );
        bus.chat("w", "a", ChatEvent::TurnStarted);
        let head = store.events_page(0, tail[0].seq, &filter, None).unwrap();
        assert_eq!(
            head.iter().map(|e| e.seq.unwrap()).collect::<Vec<_>>(),
            vec![1, 3, 5]
        );
        let page = store.events_page(3, tail[0].seq, &filter, None).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].seq, Some(5));
    }
}
