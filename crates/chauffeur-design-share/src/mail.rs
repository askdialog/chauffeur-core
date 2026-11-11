//! The service's e-mails (docs/SPEC.md, stories 51 and 53; docs/adr/share-with-any-email.md), from
//! its sender (`--mail-from-name`, `--mail-from`):
//!
//! - An invitation, when a share names someone, whatever their address: naming the owner, replies
//!   going to the owner. Never to the owner, never to the whole team when a share is opened to
//!   it, and once per address: a share's update writes to the addresses it adds, not to those it
//!   keeps or whose role changes. It never carries a sign-in link.
//! - A sign-in link, when an invitee asks for one on the share's page (sign_in.rs).
//!
//! Each one ends with "Not expecting this? Stop these emails", whose link blocks every later
//! e-mail to the address. An owner who is not a member (`--member-domain`) sends at most
//! `INVITATIONS_A_DAY` invitations a day; what a limit or a blocked address holds back is not
//! sent, and the owner is told (`notice`). Sending never fails a share: an e-mail that does not
//! leave is logged.

use std::sync::{Arc, Mutex};

use chauffeur_protocol::client::CommentAuthor;
use chauffeur_protocol::workspace_share::GuestRole;

use crate::ses::Ses;

/// Invitations a non-member owner's shares send in 24 hours, at most.
pub const INVITATIONS_A_DAY: u32 = 50;

/// How e-mails leave.
pub enum Mailer {
    /// Not at all: development, or a service without SES.
    Off,
    Ses(Arc<Ses>),
    /// Kept in memory, for tests to read.
    Memory(Arc<Mutex<Vec<Email>>>),
}

/// One e-mail to one address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Email {
    pub to: String,
    /// Where replies go: an invitation's owner; none for a sign-in link.
    pub reply_to: Option<String>,
    pub subject: String,
    pub text: String,
    pub html: String,
}

/// What a share was opened on, as the invitation words it.
pub enum Shared {
    Design,
    /// A markdown file of a workspace (story 57).
    Document,
    /// An HTML or PDF document, viewed directly.
    File,
    Workspace(GuestRole),
}

/// The links every e-mail ends with.
pub struct Footer {
    /// Blocks every later e-mail to the address (sign_in.rs).
    pub stop: String,
    /// The site "Shared with Chauffeur" links to (`--site-url`); none leaves the words alone.
    pub site: Option<String>,
}

/// The addresses of `named` to write to: not the owner's, not in `already` (lowercase, as the
/// service keeps them).
pub fn invitees<'a>(
    owner: &str,
    named: impl IntoIterator<Item = &'a str>,
    already: &[&str],
) -> Vec<&'a str> {
    named
        .into_iter()
        .filter(|email| *email != owner && !already.contains(email))
        .collect()
}

/// What an owner's invitations come to: those to send, and those held back.
#[derive(Debug, Default, PartialEq)]
pub struct Sorted<'a> {
    pub send: Vec<&'a str>,
    /// Over the owner's daily limit.
    pub over_limit: Vec<&'a str>,
    /// Addresses that stopped Chauffeur's e-mails.
    pub blocked: Vec<&'a str>,
}

/// Sorts `invitees` by D7's rules: a blocked address gets nothing; then, `left` being how many
/// more the owner may send today (none for a member), the first ones go and the rest wait.
pub fn sort<'a>(
    invitees: Vec<&'a str>,
    blocked: impl Fn(&str) -> bool,
    left: Option<u32>,
) -> Sorted<'a> {
    let mut sorted = Sorted::default();
    for to in invitees {
        if blocked(to) {
            sorted.blocked.push(to);
        } else if left.is_some_and(|left| sorted.send.len() as u32 >= left) {
            sorted.over_limit.push(to);
        } else {
            sorted.send.push(to);
        }
    }
    sorted
}

impl Sorted<'_> {
    /// What the owner reads of what was held back; none when every invitation goes.
    pub fn notice(&self) -> Option<String> {
        let mut sentences = Vec::new();
        if !self.over_limit.is_empty() {
            sentences.push(format!(
                "The share is saved, but no invitation went to {}: Chauffeur sends at most {INVITATIONS_A_DAY} invitations a day for you. Send them the link yourself.",
                self.over_limit.join(", ")
            ));
        }
        if !self.blocked.is_empty() {
            sentences.push(format!(
                "The share is saved, but no invitation went to {}, who stopped e-mails from Chauffeur. Send them the link yourself.",
                self.blocked.join(", ")
            ));
        }
        (!sentences.is_empty()).then(|| sentences.join(" "))
    }
}

