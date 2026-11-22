//! Where each session lands, from the live branches it left (docs/adr/claude-code-import.md). A
//! live branch is a workspace, the same branch in several repos one workspace, its sessions its
//! chats. A session with two live branches or more is a parent workspace on its folder's repo,
//! holding it, with one child per branch; a branch two such sessions wrote goes to the most
//! recent one, the other keeping the rest (one branch: a chat of its workspace; none: its repo's
//! history). A clone's uncommitted default-branch changes go to a workspace of their own, or to
//! the most recent parent on that repo. A session with no live branch lands in its repo's history.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chauffeur_protocol::claudecode::{
    ClaudeCodeHistoryPlan, ClaudeCodeOutcome, ClaudeCodeWorkspaceKind, ClaudeCodeWorkspacePlan,
};

use crate::slug::slugify;

/// A session to place: its folder's repo and the live branches it left, by repo.
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub last_active: String,
    pub repo: String,
    pub live: Vec<(String, String)>,
}

#[derive(Debug, Default)]
pub struct Plan {
    pub workspaces: Vec<ClaudeCodeWorkspacePlan>,
    pub histories: Vec<ClaudeCodeHistoryPlan>,
    /// Where each placed session lands, by id.
    pub lands_in: HashMap<String, String>,
}

/// `changes`: the repos whose clone holds uncommitted changes on its default branch. `bases`:
/// each repo's default branch. `made`: the branches an earlier import made a workspace of, which
/// no new parent takes (an orchestration's other branches still count).
pub fn place(
    sessions: &[Session],
    changes: &[String],
    bases: &HashMap<String, String>,
    made: &BTreeSet<String>,
) -> Plan {
    let mut by_age: Vec<&Session> = sessions.iter().collect();
    by_age.sort_by(|a, b| a.last_active.cmp(&b.last_active));
    let names =
        |s: &Session| -> BTreeSet<String> { s.live.iter().map(|(_, b)| b.clone()).collect() };
    let mut repos_of: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for session in &by_age {
        for (repo, branch) in &session.live {
            repos_of
                .entry(branch.clone())
                .or_default()
                .insert(repo.clone());
        }
    }
    let mut taken: BTreeSet<String> = repos_of.keys().cloned().collect();
    let mut plan = Plan::default();
    // A branch two orchestrations wrote goes to the most recent; one left with two branches or
    // more is a parent, oldest first.
    let orchestrations: Vec<&Session> = by_age
        .iter()
        .copied()
        .filter(|s| names(s).len() >= 2)
        .collect();
    let mut wrote_last: HashMap<String, &str> = HashMap::new();
    for session in &orchestrations {
        for branch in names(session) {
            wrote_last.insert(branch, &session.id);
        }
    }
    // An orchestration's branches that a more recent one did not take; a plain session's all.
    let kept = |s: &Session| -> BTreeSet<String> {
        let all = names(s);
        if all.len() < 2 {
            return all;
        }
        all.into_iter()
            .filter(|b| !made.contains(b) && wrote_last.get(b).is_none_or(|id| *id == s.id))
            .collect()
    };
    let mut parents: Vec<(String, &Session)> = Vec::new();
    let mut owner: HashMap<String, String> = HashMap::new();
    for session in orchestrations.iter().filter(|s| kept(s).len() >= 2) {
        let name = unique(&mut taken, &slugify(&session.title, 48), &session.id);
        for branch in kept(session) {
            owner.insert(branch, name.clone());
        }
        plan.lands_in.insert(session.id.clone(), name.clone());
        parents.push((name, session));
    }
    let mut chats: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut latest: BTreeMap<String, String> = BTreeMap::new();
    let mut histories: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for session in &by_age {
        for branch in names(session) {
            latest.insert(branch, session.last_active.clone());
        }
        let branches = kept(session);
        match branches.len() {
            0 => {
                histories
                    .entry(session.repo.clone())
                    .or_default()
                    .push(session.id.clone());
                plan.lands_in
                    .insert(session.id.clone(), format!("history of {}", session.repo));
            }
            1 => {
                let branch = branches.into_iter().next().unwrap_or_default();
                chats
                    .entry(branch.clone())
                    .or_default()
                    .push(session.id.clone());
                plan.lands_in.insert(session.id.clone(), branch);
            }
            _ => {}
        }
    }
    let branch_workspace = |branch: &str| ClaudeCodeWorkspacePlan {
        name: branch.to_string(),
        kind: ClaudeCodeWorkspaceKind::Branch,
        branch: Some(branch.to_string()),
        repos: repos_of
            .get(branch)
            .into_iter()
            .flatten()
            .cloned()
            .collect(),
        parent: owner.get(branch).cloned(),
        sessions: chats.get(branch).cloned().unwrap_or_default(),
        changes_from: Vec::new(),
        outcome: ClaudeCodeOutcome::Import,
        reason: None,
        workspace: None,
    };
    // The most recent parent first, each followed by its children; then the other branches,
    // the most recently written first; then the changes.
    for (name, session) in parents.iter().rev() {
        plan.workspaces.push(ClaudeCodeWorkspacePlan {
            name: name.clone(),
            kind: ClaudeCodeWorkspaceKind::Parent,
            branch: None,
            repos: vec![session.repo.clone()],
            parent: None,
            sessions: vec![session.id.clone()],
            changes_from: Vec::new(),
            outcome: ClaudeCodeOutcome::Import,
            reason: None,
            workspace: None,
        });
        for branch in repos_of.keys().filter(|b| owner.get(*b) == Some(name)) {
            plan.workspaces.push(branch_workspace(branch));
        }
    }
    let mut alone: Vec<&String> = repos_of
        .keys()
        .filter(|b| !owner.contains_key(*b))
        .collect();
    alone.sort_by(|a, b| latest.get(*b).cmp(&latest.get(*a)));
    for branch in alone {
        plan.workspaces.push(branch_workspace(branch));
    }
    for repo in changes {
        let parent = parents
            .iter()
            .rev()
            .find(|(_, s)| &s.repo == repo)
            .map(|(name, _)| name.clone());
        match parent.and_then(|name| plan.workspaces.iter_mut().find(|w| w.name == name)) {
            Some(parent) => parent.changes_from.push(repo.clone()),
            None => {
                let name = unique(&mut taken, &format!("{}-changes", slugify(repo, 40)), repo);
                plan.workspaces.push(ClaudeCodeWorkspacePlan {
                    name,
                    kind: ClaudeCodeWorkspaceKind::Changes,
                    branch: None,
                    repos: vec![repo.clone()],
                    parent: None,
                    sessions: Vec::new(),
                    changes_from: vec![repo.clone()],
                    outcome: ClaudeCodeOutcome::Import,
                    reason: None,
                    workspace: None,
                });
            }
        }
    }
    plan.histories = histories
        .into_iter()
        .map(|(repo, sessions)| ClaudeCodeHistoryPlan {
            base: bases.get(&repo).cloned().unwrap_or_else(|| "main".into()),
            repo,
            sessions,
            workspace: None,
        })
        .collect();
    plan
}

