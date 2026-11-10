//! Who may call what, and where a call about a moved workspace goes. Every `ClientCall` variant
//! declares both beside it (`#[access(..)]`, `#[relay(..)]`): a method without them does not
//! compile, so none is open, or relayed, by accident.
//!
//! The private Unix socket serves the user's CLI, the mobile host, and the agents, whose sandbox
//! reaches it (the `chauffeur` command is theirs by design). The daemon tells an agent apart by its
//! process: a descendant of a sidecar the daemon started.

use crate::client::ClientCall;
use crate::cloud_transfer::TransferEndpoint;
use crate::methods::Method;

/// What a connection may call.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Access {
    /// The Unix socket, from a process that is not an agent's: everything, endpoint trust included.
    Private,
    /// The Unix socket, from a process under a sidecar: what an agent may do in a workspace.
    Agent,
    /// The TCP endpoint with the daemon's token: the app.
    Token,
    /// A browser's Office session: the methods marked `office` only.
    Office,
}

/// Who a method is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    /// Every connection.
    Any,
    /// The user's own clients, the app included, never an agent: terminals, settings, updates.
    User,
    /// The app only: a shell command run from the composer leaves an agent's sandbox.
    App,
    /// The user over the private Unix socket only: endpoint trust, devices, cloud peers.
    Owner,
}

/// What becomes of a call about a workspace that runs on another machine (docs/workspace-relay.md).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Relay {
    /// Answered here, whatever the workspace.
    Local,
    /// Sent to the machine the workspace runs on: the user acts on it.
    Act,
    /// Sent there too, but a read a client makes on its own to show the workspace, which does not
    /// keep it from being reminded of as idle.
    Read,
    /// Refused: it opens one of the workspace's files with an app of this machine, where its
    /// files are not.
    Here,
}

/// Who `call` is for: its method's scope, narrowed by its params.
pub fn scope(call: &ClientCall) -> Scope {
    match call {
        // Sockets named directly are the CLI's; apps name the machines of `[transfer]`.
        ClientCall::CloudTransferStart(p) | ClientCall::CloudTransferCancel(p)
            if p.controller.is_some()
                || matches!(p.source, TransferEndpoint::Socket { .. })
                || matches!(p.destination, TransferEndpoint::Socket { .. }) =>
        {
            Scope::Owner
        }
        _ => call.method().scope(),
    }
}