pub fn invitation(
    owner: &CommentAuthor,
    to: &str,
    title: &str,
    link: &str,
    shared: Shared,
    footer: &Footer,
) -> Email {
    let can = match shared {
        Shared::Design => "look at its screens and comment on them",
        Shared::Document => "read it and comment on its passages",
        Shared::File => "read it in your browser",
        Shared::Workspace(GuestRole::Viewer) => "follow its chats and read its notes, live",
        Shared::Workspace(GuestRole::Suggest) => {
            "follow its chats, read its notes and suggest messages to its agents"
        }
    };
    let name = &owner.name;
    let how = "Sign in with the Google account of this address, or get a sign-in link by email.";
    let text = format!(
        "{name} ({}) shared “{title}” with you on Chauffeur: you can {can}.\n\n\
         Open: {link}\n\n\
         {how}\n\n{}",
        owner.email,
        footer_text(footer),
    );
    let html = page(
        &format!(
            "<p>{name} ({email}) shared <strong>“{title}”</strong> with you on Chauffeur: you can {can}.</p>\
             {button}\
             <p style=\"color:#6e6e73;font-size:13px\">{how}</p>",
            name = escape(name),
            email = escape(&owner.email),
            title = escape(title),
            button = button("Open", link),
        ),
        footer,
    );
    Email {
        to: to.to_string(),
        reply_to: Some(owner.email.clone()),
        subject: format!("{name} shared “{title}” with you"),
        text,
        html,
    }
}

/// The e-mail that carries a sign-in link, to an invitee who asked for it on the share's page.
pub fn sign_in_link(
    owner: &CommentAuthor,
    to: &str,
    title: &str,
    link: &str,
    footer: &Footer,
) -> Email {
    let name = &owner.name;
    let when = "It signs you in as this address once, within 15 minutes. If you did not ask for it, ignore this e-mail.";
    let text = format!(
        "Open “{title}”, that {name} shared with you on Chauffeur:\n\n{link}\n\n{when}\n\n{}",
        footer_text(footer),
    );
    let html = page(
        &format!(
            "<p>Open <strong>“{title}”</strong>, that {name} shared with you on Chauffeur.</p>\
             {button}\
             <p style=\"color:#6e6e73;font-size:13px\">{when}</p>",
            name = escape(name),
            title = escape(title),
            button = button("Sign in", link),
        ),
        footer,
    );
    Email {
        to: to.to_string(),
        reply_to: None,
        subject: format!("Your sign-in link to “{title}”"),
        text,
        html,
    }
}

fn footer_text(footer: &Footer) -> String {
    format!(
        "Not expecting this? Stop these emails: {}\nShared with Chauffeur{}\n",
        footer.stop,
        footer
            .site
            .as_deref()
            .map(|site| format!(": {site}"))
            .unwrap_or_default()
    )
}

fn button(label: &str, link: &str) -> String {
    format!(
        "<p><a href=\"{link}\" style=\"display:inline-block;padding:10px 18px;border-radius:8px;background:#1d1d1f;color:#ffffff;text-decoration:none\">{label}</a></p>",
        link = escape(link),
    )
}

fn page(body: &str, footer: &Footer) -> String {
    let shared = match &footer.site {
        Some(site) => format!(
            "<a href=\"{}\" style=\"color:#6e6e73\">Shared with Chauffeur</a>",
            escape(site)
        ),
        None => "Shared with Chauffeur".into(),
    };
    format!(
        "<!doctype html><html><body style=\"font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;color:#1d1d1f;line-height:1.5\">\
         {body}\
         <hr style=\"border:none;border-top:1px solid #e5e5ea;margin:24px 0 12px\">\
         <p style=\"color:#6e6e73;font-size:12px\">Not expecting this? <a href=\"{stop}\" style=\"color:#6e6e73\">Stop these emails</a><br>{shared}</p>\
         </body></html>",
        stop = escape(&footer.stop),
    )
}

pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

