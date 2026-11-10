use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The user's Chauffeur sign-in (Google), and what it gives access to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct IdentityState {
    /// The signed-in account's e-mail; none while signed out.
    pub email: Option<String>,
    /// The account's Google Workspace domain (its ID token's `hd`), the team domain a share can be
    /// opened to; none for a personal Google account, while signed out, or until it is known.
    #[serde(default)]
    pub team_domain: Option<String>,
    /// A member: signed in with an account of the distribution's `[distribution] member_domain`.
    #[serde(default)]
    pub member: bool,
    /// Signing in can work here: a Google client is configured, or a development e-mail.
    #[serde(default)]
    pub can_sign_in: bool,
}
