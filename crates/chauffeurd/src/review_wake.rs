//! An owned, one-off macOS wake event. Never replaces the user's repeating power schedule.
use crate::{Daemon, Error, Result, power};
use chauffeur_protocol::{client::DailyReviewSettings, model::KeepAwake};
use chrono::{DateTime, Local, Timelike};
use std::{process::Command, sync::Arc};

const KEY: &str = "daily_review.wake_at";
const RULE: &str = "chauffeur-review-wake";
const OWNER: &str = "com.askdialog.chauffeur.daily-review";

fn rule(user: &str) -> std::result::Result<String, String> {
    power::sudoers_rule(user)?; // Share the validated account-name grammar.
    // Each date character is constrained; no wildcard can introduce extra pmset arguments.
    let date = "[0-9][0-9]/[0-9][0-9]/[0-9][0-9] [0-9][0-9]\\:[0-9][0-9]\\:[0-9][0-9]";
    Ok(format!(
        "{user} ALL=(root) NOPASSWD: {} schedule wake {date} {OWNER}, {} schedule cancel wake {date} {OWNER}",
        power::PMSET,
        power::PMSET
    ))
}

pub async fn configure(d: &Arc<Daemon>, review: &DailyReviewSettings) -> Result<()> {
    if review.wake && !cfg!(target_os = "macos") {
        return Err(Error::BadRequest("Scheduled wake is available on macOS only; the review runs after wake on this computer".into()));
    }
    if review.wake && !std::path::Path::new("/etc/sudoers.d").join(RULE).exists() {
        tokio::task::spawn_blocking(|| -> Result<()> {
            let user = power::current_user()
                .ok_or_else(|| Error::Unavailable("could not tell who the user is".into()))?;
            let rule = rule(&user).map_err(Error::BadRequest)?;
            let script = power::install_named_rule(
                &power::Tools::system(),
                &rule,
                RULE,
                ".chauffeur-review-wake.new",
            );
            let apple = format!(
                "do shell script {} with prompt {} with administrator privileges",
                power::applescript_string(&script),
                power::applescript_string(
                    "Allow Chauffeur to schedule a wake at 18:00 for your daily session review."
                )
            );
            let output = Command::new("/usr/bin/osascript")
                .args(["-e", &apple])
                .output()
                .map_err(anyhow::Error::from)?;
            if !output.status.success() {
                return Err(Error::BadRequest(format!(
                    "Scheduled wake was not enabled: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )));
            }
            Ok(())
        })
        .await
        .map_err(anyhow::Error::from)??;
    }
    // Disabling takes effect while the rule still exists. The narrowly scoped rule can remain
    // installed, like login permissions; turning wake back on does not need another prompt.
    if !review.wake || !review.enabled {
        let d = d.clone();
        tokio::task::spawn_blocking(move || cancel(&d))
            .await
            .map_err(anyhow::Error::from)??;
    }
    Ok(())
}

fn pmset(d: &Daemon, args: &[&str]) -> Result<()> {
    let output = Command::new(d.power.tools().sudo)
        .args(["-n", power::PMSET])
        .args(args)
        .output()
        .map_err(anyhow::Error::from)?;
    if !output.status.success() {
        return Err(Error::Unavailable(format!(
            "Could not schedule the daily review wake: {}; enable scheduled wake in Settings > Sleep",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

fn cancel(d: &Daemon) -> Result<()> {
    if let Some(at) = d.store.kv_get(KEY)? {
        let future = chrono::NaiveDateTime::parse_from_str(&at, "%m/%d/%y %H:%M:%S")
            .is_ok_and(|time| time > Local::now().naive_local());
        if future {
            pmset(d, &["schedule", "cancel", "wake", &at, OWNER])?;
        }
        d.store.kv_delete(KEY)?;
    }
    Ok(())
}

pub async fn sync(
    d: &Arc<Daemon>,
    review: &DailyReviewSettings,
    now: DateTime<Local>,
) -> Result<()> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    let settings = d.settings.get();
    let (start, end) = crate::daily_review::bounds(now.date_naive())?;
    let active = review.enabled
        && review.wake
        && settings.keep_awake != KeepAwake::Never
        && now.hour() < 18
        && !d.store.review_chats(&start, &end)?.is_empty();
    let d = d.clone();
    tokio::task::spawn_blocking(move || {
        let low = active
            && power::battery_low(
                power::read_battery(&d.power.tools()),
                settings.keep_awake_min_battery,
            );
        if !active || low {
            return cancel(&d);
        }
        let at = format!("{} 18:00:00", now.format("%m/%d/%y"));
        if d.store.kv_get(KEY)?.as_deref() == Some(&at) {
            return Ok(());
        }
        cancel(&d)?;
        // Persist first: if the process dies after pmset, the next start still owns this event.
        d.store.kv_set(KEY, &at)?;
        if let Err(error) = pmset(&d, &["schedule", "wake", &at, OWNER]) {
            d.store.kv_delete(KEY)?;
            return Err(error);
        }
        Ok(())
    })
    .await
    .map_err(anyhow::Error::from)?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wake_permission_cannot_schedule_shutdown_or_replace_other_schedules() {
        let permission = rule("bob").unwrap();
        assert!(permission.contains("schedule cancel wake"));
        assert!(!permission.contains('*'));
        assert!(!permission.contains("repeat"));
        assert!(!permission.contains("shutdown"));
        assert!(rule("bob ALL=(ALL)").is_err());
    }
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn wake_is_owned_idempotent_and_cancelled_when_disabled_or_sleep_is_allowed() {
        use chrono::{Days, TimeZone};
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let paths = crate::paths::Paths::new(
            root.join("home"),
            root.join("workspaces"),
            root.join("repos"),
        );
        paths.ensure().unwrap();
        std::fs::write(&paths.settings, "linear_enabled = false\n").unwrap();
        let d = Daemon::open(paths).unwrap();
        let log = root.join("calls");
        let sudo = root.join("sudo");
        std::fs::write(
            &sudo,
            format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n", log.display()),
        )
        .unwrap();
        std::fs::set_permissions(&sudo, std::fs::Permissions::from_mode(0o755)).unwrap();
        let battery = root.join("pmset");
        std::fs::write(
            &battery,
            "#!/bin/sh\necho \"Now drawing from 'AC Power'\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&battery, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut tools = power::Tools::system();
        tools.sudo = sudo;
        tools.pmset = battery.clone();
        d.power.set_tools(tools);
        let day = Local::now().date_naive() + Days::new(1);
        let now = Local
            .from_local_datetime(&day.and_hms_opt(12, 0, 0).unwrap())
            .earliest()
            .unwrap();
        let (start, _) = crate::daily_review::bounds(day).unwrap();
        d.store.with(|c| c.execute_batch("INSERT INTO workspaces(id,slug,path,branch,state,created_at) VALUES ('w','w','/tmp/w','','ready','old');
            INSERT INTO chats(id,workspace_id,agent,permission_mode,status,created_at) VALUES ('c','w','claude','auto','idle','old');")).unwrap();
        let review = DailyReviewSettings {
            wake: true,
            ..Default::default()
        };
        sync(&d, &review, now).await.unwrap();
        assert!(!log.exists()); // No activity, no wake.
        let payload = serde_json::json!({"event":{"scope":"chat","event":{"type":"user_message","text":"work"}}});
        d.store
            .append_event(&start, Some("w"), Some("c"), false, &payload.to_string())
            .unwrap();
        sync(&d, &review, now).await.unwrap();
        sync(&d, &review, now).await.unwrap();
        let calls = std::fs::read_to_string(&log).unwrap();
        assert_eq!(calls.lines().count(), 1);
        assert!(calls.contains(OWNER));
        d.settings
            .update(|doc| doc["keep_awake"] = toml_edit::value("never"))
            .unwrap();
        sync(&d, &review, now).await.unwrap();
        assert!(
            std::fs::read_to_string(&log)
                .unwrap()
                .contains("schedule cancel wake")
        );
        assert!(d.store.kv_get(KEY).unwrap().is_none());
        d.settings
            .update(|doc| doc["keep_awake"] = toml_edit::value("while_working"))
            .unwrap();
        sync(&d, &review, now).await.unwrap();
        std::fs::write(
            &battery,
            "#!/bin/sh\necho \"Now drawing from 'Battery Power'\"\necho '5%; discharging'\n",
        )
        .unwrap();
        sync(&d, &review, now).await.unwrap();
        assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 4);
        assert!(d.store.kv_get(KEY).unwrap().is_none());
        sync(
            &d,
            &DailyReviewSettings {
                enabled: false,
                ..review
            },
            now,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 4);
    }
}
