//! Read-only native process pinning; never signal a PID based on a request or heartbeat.
use crate::{Error, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeProcessPin {
    pub pid: u32,
    pub stamp: String,
}
fn unavailable() -> Error {
    Error::Unavailable("Native process identity cannot be verified".into())
}
pub fn inspect(pid: u32) -> Result<Option<NativeProcessPin>> {
    if pid == 0 || pid > i32::MAX as u32 {
        return Err(Error::BadRequest("Invalid native process".into()));
    }
    Ok(platform(pid)?.map(|(pin, _)| pin))
}

/// The private sidecar may report only its own direct child. Verify the parent generation
/// before and after observing the child; no caller-supplied stamp is trusted.
pub fn inspect_child(parent: &NativeProcessPin, pid: u32) -> Result<NativeProcessPin> {
    if is_stopped(parent)? || pid == 0 || pid > i32::MAX as u32 {
        return Err(unavailable());
    }
    let (pin, ppid) = platform(pid)?.ok_or_else(unavailable)?;
    if ppid != parent.pid || is_stopped(parent)? {
        return Err(unavailable());
    }
    Ok(pin)
}
/// The parent of a live process of this user; None when it is gone or someone else's.
pub fn parent(pid: u32) -> Option<u32> {
    platform(pid).ok().flatten().map(|(_, ppid)| ppid)
}
pub fn is_stopped(pin: &NativeProcessPin) -> Result<bool> {
    Ok(inspect(pin.pid)?.as_ref() != Some(pin))
}
#[cfg(target_os = "macos")]
fn platform(pid: u32) -> Result<Option<(NativeProcessPin, u32)>> {
    let mut value = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    // SAFETY: output points to an aligned allocation of the exact ABI structure and advertised size.
    let read = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            value.as_mut_ptr().cast(),
            size as i32,
        )
    };
    if read == 0 {
        let error = std::io::Error::last_os_error();
        return if matches!(error.raw_os_error(), Some(libc::ESRCH) | Some(libc::ENOENT)) {
            Ok(None)
        } else {
            Err(unavailable())
        };
    }
    if read != size as i32 {
        return Err(unavailable());
    }
    // SAFETY: proc_pidinfo successfully initialized the complete advertised structure.
    let value = unsafe { value.assume_init() };
    // SAFETY: geteuid has no preconditions.
    if value.pbi_uid != unsafe { libc::geteuid() } {
        return Err(unavailable());
    }
    Ok(Some((
        NativeProcessPin {
            pid,
            stamp: format!("mac:{}:{}", value.pbi_start_tvsec, value.pbi_start_tvusec),
        },
        value.pbi_ppid,
    )))
}
#[cfg(target_os = "linux")]
fn platform(pid: u32) -> Result<Option<(NativeProcessPin, u32)>> {
    let path = format!("/proc/{pid}/stat");
    let stat = match std::fs::read_to_string(path) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(unavailable()),
    };
    let close = stat.rfind(')').ok_or_else(unavailable)?;
    let parent = stat[close + 1..]
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u32>().ok())
        .ok_or_else(unavailable)?;
    let start = stat[close + 1..]
        .split_whitespace()
        .nth(19)
        .ok_or_else(unavailable)?;
    let ticks = start.parse::<u64>().map_err(|_| unavailable())?;
    let boot =
        std::fs::read_to_string("/proc/sys/kernel/random/boot_id").map_err(|_| unavailable())?;
    let status =
        std::fs::read_to_string(format!("/proc/{pid}/status")).map_err(|_| unavailable())?;
    let uid = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|line| line.split_whitespace().next())
        .and_then(|uid| uid.parse::<u32>().ok())
        .ok_or_else(unavailable)?;
    // SAFETY: getuid has no preconditions.
    if uid != unsafe { libc::getuid() } {
        return Err(unavailable());
    }
    // UID was read separately: reject exit/reuse between the two procfs reads.
    let again = std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|_| unavailable())?;
    let end = again.rfind(')').ok_or_else(unavailable)?;
    let fields: Vec<_> = again[end + 1..].split_whitespace().collect();
    if fields.get(19).and_then(|s| s.parse::<u64>().ok()) != Some(ticks)
        || fields.get(1).and_then(|s| s.parse::<u32>().ok()) != Some(parent)
    {
        return Err(unavailable());
    }
    Ok(Some((
        NativeProcessPin {
            pid,
            stamp: format!("linux:{}:{ticks}", boot.trim()),
        },
        parent,
    )))
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform(_pid: u32) -> Result<Option<(NativeProcessPin, u32)>> {
    Err(unavailable())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_native_child_must_really_exit_before_its_pin_is_stopped() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pin = inspect(child.id()).unwrap().unwrap();
        assert!(!is_stopped(&pin).unwrap());
        let mut other = pin.clone();
        other.stamp.push_str("-another-process");
        assert!(is_stopped(&other).unwrap());
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(is_stopped(&pin).unwrap());
    }
}