/// `wanted`, or `session-<id>` when it is empty, with `-2`, `-3`… when another workspace has it.
fn unique(taken: &mut BTreeSet<String>, wanted: &str, id: &str) -> String {
    let base = if wanted.is_empty() {
        format!("session-{}", id.chars().take(8).collect::<String>())
    } else {
        wanted.to_string()
    };
    let mut name = base.clone();
    for n in 2.. {
        if !taken.contains(&name) {
            break;
        }
        name = format!("{base}-{n}");
    }
    taken.insert(name.clone());
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, at: &str, repo: &str, live: &[(&str, &str)]) -> Session {
        Session {
            id: id.into(),
            title: format!("Session {id}"),
            last_active: format!("2026-11-{at}T00:00:00.000Z"),
            repo: repo.into(),
            live: live
                .iter()
                .map(|(r, b)| (r.to_string(), b.to_string()))
                .collect(),
        }
    }

    fn shape(plan: &Plan) -> Vec<String> {
        plan.workspaces
            .iter()
            .map(|w| {
                format!(
                    "{:?} {} on {} [{}] parent={} chats={} changes={}",
                    w.kind,
                    w.name,
                    w.branch.as_deref().unwrap_or("(new)"),
                    w.repos.join(","),
                    w.parent.as_deref().unwrap_or("-"),
                    w.sessions.join(","),
                    w.changes_from.join(","),
                )
            })
            .collect()
    }

    #[test]
    fn a_live_branch_is_a_workspace_holding_its_sessions_and_the_same_branch_in_two_repos_is_one() {
        let plan = place(
            &[
                session("a", "01", "web", &[("web", "feat/x")]),
                session("b", "03", "api", &[("api", "feat/x")]),
                session("c", "02", "web", &[("web", "fix/y")]),
            ],
            &[],
            &HashMap::new(),
            &BTreeSet::new(),
        );
        assert_eq!(
            shape(&plan),
            [
                "Branch feat/x on feat/x [api,web] parent=- chats=a,b changes=",
                "Branch fix/y on fix/y [web] parent=- chats=c changes=",
            ]
        );
        assert_eq!(plan.lands_in["b"], "feat/x");
        assert!(plan.histories.is_empty());
    }

    #[test]
    fn a_session_with_two_live_branches_is_a_parent_and_a_branch_two_parents_wrote_goes_to_the_most_recent()
     {
        let plan = place(
            &[
                session("old", "01", "web", &[("web", "a"), ("web", "b")]),
                session("new", "05", "web", &[("web", "b"), ("api", "c")]),
                session("simple", "03", "web", &[("web", "b")]),
            ],
            &[],
            &HashMap::new(),
            &BTreeSet::new(),
        );
        assert_eq!(
            shape(&plan),
            [
                "Parent session-new on (new) [web] parent=- chats=new changes=",
                "Branch b on b [web] parent=session-new chats=simple changes=",
                "Branch c on c [api] parent=session-new chats= changes=",
                "Branch a on a [web] parent=- chats=old changes=",
            ]
        );
        assert_eq!(plan.lands_in["simple"], "b");
        assert_eq!(plan.lands_in["new"], "session-new");
    }

    #[test]
    fn no_new_parent_takes_a_branch_an_earlier_import_made_a_workspace_of() {
        let plan = place(
            &[
                session(
                    "two-left",
                    "01",
                    "web",
                    &[("web", "a"), ("web", "b"), ("web", "c")],
                ),
                session("one-left", "02", "api", &[("web", "a"), ("api", "d")]),
            ],
            &[],
            &HashMap::new(),
            &BTreeSet::from(["a".to_string()]),
        );
        assert_eq!(
            shape(&plan),
            [
                "Parent session-two-left on (new) [web] parent=- chats=two-left changes=",
                "Branch b on b [web] parent=session-two-left chats= changes=",
                "Branch c on c [web] parent=session-two-left chats= changes=",
                "Branch a on a [web] parent=- chats= changes=",
                "Branch d on d [api] parent=- chats=one-left changes=",
            ]
        );
    }

    #[test]
    fn a_parent_left_one_branch_is_a_chat_of_it_and_none_its_repos_history() {
        let plan = place(
            &[
                session("one", "01", "web", &[("web", "a"), ("web", "b")]),
                session("none", "02", "api", &[("web", "b"), ("api", "c")]),
                session("new", "05", "web", &[("web", "b"), ("api", "c")]),
            ],
            &[],
            &HashMap::new(),
            &BTreeSet::new(),
        );
        assert_eq!(
            shape(&plan),
            [
                "Parent session-new on (new) [web] parent=- chats=new changes=",
                "Branch b on b [web] parent=session-new chats= changes=",
                "Branch c on c [api] parent=session-new chats= changes=",
                "Branch a on a [web] parent=- chats=one changes=",
            ]
        );
        assert_eq!(plan.lands_in["one"], "a");
        assert_eq!(plan.lands_in["none"], "history of api");
        assert_eq!(plan.histories[0].sessions, ["none"]);
    }

    #[test]
    fn a_session_with_no_live_branch_lands_in_its_repos_history() {
        let bases = HashMap::from([("web".to_string(), "staging".to_string())]);
        let plan = place(
            &[
                session("a", "01", "web", &[]),
                session("b", "02", "web", &[]),
            ],
            &[],
            &bases,
            &BTreeSet::new(),
        );
        assert!(plan.workspaces.is_empty());
        assert_eq!(plan.histories.len(), 1);
        assert_eq!(
            (
                plan.histories[0].repo.as_str(),
                plan.histories[0].base.as_str()
            ),
            ("web", "staging")
        );
        assert_eq!(plan.histories[0].sessions, ["a", "b"]);
        assert_eq!(plan.lands_in["a"], "history of web");
    }

    #[test]
    fn default_branch_changes_go_to_the_most_recent_parent_on_their_repo_else_a_workspace_of_their_own()
     {
        let plan = place(
            &[
                session("p1", "01", "web", &[("web", "a"), ("web", "b")]),
                session("p2", "02", "web", &[("web", "c"), ("web", "d")]),
            ],
            &["web".to_string(), "owner/api".to_string()],
            &HashMap::new(),
            &BTreeSet::new(),
        );
        let shapes = shape(&plan);
        assert!(
            shapes[0].starts_with("Parent session-p2") && shapes[0].ends_with("changes=web"),
            "{shapes:?}"
        );
        assert!(shapes.iter().any(|s| s == "Changes owner-api-changes on (new) [owner/api] parent=- chats= changes=owner/api"), "{shapes:?}");
        assert!(shapes.iter().filter(|s| s.ends_with("changes=web")).count() == 1);
    }

    #[test]
    fn a_parent_named_like_a_branch_or_another_parent_takes_a_number() {
        let mut first = session("one", "01", "web", &[("web", "x"), ("web", "y")]);
        first.title = "x".into();
        let mut second = session("two", "02", "web", &[("web", "z"), ("web", "w")]);
        second.title = "x".into();
        let plan = place(&[first, second], &[], &HashMap::new(), &BTreeSet::new());
        let parents: Vec<&str> = plan
            .workspaces
            .iter()
            .filter(|w| w.kind == ClaudeCodeWorkspaceKind::Parent)
            .map(|w| w.name.as_str())
            .collect();
        assert_eq!(parents, ["x-3", "x-2"]);
    }
}