/// Refuses a call the connection may not make, saying who may.
pub fn check(access: Access, call: &ClientCall) -> Result<(), String> {
    let method = call.method().name();
    if access == Access::Office {
        // Its subscription follows the registry and the Office only, never a chat.
        let allowed = call.method().office()
            && !matches!(call, ClientCall::EventsSubscribe(p)
                if p.all || !p.workspaces.is_empty() || !p.chats.is_empty());
        return match allowed {
            true => Ok(()),
            false => Err(format!(
                "The Office in the browser may not call {method}: do it in the Chauffeur app"
            )),
        };
    }
    let refusal = match (scope(call), access) {
        (Scope::Any, _) => return Ok(()),
        (Scope::Owner, Access::Private) => return Ok(()),
        (Scope::Owner, Access::Agent) => {
            format!("An agent may not call {method}: endpoint trust is the user's")
        }
        (Scope::Owner, _) => "Endpoint trust is only available over the private Unix socket".into(),
        (Scope::User, Access::Private | Access::Token) => return Ok(()),
        (Scope::User, _) => {
            format!(
                "An agent may not call {method}: terminals, settings and updates are the user's"
            )
        }
        (Scope::App, Access::Token) => return Ok(()),
        (Scope::App, _) => "Shell commands run from the app's composer only".into(),
    };
    Err(refusal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::ClientMethod;
    use crate::client::{RemoteDeviceParams, ShellParams};
    use crate::methods::Method;
    use serde_json::json;

    fn call(method: &str, params: serde_json::Value) -> ClientCall {
        serde_json::from_value(json!({ "method": method, "params": params })).unwrap()
    }

    #[test]
    fn endpoint_trust_is_the_user_s_over_the_private_socket_only() {
        let revoke = ClientCall::RemoteRevoke(RemoteDeviceParams {
            device_id: "d".into(),
        });
        assert!(check(Access::Private, &revoke).is_ok());
        for access in [Access::Agent, Access::Token, Access::Office] {
            assert!(check(access, &revoke).is_err(), "{access:?}");
        }
        assert_eq!(
            check(Access::Agent, &revoke).unwrap_err(),
            "An agent may not call remote.revoke: endpoint trust is the user's"
        );
    }

    #[test]
    fn a_transfer_naming_a_socket_or_a_controller_is_the_user_s() {
        let to_machine = call(
            "cloud.transfer.start",
            json!({"workspace":"w","source":{"kind":"here"},"destination":{"kind":"machine","name":"vm"}}),
        );
        assert_eq!(scope(&to_machine), Scope::Any);
        let to_socket = call(
            "cloud.transfer.start",
            json!({"workspace":"w","source":{"kind":"here"},"destination":{"kind":"socket","path":"/tmp/x.sock"}}),
        );
        assert_eq!(scope(&to_socket), Scope::Owner);
        let with_controller = call(
            "cloud.transfer.cancel",
            json!({"workspace":"w","source":{"kind":"here"},"destination":{"kind":"machine","name":"vm"},"controller":"/tmp/c.sock"}),
        );
        assert_eq!(scope(&with_controller), Scope::Owner);
    }

    #[test]
    fn an_agent_keeps_its_workspace_but_not_the_user_s_terminals_settings_or_daemon() {
        for method in [
            "workspace.list",
            "chat.send",
            "process.run",
            "worktree.add",
            "files.write",
            "settings.get",
        ] {
            let params = match method {
                "chat.send" => json!({"chat":"c","text":"hi"}),
                "process.run" => json!({"workspace":"w","repo":"r"}),
                "worktree.add" => json!({"workspace":"w","repos":["r"]}),
                "files.write" => json!({"workspace":"w","path":"a","text":""}),
                _ => json!({}),
            };
            let call = call(method, params);
            assert!(check(Access::Agent, &call).is_ok(), "{method}");
        }
        for method in [
            "terminal.list",
            "settings.set",
            "slack.login",
            "slack.poll",
            "slack.cancel",
            "slack.logout",
            "slack.context",
            "slack.status",
            "slack.receive",
            "daemon.shutdown",
            "update.install",
        ] {
            let params = match method {
                "terminal.list" => json!({"workspace":"w"}),
                "slack.receive" => json!({"focused":false,"ack":null}),
                "slack.context" => json!({"link":"chauffeur://slack/test"}),
                _ => json!({}),
            };
            let call = call(method, params);
            let error = check(Access::Agent, &call).unwrap_err();
            assert!(
                error.starts_with("An agent may not call"),
                "{method}: {error}"
            );
            assert!(check(Access::Private, &call).is_ok(), "{method}");
            assert!(check(Access::Token, &call).is_ok(), "{method}");
        }
    }

    #[test]
    fn database_credentials_and_queries_are_available_only_to_the_users_clients() {
        for (method, params) in [
            ("database.list", json!({"workspace":null})),
            (
                "database.query",
                json!({"connection":"db","workspace":"w","request":"q","sql":"SELECT 1"}),
            ),
            ("database.cancel", json!({"request":"q"})),
            (
                "database.delete",
                json!({"connection":"db","workspace":"w"}),
            ),
        ] {
            let call = call(method, params);
            for access in [Access::Agent, Access::Office] {
                assert!(check(access, &call).is_err(), "{method} {access:?}");
            }
            for access in [Access::Private, Access::Token] {
                assert!(check(access, &call).is_ok(), "{method} {access:?}");
            }
        }
    }

    #[test]
    fn shell_commands_are_the_app_s_only() {
        let shell = ClientCall::ChatShell(ShellParams {
            chat: "c".into(),
            command: "id".into(),
        });
        assert!(check(Access::Token, &shell).is_ok());
        for access in [Access::Private, Access::Agent] {
            assert_eq!(
                check(access, &shell).unwrap_err(),
                "Shell commands run from the app's composer only"
            );
        }
    }

    #[test]
    fn opening_the_development_services_panel_is_a_read_and_saving_them_is_not() {
        assert_eq!(ClientMethod::ServicesDetect.relay(), Relay::Read);
        assert_eq!(ClientMethod::ServicesSave.relay(), Relay::Act);
        assert_eq!(ClientMethod::ChatSend.relay(), Relay::Act);
        assert_eq!(ClientMethod::TerminalInput.relay(), Relay::Act);
        // Its files are on the machine it runs on, not here.
        assert_eq!(ClientMethod::FilesOpen.relay(), Relay::Here);
        assert_eq!(ClientMethod::OpenApps.relay(), Relay::Local);
    }

    #[test]
    fn mobile_design_resources_are_user_only_and_other_phone_and_office_calls_are_shared() {
        for method in ClientMethod::ALL {
            if *method == ClientMethod::DesignResource {
                assert!(method.phone());
                assert!(!method.office());
                assert_eq!(method.scope(), Scope::User);
            } else if method.phone() || method.office() {
                assert_eq!(method.scope(), Scope::Any, "{method:?}");
            }
        }
    }
}
