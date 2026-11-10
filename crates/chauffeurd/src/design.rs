//! Design chats (story 47): their folder in the notes, what their agent is told, and the text a
//! design brief is written from.

use std::path::Path;

use chauffeur_protocol::events::ChatEvent;
use chauffeur_protocol::model::AgentKind;
use serde_json::Value;

use crate::paths::{DESIGNS_DIR, NOTES_DIR};

/// How much of a chat a design brief is written from, or a fork into the other agent hands its
/// agent, in characters.
pub const CONVERSATION_BUDGET: usize = 150_000;

/// A tool call's line: its name and what it named (a path, a command), never its content.
const TOOL_DETAIL_CHARS: usize = 200;

/// A design chat's folder, `<workspace>/.chauffeur/designs/<chat id>`, created if missing, never
/// through a symlink (it would be made wherever that points).
pub fn design_dir(workspace_path: &str, chat_id: &str) -> std::io::Result<String> {
    if chat_id.is_empty() || chat_id.contains(['/', '\\', '\0']) || chat_id.starts_with('.') {
        return Err(std::io::Error::other(format!(
            "{chat_id:?} cannot name a folder"
        )));
    }
    let notes = Path::new(workspace_path).join(NOTES_DIR);
    let designs = notes.join(DESIGNS_DIR);
    let dir = designs.join(chat_id);
    for path in [&notes, &designs, &dir] {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(std::io::Error::other(format!(
                    "{} is not a folder",
                    path.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(path)?
            }
            Err(error) => return Err(error),
        }
    }
    Ok(dir.to_string_lossy().into_owned())
}

/// A design folder copied into another, as a share carries it (served kinds, no exports, no
/// dotfiles), with its comments, none sent: the new chat's agent has read nothing.
pub fn copy(from: &Path, to: &Path) -> crate::Result<()> {
    let failed = |path: &Path, error: std::io::Error| {
        crate::Error::Internal(anyhow::anyhow!("copying {}: {error}", path.display()))
    };
    for relative in crate::share::scan(from).keys() {
        let target = to.join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| failed(parent, e))?;
        }
        std::fs::copy(from.join(relative), &target).map_err(|e| failed(&target, e))?;
    }
    crate::comments::write_imported(to, crate::comments::read(from).0)
}

/// What a design chat's agent is told, after Claude Code's own prompt. `headless-browser` is the
/// MCP server the sidecar gives every design chat (`DESIGN_BROWSER`).
pub fn system_prompt(dir: &str, agent: AgentKind) -> String {
    // Claude's Artifact tool publishes to claude.ai; Codex has none.
    let artifact = match agent {
        AgentKind::Claude => {
            " Publish a screen to claude.ai with the Artifact tool only when the user asks for it."
        }
        AgentKind::Codex => "",
    };
    format!(
        "This chat is a Chauffeur design chat: you design screens, you never write or change code.\n\
\n\
Read the repos first (any DESIGN.md, the design tokens, the existing components and screens) and design with what the product already has.\n\
\n\
Write each screen as one self-contained HTML file in your design folder, {dir} (your working directory); shared CSS or JS files can sit beside them. External resources load over https only.\n\
\n\
Lay the canvas out in canvas.json, in that folder:\n\
{{\"pages\": [{{\"name\": \"Page 1\", \"sections\": [{{\"title\": \"Sign in — states\", \"screens\": [{{\"file\": \"sign-in/empty.html\", \"label\": \"Empty\", \"width\": 390, \"height\": 844}}]}}]}}]}}\n\
`file` is relative to the folder. A screen without a size is 390 × 844. Usual sizes: mobile 390 × 844, tablet 834 × 1194, desktop 1440 × 900. A section groups the states of one view or flow: design every state of a view (loading, empty, error, success, partial data), not only the one where everything works.\n\
\n\
The user sees your screens on the canvas beside this chat, reloaded as you write them. To check a render yourself (a size, a state, an overflow), use the headless-browser MCP tools: they open no window, take file:// URLs of your folder, and return screenshots. Open a browser the user sees (another browser MCP server, if there is one) only to show them what the canvas cannot, such as an interaction or an animation, and say so when you do.\n\
\n\
Only files of that folder can be written; commits, pushes and worktrees are refused. When the design is ready, the user sends it to a coding chat that implements it: never offer to write the code yourself.{artifact}\n\
\n\
{comments}",
        comments = crate::comments::system_prompt()
    )
}

/// A chat's conversation as text, which a design brief is written from and a fork into the other
/// agent hands its agent: the user's messages, the main agent's texts, and one line per tool call
/// (its name and the path or command it named).
/// Never a file's content, thinking or a tool's output, nor a subagent's work. When it is longer
/// than `budget`, the first message (the ticket) is kept whole and the end fills the rest.
pub fn conversation_text(events: &[ChatEvent], budget: usize) -> String {
    let mut blocks: Vec<String> = Vec::new();
    for event in events {
        match event {
            ChatEvent::UserMessage { text, .. } if !text.trim().is_empty() => {
                blocks.push(format!("[Developer]\n{}", text.trim()));
            }
            ChatEvent::AssistantText {
                text,
                parent_tool_use_id: None,
                ..
            } if !text.trim().is_empty() => {
                blocks.push(format!("[Agent]\n{}", text.trim()));
            }
            ChatEvent::ToolCall {
                name,
                input,
                parent_tool_use_id: None,
                ..
            } => blocks.push(match tool_detail(input) {
                Some(detail) => format!("[Agent tool] {name}: {detail}"),
                None => format!("[Agent tool] {name}"),
            }),
            _ => {}
        }
    }
    fit(blocks, budget)
}

