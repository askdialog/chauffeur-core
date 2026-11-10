//! A Conductor chat as a Markdown file: the user's messages, the agent's texts with their time,
//! and one line per tool call. Not thinking, tool outputs nor subagents' work, which Conductor
//! keeps as the agent SDK's JSON messages (Claude's and Codex's alike).

use std::collections::HashSet;

use serde_json::Value;

use super::db::{Message, Session};

/// The file name a chat's history gets, unique among `taken` (which it joins).
pub fn file_name(title: &str, taken: &mut HashSet<String>) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '\0' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let mut base: String = cleaned.trim().trim_start_matches('.').trim().to_string();
    if base.is_empty() {
        base = "Untitled".into();
    }
    base = base
        .chars()
        .take(80)
        .collect::<String>()
        .trim_end()
        .to_string();
    let mut name = format!("{base}.md");
    let mut n = 2;
    while !taken.insert(name.to_lowercase()) {
        name = format!("{base} ({n}).md");
        n += 1;
    }
    name
}

pub fn render(session: &Session, messages: &[Message]) -> String {
    let agent = agent_name(&session.agent);
    let title = if session.title.trim().is_empty() {
        "Untitled"
    } else {
        session.title.trim()
    };
    let mut out = format!("# {title}\n\nA Conductor chat with {agent}");
    if let Some(model) = session.model.as_deref().filter(|m| !m.is_empty()) {
        out.push_str(&format!(" ({model})"));
    }
    out.push_str(&format!(
        ", started {}. Imported into Chauffeur; the conversation cannot be resumed.\n",
        time(&session.created_at)
    ));
    // Consecutive texts of the agent form one section, under the time of the first.
    let mut agent_open = false;
    // Tool lines form a list, which a text after them must not continue.
    let mut in_list = false;
    for message in messages {
        if message.role == "user" {
            if message.content.trim().is_empty() {
                continue;
            }
            out.push_str(&format!(
                "\n## You · {}\n\n{}\n",
                time(&message.created_at),
                message.content.trim()
            ));
            agent_open = false;
            in_list = false;
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&message.content) else {
            continue;
        };
        for part in parts(&value) {
            if !agent_open {
                out.push_str(&format!("\n## {agent} · {}\n", time(&message.created_at)));
                agent_open = true;
                in_list = false;
            }
            match part {
                Part::Tool(line) => {
                    if !in_list {
                        out.push('\n');
                    }
                    out.push_str(&format!("- {line}\n"));
                    in_list = true;
                }
                Part::Text(text) => {
                    out.push_str(&format!("\n{text}\n"));
                    in_list = false;
                }
                Part::Error(text) => {
                    out.push_str(&format!("\n> Error: {text}\n"));
                    in_list = false;
                }
            }
        }
    }
    out.push('\n');
    out
}

enum Part {
    Text(String),
    Tool(String),
    Error(String),
}

/// What an SDK message shows: the main agent's texts and tool calls, and errors.
fn parts(value: &Value) -> Vec<Part> {
    match value["type"].as_str() {
        Some("assistant") if value["parent_tool_use_id"].is_null() => value["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|block| match block["type"].as_str() {
                Some("text") => block["text"]
                    .as_str()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(|t| Part::Text(t.to_string())),
                Some("tool_use") => {
                    let name = block["name"].as_str().unwrap_or("tool");
                    Some(Part::Tool(
                        match crate::design::tool_detail(&block["input"]) {
                            Some(detail) => format!("*{name}*: `{}`", detail.replace('`', "'")),
                            None => format!("*{name}*"),
                        },
                    ))
                }
                _ => None,
            })
            .collect(),
        Some("error") => value["content"]
            .as_str()
            .map(|text| vec![Part::Error(text.trim().to_string())])
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn agent_name(agent: &str) -> &'static str {
    match agent {
        "codex" => "Codex",
        _ => "Claude",
    }
}

/// Conductor's timestamps (`2026-10-02T05:37:21.545Z`, or SQLite's `2026-07-20 15:31:57`), both
/// UTC, to the minute.
fn time(stamp: &str) -> String {
    let minute: String = stamp.chars().take(16).collect();
    format!("{} UTC", minute.replace('T', " "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(role: &str, content: &str, at: &str) -> Message {
        Message {
            role: role.into(),
            content: content.into(),
            created_at: at.into(),
        }
    }

    #[test]
    fn a_chat_reads_as_the_users_messages_the_agents_texts_and_a_line_per_tool_call() {
        let session = Session {
            id: "s".into(),
            workspace_id: "w".into(),
            agent: "codex".into(),
            title: "Price filter".into(),
            model: Some("gpt-5".into()),
            created_at: "2026-10-02T05:37:21.545Z".into(),
            messages: 5,
        };
        let messages = [
            message("user", "Add a price filter", "2026-10-02T05:37:21.545Z"),
            message(
                "assistant",
                r#"{"type":"system","subtype":"init"}"#,
                "2026-10-02T05:37:22Z",
            ),
            message(
                "assistant",
                r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"secret"},{"type":"tool_use","name":"Bash","input":{"command":"rg price\nmore"}}]}}"#,
                "2026-10-02T05:38:00Z",
            ),
            message(
                "assistant",
                r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"output"}]}}"#,
                "2026-10-02T05:38:01Z",
            ),
            message(
                "assistant",
                r#"{"type":"assistant","parent_tool_use_id":"t","message":{"content":[{"type":"text","text":"subagent"}]}}"#,
                "2026-10-02T05:38:02Z",
            ),
            message(
                "assistant",
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Done: the filter is in."}]}}"#,
                "2026-10-02T05:39:00Z",
            ),
        ];
        let text = render(&session, &messages);
        assert!(text.starts_with(
            "# Price filter\n\nA Conductor chat with Codex (gpt-5), started 2026-10-02 05:37 UTC."
        ));
        assert!(text.contains("## You · 2026-10-02 05:37 UTC\n\nAdd a price filter\n"));
        assert!(text.contains("## Codex · 2026-10-02 05:38 UTC\n\n- *Bash*: `rg price…`"));
        assert!(
            text.contains("- *Bash*: `rg price…`\n\nDone: the filter is in.\n"),
            "{text}"
        );
        assert_eq!(text.matches("## Codex").count(), 1, "{text}");
        for hidden in ["secret", "output", "subagent", "init"] {
            assert!(!text.contains(hidden), "{hidden} shown in {text}");
        }
    }

    #[test]
    fn chats_of_the_same_title_get_files_of_their_own() {
        let mut taken = HashSet::new();
        assert_eq!(file_name("Fix: a/b", &mut taken), "Fix- a-b.md");
        assert_eq!(file_name("fix- A-B", &mut taken), "fix- A-B (2).md");
        assert_eq!(file_name("  ", &mut taken), "Untitled.md");
        assert_eq!(file_name("..hidden", &mut taken), "hidden.md");
    }
}
