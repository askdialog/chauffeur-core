//! `chauffeurd`: runs in the foreground until SIGTERM, SIGINT or `daemon.shutdown`.
//! The CLI starts it in the background (`chauffeur daemon start`).

use std::time::Duration;

use crate::Daemon;
use crate::paths::Paths;
use anyhow::Context;

/// A debug build gives `rpc::handle` a frame of hundreds of kilobytes, and a transfer calls it
/// from inside itself (`cloud.transfer.start` dispatches to this daemon's own `handle`): with
/// tokio's default 2 MiB per thread, the daemon aborted on a stack overflow when a transfer
/// started. Stack memory is only committed as it is used.
const RUNTIME_STACK_SIZE: usize = 8 * 1024 * 1024;

pub fn main(extensions: Vec<crate::extensions::Registration>) -> anyhow::Result<()> {
    let paths = Paths::from_env()?;
    paths.ensure()?;
    {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::EnvFilter::try_from_env("CHAUFFEUR_LOG")
                    .unwrap_or_else(|_| "info".into()),
            )
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
            .with(crate::error_reports::RequestLogs)
            .init();
    }
    raise_open_files_limit();
    let _lock = acquire_lock(&paths)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(RUNTIME_STACK_SIZE)
        .build()?;
    runtime.block_on(run(paths, extensions))
}

async fn run(paths: Paths, extensions: Vec<crate::extensions::Registration>) -> anyhow::Result<()> {
    let daemon = Daemon::open_with_extensions(paths, extensions)?;
    {
        let env = daemon.shell_env.clone();
        tokio::task::spawn_blocking(move || env.capture()).await?;
    }
    {
        let shutdown = daemon.shutdown.clone();
        tokio::spawn(async move {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("SIGTERM handler");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
            shutdown.cancel();
        });
    }
    if std::env::args().any(|arg| arg == "--transfer-controller") {
        return crate::server::serve(daemon).await;
    }
    crate::transfer_controller::spawn(daemon.clone());
    daemon.chats.recover_after_restart();
    daemon.workspaces.recover_after_restart();
    crate::daily_review::spawn(daemon.clone());
    crate::routines::spawn(daemon.clone());
    daemon.error_reports.spawn_flush(daemon.shutdown.clone());
    crate::idle::spawn(daemon.clone());
    crate::search::spawn(daemon.clone());
    crate::power::spawn(daemon.clone());
    crate::update::spawn(daemon.clone());
    crate::cloud_workspace_discard::spawn_retries(daemon.clone());
    crate::cloud_transfer_driver::spawn_discard_checks(daemon.clone());
    crate::cloud_transfer_driver::recover(&daemon);
    crate::relay_events::spawn_watch(daemon.clone());
    crate::idle_reminders::spawn(daemon.clone());
    crate::relay_ports::spawn(daemon.clone());
    crate::workers::resume(&daemon);
    crate::worker_lifecycle::spawn(daemon.clone());
    crate::cloud_workspace_backup::spawn(daemon.clone());
    daemon.extensions.resume();
    crate::identity::learn_team_domain(daemon.clone());
    {
        let daemon = daemon.clone();
        tokio::spawn(async move {
            loop {
                match daemon.workspaces.purge_expired_snapshots().await {
                    Ok(0) => {}
                    Ok(count) => tracing::info!(count, "expired archive snapshots deleted"),
                    Err(error) => tracing::warn!(%error, "snapshot purge failed"),
                }
                tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
            }
        });
    }
    let served = crate::server::serve(daemon.clone()).await;
    daemon.chats.sidecar.shutdown().await;
    daemon.workers.shutdown();
    daemon.transfer_controller.shutdown(&daemon).await;
    daemon.ollama.shutdown().await;
    // The power assertion goes and sleep turned off comes back on.
    {
        let daemon = daemon.clone();
        let _ = tokio::task::spawn_blocking(move || daemon.power.shutdown()).await;
    }
    served
}

/// launchd starts the daemon with a soft limit of 256 open files, which its concurrent `git` and
/// `gh` children (three pipes each), terminals and sockets go past: an archive then failed with
/// "Too many open files". Raises the soft limit to the hard one, capped on macOS by
/// `kern.maxfilesperproc` since `setrlimit` refuses more (`RLIM_INFINITY` included).
fn raise_open_files_limit() {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit writes into a struct we own.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        tracing::warn!(error = %std::io::Error::last_os_error(), "could not read the open files limit");
        return;
    }
    // macOS also caps a process below its hard limit (kern.maxfilesperproc).
    #[cfg(target_os = "macos")]
    let target = max_files_per_process().map_or(limit.rlim_max, |max| limit.rlim_max.min(max));
    #[cfg(not(target_os = "macos"))]
    let target = limit.rlim_max;
    if limit.rlim_cur >= target {
        return;
    }
    let raised = libc::rlimit {
        rlim_cur: target,
        rlim_max: limit.rlim_max,
    };
    // SAFETY: setrlimit reads a struct we own.
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raised) } != 0 {
        tracing::warn!(error = %std::io::Error::last_os_error(), target, "could not raise the open files limit");
    }
}

#[cfg(target_os = "macos")]
fn max_files_per_process() -> Option<libc::rlim_t> {
    let mut value: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    // SAFETY: the name is NUL-terminated and `value` is as large as `size` says.
    let read = unsafe {
        libc::sysctlbyname(
            c"kern.maxfilesperproc".as_ptr(),
            (&raw mut value).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (read == 0 && value > 0).then_some(value as libc::rlim_t)
}

/// One daemon per state directory.
fn acquire_lock(paths: &Paths) -> anyhow::Result<std::fs::File> {
    use std::os::fd::AsRawFd;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&paths.lock)
        .with_context(|| format!("opening {}", paths.lock.display()))?;
    // A daemon that is shutting down still holds the lock for a moment: wait for it.
    let mut locked = false;
    for _ in 0..100 {
        // SAFETY: flock on a file descriptor we own.
        locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
        if locked {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    anyhow::ensure!(
        locked,
        "another chauffeurd already runs for {}",
        paths.home.display()
    );
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_files_limit() -> libc::rlimit {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: getrlimit writes into a struct we own.
        assert_eq!(
            unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) },
            0
        );
        limit
    }

    #[test]
    fn the_launchd_default_of_256_open_files_is_raised() {
        let hard = open_files_limit().rlim_max;
        let launchd = libc::rlimit {
            rlim_cur: 256,
            rlim_max: hard,
        };
        // SAFETY: setrlimit reads a struct we own.
        assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &launchd) }, 0);
        raise_open_files_limit();
        let raised = open_files_limit();
        assert!(raised.rlim_cur > 256, "still {}", raised.rlim_cur);
        assert_eq!(raised.rlim_max, hard);
    }
}
