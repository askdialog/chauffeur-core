//! Short-lived capabilities for a single document download; the daemon token stays in the app.
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::{Error, Result};

struct Ticket {
    workspace: String,
    path: String,
    expires: Instant,
}

#[derive(Default)]
pub struct Downloads(Mutex<HashMap<String, Ticket>>);

impl Downloads {
    pub fn issue(&self, workspace: &str, path: &str) -> Result<String> {
        let mut tickets = self.0.lock().unwrap();
        let now = Instant::now();
        tickets.retain(|_, ticket| ticket.expires > now);
        if tickets.len() >= 256 {
            return Err(Error::Unavailable(
                "too many pending downloads: try again in one minute".into(),
            ));
        }
        let bytes: [u8; 32] = rand::random();
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        tickets.insert(
            token.clone(),
            Ticket {
                workspace: workspace.into(),
                path: path.into(),
                expires: now + Duration::from_secs(60),
            },
        );
        Ok(token)
    }

    pub fn take(&self, token: &str, workspace: &str) -> Option<String> {
        self.0
            .lock()
            .unwrap()
            .remove(token)
            .filter(|ticket| ticket.workspace == workspace && ticket.expires > Instant::now())
            .map(|ticket| ticket.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_tickets_are_single_use_scoped_and_expire() {
        let downloads = Downloads::default();
        let token = downloads.issue("w", "report.html").unwrap();
        assert_eq!(downloads.take(&token, "w").as_deref(), Some("report.html"));
        assert!(downloads.take(&token, "w").is_none());
        let token = downloads.issue("w", "report.html").unwrap();
        assert!(downloads.take(&token, "other").is_none());
        let token = downloads.issue("w", "report.html").unwrap();
        downloads.0.lock().unwrap().get_mut(&token).unwrap().expires = Instant::now();
        assert!(downloads.take(&token, "w").is_none());
    }
}
