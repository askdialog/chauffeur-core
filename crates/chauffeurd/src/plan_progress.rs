//! How far a chat's agent is through its own plan, read from its events: Codex's plan
//! (`plan_updated`), Claude's `update_plan` (Chauffeur's tool, sidecar/src/agents/plan-tool.ts),
//! and Claude Code's own todo list (`TodoWrite`) or tasks (`TaskCreate`, `TaskUpdate`) when a
//! session has them. Only the counts are kept; an agent that keeps no plan has no progress.

use std::collections::HashSet;

use chauffeur_protocol::events::{ChatEvent, PlanStepStatus};
use chauffeur_protocol::model::ChatProgress;
use serde_json::Value;

/// What a chat's Claude Code tasks were made into so far in its session.
#[derive(Default)]
pub struct Tasks {
    created: u32,
    completed: HashSet<String>,
    deleted: HashSet<String>,
}

impl Tasks {
    fn progress(&self) -> Option<ChatProgress> {
        let total = self.created.saturating_sub(self.deleted.len() as u32);
        (total > 0).then(|| ChatProgress {
            done: (self.completed.len() as u32).min(total),
            total,
        })
    }
}

/// The chat's progress after `event`: `None` when the event says nothing of it, `Some(None)` when
/// the plan was emptied.
pub fn after(event: &ChatEvent, tasks: &mut Tasks) -> Option<Option<ChatProgress>> {
    match event {
        ChatEvent::PlanUpdated { steps } => Some(counted(
            steps.iter().map(|s| s.status == PlanStepStatus::Completed),
        )),
        // A subagent's own list is not the chat's plan.
        ChatEvent::ToolCall {
            parent_tool_use_id: Some(_),
            ..
        } => None,
        ChatEvent::ToolCall { name, input, .. } => match name.as_str() {
            "mcp__chauffeur__update_plan" => {
                let steps = input.get("steps")?.as_array()?;
                Some(counted(steps.iter().map(|step| {
                    step.get("status").and_then(Value::as_str) == Some("completed")
                })))
            }
            "TodoWrite" => {
                let todos = input.get("todos")?.as_array()?;
                Some(counted(todos.iter().map(|todo| {
                    todo.get("status").and_then(Value::as_str) == Some("completed")
                })))
            }
            "TaskCreate" => {
                tasks.created += 1;
                Some(tasks.progress())
            }
            "TaskUpdate" => {
                let id = match input.get("taskId")? {
                    Value::String(id) => id.clone(),
                    Value::Number(id) => id.to_string(),
                    _ => return None,
                };
                match input.get("status").and_then(Value::as_str)? {
                    "completed" => {
                        tasks.completed.insert(id);
                    }
                    "deleted" => {
                        tasks.completed.remove(&id);
                        tasks.deleted.insert(id);
                    }
                    _ => {
                        tasks.completed.remove(&id);
                    }
                }
                Some(tasks.progress())
            }
            _ => None,
        },
        _ => None,
    }
}

fn counted(done: impl Iterator<Item = bool>) -> Option<ChatProgress> {
    let (mut total, mut finished) = (0, 0);
    for step in done {
        total += 1;
        finished += u32::from(step);
    }
    (total > 0).then_some(ChatProgress {
        done: finished,
        total,
    })
}

/// `3/7`, as kept between daemon runs.
pub fn encode(progress: ChatProgress) -> String {
    format!("{}/{}", progress.done, progress.total)
}

pub fn decode(text: &str) -> Option<ChatProgress> {
    let (done, total) = text.split_once('/')?;
    Some(ChatProgress {
        done: done.parse().ok()?,
        total: total.parse().ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chauffeur_protocol::events::PlanStep;
    use serde_json::json;

    fn call(name: &str, input: Value) -> ChatEvent {
        ChatEvent::ToolCall {
            tool_use_id: "t".into(),
            name: name.into(),
            input,
            parent_tool_use_id: None,
        }
    }

    fn of(done: u32, total: u32) -> Option<Option<ChatProgress>> {
        Some(Some(ChatProgress { done, total }))
    }

    #[test]
    fn a_codex_plan_counts_its_completed_steps() {
        let step = |status| PlanStep {
            text: "x".into(),
            status,
        };
        let mut tasks = Tasks::default();
        let plan = ChatEvent::PlanUpdated {
            steps: vec![
                step(PlanStepStatus::Completed),
                step(PlanStepStatus::InProgress),
                step(PlanStepStatus::Pending),
            ],
        };
        assert_eq!(after(&plan, &mut tasks), of(1, 3));
        let emptied = ChatEvent::PlanUpdated { steps: vec![] };
        assert_eq!(after(&emptied, &mut tasks), Some(None));
    }

    #[test]
    fn claudes_update_plan_counts_its_completed_steps() {
        let plan = call(
            "mcp__chauffeur__update_plan",
            json!({"steps": [
                {"text": "a", "status": "completed"},
                {"text": "b", "status": "in_progress"}
            ]}),
        );
        assert_eq!(after(&plan, &mut Tasks::default()), of(1, 2));
    }

    #[test]
    fn claude_todos_count_the_completed_ones() {
        let todos = call(
            "TodoWrite",
            json!({"todos": [
                {"content": "a", "status": "completed", "activeForm": "A"},
                {"content": "b", "status": "completed", "activeForm": "B"},
                {"content": "c", "status": "in_progress", "activeForm": "C"},
                {"content": "d", "status": "pending", "activeForm": "D"}
            ]}),
        );
        assert_eq!(after(&todos, &mut Tasks::default()), of(2, 4));
    }

    #[test]
    fn claude_tasks_add_up_as_they_are_created_completed_and_deleted() {
        let mut tasks = Tasks::default();
        for _ in 0..3 {
            after(&call("TaskCreate", json!({"subject": "s"})), &mut tasks);
        }
        assert_eq!(
            after(
                &call("TaskUpdate", json!({"taskId": "1", "status": "completed"})),
                &mut tasks
            ),
            of(1, 3)
        );
        assert_eq!(
            after(
                &call("TaskUpdate", json!({"taskId": 3, "status": "deleted"})),
                &mut tasks
            ),
            of(1, 2)
        );
        assert_eq!(
            after(
                &call(
                    "TaskUpdate",
                    json!({"taskId": "1", "status": "in_progress"})
                ),
                &mut tasks
            ),
            of(0, 2)
        );
    }

    #[test]
    fn other_events_and_tools_say_nothing() {
        let mut tasks = Tasks::default();
        let mut subagent = call("TodoWrite", json!({"todos": [{"status": "completed"}]}));
        if let ChatEvent::ToolCall {
            parent_tool_use_id, ..
        } = &mut subagent
        {
            *parent_tool_use_id = Some("p".into());
        }
        assert_eq!(after(&subagent, &mut tasks), None, "a subagent's todos");
        assert_eq!(
            after(&call("Bash", json!({"command": "ls"})), &mut tasks),
            None
        );
        assert_eq!(after(&ChatEvent::TurnStarted, &mut tasks), None);
    }

    #[test]
    fn progress_is_kept_as_done_over_total() {
        let progress = ChatProgress { done: 3, total: 7 };
        assert_eq!(encode(progress), "3/7");
        assert_eq!(decode("3/7"), Some(progress));
        assert_eq!(decode("x"), None);
    }
}