/// What a tool call named: its file, path, command, pattern or URL, on one line.
pub(crate) fn tool_detail(input: &Value) -> Option<String> {
    let named = [
        "file_path",
        "path",
        "command",
        "pattern",
        "url",
        "description",
    ]
    .iter()
    .find_map(|key| input.get(key).and_then(Value::as_str))?;
    let line = named.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        return None;
    }
    let mut detail: String = line.chars().take(TOOL_DETAIL_CHARS).collect();
    if line.chars().count() > TOOL_DETAIL_CHARS || named.lines().count() > 1 {
        detail.push('…');
    }
    Some(detail)
}

fn fit(blocks: Vec<String>, budget: usize) -> String {
    const SEPARATOR: &str = "\n\n";
    let size = |block: &String| block.chars().count() + SEPARATOR.len();
    let total: usize = blocks.iter().map(size).sum();
    if total <= budget || blocks.len() < 2 {
        return blocks.join(SEPARATOR);
    }
    let first = &blocks[0];
    let mut room = budget.saturating_sub(size(first));
    let mut tail: Vec<&String> = Vec::new();
    for block in blocks[1..].iter().rev() {
        let cost = size(block);
        if cost > room {
            break;
        }
        room -= cost;
        tail.push(block);
    }
    tail.reverse();
    let left_out = blocks.len() - 1 - tail.len();
    let mut out = vec![first.clone()];
    if left_out > 0 {
        out.push(format!("[… {left_out} earlier entries left out …]"));
    }
    out.extend(tail.into_iter().cloned());
    out.join(SEPARATOR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user(text: &str) -> ChatEvent {
        ChatEvent::UserMessage {
            text: text.into(),
            queued: false,
            attachments: None,
            message_id: None,
            suggested_by: None,
            delegated_answer: None,
        }
    }

    fn agent(text: &str) -> ChatEvent {
        ChatEvent::AssistantText {
            message_id: "m".into(),
            text: text.into(),
            parent_tool_use_id: None,
        }
    }

    fn tool(name: &str, input: Value, parent: Option<&str>) -> ChatEvent {
        ChatEvent::ToolCall {
            tool_use_id: "t".into(),
            name: name.into(),
            input,
            parent_tool_use_id: parent.map(str::to_string),
        }
    }

    #[test]
    fn the_agent_checks_its_renders_without_a_window_and_shows_one_only_on_purpose() {
        for agent in [AgentKind::Claude, AgentKind::Codex] {
            let prompt = system_prompt("/w/.chauffeur/designs/c1", agent);
            assert!(prompt.contains("use the headless-browser MCP tools: they open no window"));
            assert!(prompt.contains("Open a browser the user sees"));
        }
    }

    #[test]
    fn only_claude_is_told_about_the_artifact_tool_codex_has_none() {
        assert!(system_prompt("/d", AgentKind::Claude).contains("Artifact tool"));
        assert!(!system_prompt("/d", AgentKind::Codex).contains("Artifact"));
    }

    #[test]
    fn the_conversation_keeps_messages_and_names_tool_calls_without_their_content() {
        let events = vec![
            user("Add a reconnect button"),
            ChatEvent::Thinking {
                message_id: "m".into(),
                text: "secret thoughts".into(),
                parent_tool_use_id: None,
            },
            tool(
                "Write",
                json!({"file_path": "app/src/Login.tsx", "content": "export const X = 1;"}),
                None,
            ),
            tool("Bash", json!({"command": "bun test\nrm -rf /"}), None),
            tool(
                "Read",
                json!({"file_path": "sub/agent.ts"}),
                Some("toolu_1"),
            ),
            ChatEvent::AssistantText {
                message_id: "s".into(),
                text: "a subagent's words".into(),
                parent_tool_use_id: Some("toolu_1".into()),
            },
            agent("Done: the button reconnects."),
        ];
        let text = conversation_text(&events, CONVERSATION_BUDGET);
        assert_eq!(
            text,
            "[Developer]\nAdd a reconnect button\n\n[Agent tool] Write: app/src/Login.tsx\n\n[Agent tool] Bash: bun test…\n\n[Agent]\nDone: the button reconnects."
        );
        assert!(!text.contains("export const"));
        assert!(!text.contains("secret"));
    }

    #[test]
    fn a_long_conversation_keeps_its_first_message_and_its_end() {
        let mut events = vec![user("TICKET: the whole ticket")];
        for i in 0..100 {
            events.push(agent(&format!("answer {i:03} {}", "x".repeat(50))));
        }
        let text = conversation_text(&events, 1_000);
        assert!(text.starts_with("[Developer]\nTICKET: the whole ticket\n\n[… "));
        assert!(text.contains("earlier entries left out"));
        assert!(text.ends_with(&format!("answer 099 {}", "x".repeat(50))));
        assert!(!text.contains("answer 000"));
        assert!(text.chars().count() <= 1_100);
    }

    #[test]
    fn the_design_folder_is_made_under_the_notes_and_never_through_a_symlink() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().to_string_lossy().into_owned();
        let dir = design_dir(&workspace, "chat-1").unwrap();
        assert!(dir.ends_with(".chauffeur/designs/chat-1"));
        assert!(Path::new(&dir).is_dir());
        assert!(design_dir(&workspace, "../x").is_err());

        let other = tempfile::tempdir().unwrap();
        let linked = tempfile::tempdir().unwrap();
        std::fs::create_dir(linked.path().join(NOTES_DIR)).unwrap();
        std::os::unix::fs::symlink(
            other.path(),
            linked.path().join(NOTES_DIR).join(DESIGNS_DIR),
        )
        .unwrap();
        assert!(design_dir(&linked.path().to_string_lossy(), "chat-1").is_err());
        assert!(!other.path().join("chat-1").exists());
    }
}