impl Mailer {
    /// Hands the e-mails over and returns at once; SES sends them one after the other.
    pub fn send(&self, emails: Vec<Email>) {
        if emails.is_empty() {
            return;
        }
        match self {
            Mailer::Off => {}
            Mailer::Memory(sent) => sent.lock().unwrap().extend(emails),
            Mailer::Ses(ses) => {
                let ses = ses.clone();
                tokio::spawn(async move {
                    for email in emails {
                        match ses.send(&email).await {
                            Ok(()) => tracing::info!(to = email.to, "e-mail sent"),
                            Err(error) => tracing::warn!(
                                error = format!("{error:#}"),
                                to = email.to,
                                "could not send an e-mail"
                            ),
                        }
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn carol() -> CommentAuthor {
        CommentAuthor {
            name: "Carol".into(),
            email: "carol@example.com".into(),
        }
    }

    fn footer() -> Footer {
        Footer {
            stop: "https://share.example.com/v1/stop-emails?to=a&sig=b".into(),
            site: Some("https://chauffeur.example.com".into()),
        }
    }

    #[test]
    fn new_invitees_of_any_domain_are_invited_never_the_owner() {
        assert_eq!(
            invitees(
                "carol@example.com",
                [
                    "alice@example.com",
                    "carol@example.com",
                    "bob@example.com",
                    "someone@gmail.com",
                ],
                &["bob@example.com"],
            ),
            vec!["alice@example.com", "someone@gmail.com"]
        );
    }

    #[test]
    fn a_blocked_address_and_those_over_the_day_s_limit_are_held_back_and_told() {
        let sorted = sort(
            vec!["a@x.com", "b@x.com", "c@x.com", "d@x.com"],
            |to| to == "b@x.com",
            Some(2),
        );
        assert_eq!(
            sorted,
            Sorted {
                send: vec!["a@x.com", "c@x.com"],
                over_limit: vec!["d@x.com"],
                blocked: vec!["b@x.com"],
            }
        );
        let notice = sorted.notice().unwrap();
        assert!(notice.contains(
            "no invitation went to d@x.com: Chauffeur sends at most 50 invitations a day"
        ));
        assert!(notice.contains("b@x.com, who stopped e-mails from Chauffeur"));
        let member = sort(vec!["a@x.com", "c@x.com"], |_| false, None);
        assert_eq!(member.send.len(), 2);
        assert_eq!(member.notice(), None);
        assert_eq!(sort(vec!["a@x.com"], |_| false, Some(0)).send.len(), 0);
    }

    #[test]
    fn an_invitation_names_the_owner_the_share_how_to_sign_in_and_how_to_stop() {
        let invitation = invitation(
            &carol(),
            "alice@gmail.com",
            "Cut <the> project",
            "https://share.example.com/w/abc",
            Shared::Workspace(GuestRole::Suggest),
            &footer(),
        );
        assert_eq!(invitation.to, "alice@gmail.com");
        assert_eq!(invitation.reply_to.as_deref(), Some("carol@example.com"));
        assert_eq!(
            invitation.subject,
            "Carol shared “Cut <the> project” with you"
        );
        assert!(invitation.text.contains("suggest messages to its agents"));
        assert!(
            invitation
                .text
                .contains("Open: https://share.example.com/w/abc")
        );
        assert!(invitation.text.contains(
            "Sign in with the Google account of this address, or get a sign-in link by email."
        ));
        assert!(
            invitation
                .text
                .contains("Not expecting this? Stop these emails: https://share.example.com/v1/stop-emails?to=a&sig=b")
        );
        assert!(
            invitation
                .text
                .contains("Shared with Chauffeur: https://chauffeur.example.com")
        );
        assert!(invitation.html.contains("“Cut &lt;the&gt; project”"));
        assert!(!invitation.html.contains("<the>"));
        assert!(invitation.html.contains(">Open</a>"));
        assert!(invitation.html.contains(">Stop these emails</a>"));
        assert!(
            invitation
                .html
                .contains("href=\"https://chauffeur.example.com\" style=\"color:#6e6e73\">Shared with Chauffeur</a>")
        );

        let design = super::invitation(
            &carol(),
            "alice@example.com",
            "CTA",
            "https://share.example.com/s/abc",
            Shared::Design,
            &Footer {
                stop: "https://s/stop".into(),
                site: None,
            },
        );
        assert_eq!(design.subject, "Carol shared “CTA” with you");
        assert!(design.text.contains("comment on them"));
        assert!(design.text.contains("Shared with Chauffeur\n"));
    }

    #[test]
    fn a_sign_in_link_e_mail_carries_the_link_and_no_reply_address() {
        let email = sign_in_link(
            &carol(),
            "bob@gmail.com",
            "CTA",
            "https://share.example.com/s/abc#sign-in=t",
            &footer(),
        );
        assert_eq!(email.reply_to, None);
        assert_eq!(email.subject, "Your sign-in link to “CTA”");
        assert!(
            email
                .text
                .contains("https://share.example.com/s/abc#sign-in=t")
        );
        assert!(email.text.contains("within 15 minutes"));
        assert!(email.html.contains(">Sign in</a>"));
        assert!(email.html.contains("Stop these emails"));
    }
}
