//! Operating-system view of a source workspace's processes, beyond those the daemon registered.
//! On Linux a process is the workspace's when its initial environment holds the workspace's
//! `CHAUFFEUR_WORKSPACE_ID`, which agents, scripts and terminals start with and their descendants
//! inherit, detached or launched by a lost daemon generation included.
//!
//! macOS hides other processes' environment, and neither the kernel's responsibility nor a
//! descriptor follows an agent's commands (Claude Code runs each in a new session, without the
//! agent's descriptors). There a process that works or writes in the workspace is its own when it
//! descends from the daemon, or when its session is not a living program's: its leader is gone,
//! descends from the daemon, or is itself an orphan working in the workspace (the shell of an
//! agent's background command). One whose session leads to a living program outside the daemon
//! (a terminal tab, an editor, an app) is an external writer, as is, on both systems, any other
//! process working or writing in the workspace. Like archiving, which ends every process working
//! in a workspace, this counts a job the user detached in the workspace from a closed terminal,
//! or a tmux server started there, as the workspace's.
//!
//! Outside this view: other users' processes and, on Linux, those that changed credentials (not
//! dumpable, the kernel hides them alike), writes through memory mappings without a descriptor,
//! on macOS a process of the workspace that neither works nor writes in it, on Linux one started
//! with the marker removed from its environment.
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedProcess {
    pub pid: u32,
    pub stamp: String,
    pub name: String,
}

/// What tells the workspace's processes from the others.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Owner {
    /// Linux: the workspace's `CHAUFFEUR_WORKSPACE_ID` in the initial environment.
    Marker,
    /// macOS: this daemon process, whose descendants and orphaned sessions are the workspace's.
    Daemon(u32),
}

impl Owner {
    /// This daemon's: the marker on Linux, the daemon's pid on macOS. None elsewhere.
    pub fn of_daemon() -> Option<Owner> {
        platform::owner()
    }
}

/// Why a process that is not the workspace's counts as writing in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Touch {
    WorkingDirectory(PathBuf),
    WritableFile(PathBuf),
    /// Its working directory or descriptors could not be read.
    Unreadable,
}

#[derive(Debug, Default)]
pub struct Inventory {
    /// The workspace's own processes, to stop.
    pub own: Vec<ObservedProcess>,
    /// Other processes working or writing in the workspace: never signalled.
    pub external: Vec<(ObservedProcess, Touch)>,
}

impl Inventory {
    /// The refusal a user reads when other programs work in the workspace.
    pub fn external_refusal(&self) -> Option<Error> {
        if self.external.is_empty() {
            return None;
        }
        let named: Vec<String> = self
            .external
            .iter()
            .take(8)
            .map(|(process, touch)| {
                let what = match touch {
                    Touch::WorkingDirectory(path) => format!("works in {}", path.display()),
                    Touch::WritableFile(path) => format!("writes {}", path.display()),
                    Touch::Unreadable => "cannot be inspected".into(),
                };
                format!("{} (pid {}) {what}", process.name, process.pid)
            })
            .collect();
        let more = self.external.len().saturating_sub(named.len());
        let more = if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        };
        Some(Error::Conflict(format!(
            "Programs Chauffeur did not start use this workspace: {}{more}. Close them, then retry the transfer",
            named.join("; ")
        )))
    }
}

fn unavailable() -> Error {
    Error::Unavailable("The workspace's processes cannot be listed; retry the source stop".into())
}

fn inside(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path.starts_with(root))
}

/// Read-only scan of every process of this user but the daemon. `roots` must be canonical.
pub fn scan(workspace: &str, owner: &Owner, roots: &[PathBuf]) -> Result<Inventory> {
    let marker = format!("CHAUFFEUR_WORKSPACE_ID={workspace}");
    let own = std::process::id();
    let mut inventory = Inventory::default();
    for pid in platform::pids()? {
        if pid == own || pid == 0 {
            continue;
        }
        let Some(process) = platform::observe(pid, owner, marker.as_bytes(), roots)? else {
            continue;
        };
        if process.own {
            inventory.own.push(process.observed);
        } else if let Some(touch) = process.touch {
            inventory.external.push((process.observed, touch));
        }
    }
    inventory.own.sort_by_key(|p| p.pid);
    inventory.external.sort_by_key(|(p, _)| p.pid);
    Ok(inventory)
}

/// Sends `signal` only if `process` is still the exact live process observed: same pid, start
/// time and user. Returns false when it is gone. A pid reused between this check and `kill` is
/// the remaining window: macOS has no descriptor to signal a process exactly.
pub fn signal(process: &ObservedProcess, signal: i32) -> Result<bool> {
    if is_gone(process) {
        return Ok(false);
    }
    // SAFETY: kill only sends a signal; the pid was just verified to be the observed process.
    if unsafe { libc::kill(process.pid as i32, signal) } == 0 {
        return Ok(true);
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::ESRCH) => Ok(false),
        _ => Err(unavailable()),
    }
}

/// The folder process `pid` works in, if the system tells it.
pub fn working_directory(pid: u32) -> Option<PathBuf> {
    platform::working_directory(pid)
}

/// A zombie counts as gone: it runs nothing and holds no file.
pub fn is_gone(process: &ObservedProcess) -> bool {
    platform::live_stamp(process.pid).as_deref() != Some(process.stamp.as_str())
}

/// Waits until every process is gone, at most `limit`. Returns those still running.
pub async fn wait_gone(processes: &[ObservedProcess], limit: Duration) -> Vec<ObservedProcess> {
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        let left: Vec<ObservedProcess> =
            processes.iter().filter(|p| !is_gone(p)).cloned().collect();
        if left.is_empty() || tokio::time::Instant::now() >= deadline {
            return left;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

struct Seen {
    observed: ObservedProcess,
    own: bool,
    touch: Option<Touch>,
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    const PROC_PIDFDVNODEPATHINFO: i32 = 2;
    const FWRITE: u32 = 0x2;

    #[repr(C)]
    struct ProcFileInfo {
        fi_openflags: u32,
        fi_status: u32,
        fi_offset: i64,
        fi_type: i32,
        fi_guardflags: u32,
    }
    #[repr(C)]
    struct VnodeFdInfoWithPath {
        pfi: ProcFileInfo,
        pvip: libc::vnode_info_path,
    }

    fn errno() -> Option<i32> {
        std::io::Error::last_os_error().raw_os_error()
    }

    /// libproc returns 0 both on failure and for an empty answer: only errno tells them apart.
    fn clear_errno() {
        // SAFETY: __error returns this thread's errno location.
        unsafe { *libc::__error() = 0 };
    }

    pub fn pids() -> Result<Vec<u32>> {
        // SAFETY: a null buffer asks for the number of pids.
        let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        if count <= 0 {
            return Err(unavailable());
        }
        // Processes started between the two calls fit in the margin; the sweep scans again.
        let mut pids = vec![0i32; count as usize + 256];
        let bytes = (pids.len() * std::mem::size_of::<i32>()) as i32;
        // SAFETY: the buffer holds `bytes` bytes of i32.
        let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        if count <= 0 || count as usize >= pids.len() {
            return Err(unavailable());
        }
        pids.truncate(count as usize);
        Ok(pids
            .into_iter()
            .filter(|p| *p > 0)
            .map(|p| p as u32)
            .collect())
    }

    fn path_of(info: &libc::vnode_info_path) -> PathBuf {
        // SAFETY: vip_path is MAXPATHLEN contiguous bytes.
        let bytes: &[u8] =
            unsafe { std::slice::from_raw_parts(info.vip_path.as_ptr().cast(), 1024) };
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        PathBuf::from(std::ffi::OsStr::from_bytes(&bytes[..end]))
    }

    fn name_of(info: &libc::proc_bsdinfo) -> String {
        let field: &[libc::c_char] = if info.pbi_name[0] != 0 {
            &info.pbi_name
        } else {
            &info.pbi_comm
        };
        let bytes: Vec<u8> = field
            .iter()
            .take_while(|c| **c != 0)
            .map(|c| *c as u8)
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// The start stamp of a live process of this user, in `cloud_process`'s format.
    pub fn live_stamp(pid: u32) -> Option<String> {
        let bsd = bsd_info(pid)?;
        Some(format!(
            "mac:{}:{}",
            bsd.pbi_start_tvsec, bsd.pbi_start_tvusec
        ))
    }

    /// None when gone, a zombie, or another user's (EPERM): none is observable here.
    fn bsd_info(pid: u32) -> Option<libc::proc_bsdinfo> {
        let mut bsd = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        // SAFETY: output points to an aligned allocation of the exact structure and size.
        let read = unsafe {
            libc::proc_pidinfo(
                pid as i32,
                libc::PROC_PIDTBSDINFO,
                0,
                bsd.as_mut_ptr().cast(),
                size,
            )
        };
        if read != size {
            return None;
        }
        // SAFETY: proc_pidinfo initialized the complete structure.
        let bsd = unsafe { bsd.assume_init() };
        // SAFETY: geteuid has no preconditions.
        if bsd.pbi_uid != unsafe { libc::geteuid() } || bsd.pbi_status == libc::SZOMB {
            return None;
        }
        Some(bsd)
    }

    pub(super) fn observe(
        pid: u32,
        owner: &Owner,
        _marker: &[u8],
        roots: &[PathBuf],
    ) -> Result<Option<Seen>> {
        let Some(bsd) = bsd_info(pid) else {
            return Ok(None);
        };
        let observed = ObservedProcess {
            pid,
            stamp: format!("mac:{}:{}", bsd.pbi_start_tvsec, bsd.pbi_start_tvusec),
            name: name_of(&bsd),
        };
        let touch = match touch(pid, roots) {
            Ok(touch) => touch,
            Err(()) if live_stamp(pid).as_deref() != Some(observed.stamp.as_str()) => {
                return Ok(None);
            }
            Err(()) => Some(Touch::Unreadable),
        };
        // Ownership rests on what the process touches: an unreadable one stays external.
        let own = matches!(
            touch,
            Some(Touch::WorkingDirectory(_) | Touch::WritableFile(_))
        ) && matches!(owner, Owner::Daemon(daemon) if belongs(pid, *daemon, roots));
        Ok(Some(Seen {
            observed,
            own,
            touch,
        }))
    }

    pub fn working_directory(pid: u32) -> Option<PathBuf> {
        let mut dirs = std::mem::MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
        let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as i32;
        // SAFETY: output points to an aligned allocation of the exact structure and size.
        let read = unsafe {
            libc::proc_pidinfo(
                pid as i32,
                libc::PROC_PIDVNODEPATHINFO,
                0,
                dirs.as_mut_ptr().cast(),
                size,
            )
        };
        if read != size {
            return None;
        }
        // SAFETY: proc_pidinfo initialized the complete structure.
        let dirs = unsafe { dirs.assume_init() };
        Some(path_of(&dirs.pvi_cdir))
    }

    fn touch(pid: u32, roots: &[PathBuf]) -> std::result::Result<Option<Touch>, ()> {
        let cwd = working_directory(pid).ok_or(())?;
        if inside(&cwd, roots) {
            return Ok(Some(Touch::WorkingDirectory(cwd)));
        }
        let fds = descriptors(pid)?;
        for fd in fds
            .iter()
            .filter(|fd| fd.proc_fdtype == libc::PROX_FDTYPE_VNODE as u32)
        {
            let mut info = std::mem::MaybeUninit::<VnodeFdInfoWithPath>::zeroed();
            let size = std::mem::size_of::<VnodeFdInfoWithPath>() as i32;
            // SAFETY: output points to an aligned allocation of the exact structure and size.
            let read = unsafe {
                libc::proc_pidfdinfo(
                    pid as i32,
                    fd.proc_fd,
                    PROC_PIDFDVNODEPATHINFO,
                    info.as_mut_ptr().cast(),
                    size,
                )
            };
            if read != size {
                // EBADF: closed since the listing. ENOENT: revoked or unlinked, outside any
                // folder. EPERM: the system withholds the path of protected stores (data vaults),
                // which a workspace folder cannot be.
                if matches!(errno(), Some(libc::EBADF | libc::ENOENT | libc::EPERM)) {
                    continue;
                }
                return Err(());
            }
            // SAFETY: proc_pidfdinfo initialized the complete structure.
            let info = unsafe { info.assume_init() };
            if info.pfi.fi_openflags & FWRITE == 0 {
                continue;
            }
            let path = path_of(&info.pvip);
            if inside(&path, roots) {
                return Ok(Some(Touch::WritableFile(path)));
            }
        }
        Ok(None)
    }

    /// Every descriptor, the list growing until it fits: a truncated one could miss a writer.
    fn descriptors(pid: u32) -> std::result::Result<Vec<libc::proc_fdinfo>, ()> {
        let entry = std::mem::size_of::<libc::proc_fdinfo>();
        clear_errno();
        // SAFETY: a null buffer asks for the size of the descriptor list.
        let bytes = unsafe {
            libc::proc_pidinfo(
                pid as i32,
                libc::PROC_PIDLISTFDS,
                0,
                std::ptr::null_mut(),
                0,
            )
        };
        if bytes <= 0 && errno() != Some(0) {
            return Err(());
        }
        let mut room = bytes.max(0) as usize / entry + 32;
        for _ in 0..4 {
            let mut fds: Vec<libc::proc_fdinfo> = Vec::with_capacity(room);
            let capacity = (room * entry) as i32;
            clear_errno();
            // SAFETY: the buffer has room for `capacity` bytes of proc_fdinfo.
            let read = unsafe {
                libc::proc_pidinfo(
                    pid as i32,
                    libc::PROC_PIDLISTFDS,
                    0,
                    fds.as_mut_ptr().cast(),
                    capacity,
                )
            };
            if read <= 0 && errno() != Some(0) {
                return Err(());
            }
            if read < capacity {
                // SAFETY: the kernel initialized `read` bytes of whole entries.
                unsafe { fds.set_len(read.max(0) as usize / entry) };
                return Ok(fds);
            }
            room *= 4;
        }
        Err(())
    }

    /// The parent of any process, another user's included; None when unknown.
    fn parent(pid: u32) -> Option<u32> {
        let mut bsd = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        // SAFETY: output points to an aligned allocation of the exact structure and size.
        let read = unsafe {
            libc::proc_pidinfo(
                pid as i32,
                libc::PROC_PIDTBSDINFO,
                0,
                bsd.as_mut_ptr().cast(),
                size,
            )
        };
        // SAFETY: proc_pidinfo initialized the complete structure when it read all of it.
        (read == size).then(|| unsafe { bsd.assume_init() }.pbi_ppid)
    }

    fn descends(mut pid: u32, daemon: u32) -> bool {
        for _ in 0..128 {
            if pid == daemon {
                return true;
            }
            match parent(pid) {
                Some(next) if next > 1 && next != pid => pid = next,
                _ => return false,
            }
        }
        false
    }

    fn session(pid: u32) -> Option<u32> {
        // SAFETY: getsid only reads the kernel's record for `pid`.
        let sid = unsafe { libc::getsid(pid as libc::pid_t) };
        (sid > 0).then_some(sid as u32)
    }

    /// Whether a process working in the workspace is the workspace's (see the module).
    fn belongs(pid: u32, daemon: u32, roots: &[PathBuf]) -> bool {
        if descends(pid, daemon) {
            return true;
        }
        let Some(leader) = session(pid) else {
            return false;
        };
        // A pid reused since by another process that does not lead that session: gone.
        if leader <= 1 || session(leader) != Some(leader) {
            return leader > 1;
        }
        descends(leader, daemon)
            || (parent(leader) == Some(1)
                && matches!(
                    touch(leader, roots),
                    Ok(Some(Touch::WorkingDirectory(_) | Touch::WritableFile(_)))
                ))
    }

    pub fn owner() -> Option<Owner> {
        Some(Owner::Daemon(std::process::id()))
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use std::io::ErrorKind;
    use std::os::unix::fs::MetadataExt;

    pub fn working_directory(pid: u32) -> Option<PathBuf> {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }

    pub fn pids() -> Result<Vec<u32>> {
        let mut pids = Vec::new();
        for entry in std::fs::read_dir("/proc").map_err(|_| unavailable())? {
            let entry = entry.map_err(|_| unavailable())?;
            if let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<u32>().ok())
            {
                pids.push(pid);
            }
        }
        Ok(pids)
    }

    /// The start stamp of a live process of this user, in `cloud_process`'s format. Unlike
    /// `cloud_process::inspect`, a parent changing under a re-parented child is no exit.
    pub fn live_stamp(pid: u32) -> Option<String> {
        let dir = PathBuf::from(format!("/proc/{pid}"));
        // SAFETY: getuid has no preconditions.
        if std::fs::metadata(&dir).ok()?.uid() != unsafe { libc::getuid() } {
            return None;
        }
        let stat = std::fs::read_to_string(dir.join("stat")).ok()?;
        let close = stat.rfind(')')?;
        let fields: Vec<&str> = stat[close + 1..].split_whitespace().collect();
        if matches!(fields.first(), Some(&"Z" | &"X")) {
            return None;
        }
        let ticks = fields.get(19)?.parse::<u64>().ok()?;
        let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
        Some(format!("linux:{}:{ticks}", boot.trim()))
    }

    pub fn owner() -> Option<Owner> {
        Some(Owner::Marker)
    }

    pub(super) fn observe(
        pid: u32,
        owner: &Owner,
        marker: &[u8],
        roots: &[PathBuf],
    ) -> Result<Option<Seen>> {
        let dir = PathBuf::from(format!("/proc/{pid}"));
        let Some(stamp) = live_stamp(pid) else {
            return Ok(None);
        };
        let name = std::fs::read_to_string(dir.join("comm"))
            .map(|n| n.trim().to_owned())
            .unwrap_or_default();
        let observed = ObservedProcess { pid, stamp, name };
        let own = *owner == Owner::Marker
            && std::fs::read(dir.join("environ"))
                .is_ok_and(|env| env.split(|b| *b == 0).any(|entry| entry == marker));
        // A process that changed credentials (sd-pam, ssh-agent) is not dumpable: the kernel
        // refuses its working directory and descriptors as for another user's, outside this view.
        // One that is exiting loses them before it turns zombie: NotFound.
        let touch = match touch(&dir, roots) {
            Ok(touch) => touch,
            Err(ErrorKind::PermissionDenied | ErrorKind::NotFound) => {
                return Ok(None);
            }
            Err(_) if live_stamp(pid).as_deref() != Some(observed.stamp.as_str()) => {
                return Ok(None);
            }
            Err(_) => Some(Touch::Unreadable),
        };
        Ok(Some(Seen {
            observed,
            own,
            touch,
        }))
    }

    fn touch(dir: &Path, roots: &[PathBuf]) -> std::result::Result<Option<Touch>, ErrorKind> {
        let cwd = std::fs::read_link(dir.join("cwd")).map_err(|e| e.kind())?;
        if inside(&cwd, roots) {
            return Ok(Some(Touch::WorkingDirectory(cwd)));
        }
        for entry in std::fs::read_dir(dir.join("fd")).map_err(|e| e.kind())? {
            let entry = entry.map_err(|e| e.kind())?;
            let Ok(path) = std::fs::read_link(entry.path()) else {
                continue;
            };
            // An unlinked file is in no folder any more.
            if path.to_string_lossy().ends_with(" (deleted)") || !inside(&path, roots) {
                continue;
            }
            let Ok(info) = std::fs::read_to_string(dir.join("fdinfo").join(entry.file_name()))
            else {
                continue;
            };
            let flags = info
                .lines()
                .find_map(|line| line.strip_prefix("flags:"))
                .and_then(|flags| u32::from_str_radix(flags.trim(), 8).ok())
                .ok_or(ErrorKind::InvalidData)?;
            if flags & (libc::O_WRONLY | libc::O_RDWR) as u32 != 0 {
                return Ok(Some(Touch::WritableFile(path)));
            }
        }
        Ok(None)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod platform {
    use super::*;
    pub fn pids() -> Result<Vec<u32>> {
        Err(unavailable())
    }
    pub fn owner() -> Option<Owner> {
        None
    }
    pub(super) fn observe(_: u32, _: &Owner, _: &[u8], _: &[PathBuf]) -> Result<Option<Seen>> {
        Err(unavailable())
    }
    pub fn working_directory(_: u32) -> Option<PathBuf> {
        None
    }

    pub fn live_stamp(_: u32) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command};

    fn spawn(script: &str, cwd: &Path, marker: Option<&str>, arg: &Path) -> Child {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script, "sh"]).arg(arg).current_dir(cwd);
        command.env_remove("CHAUFFEUR_WORKSPACE_ID");
        if let Some(workspace) = marker {
            command.env("CHAUFFEUR_WORKSPACE_ID", workspace);
        }
        command.spawn().unwrap()
    }

    fn end(mut children: Vec<Child>) {
        for child in &mut children {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn touch_of(inventory: &Inventory, pid: u32) -> Option<&Touch> {
        inventory
            .external
            .iter()
            .find(|(p, _)| p.pid == pid)
            .map(|(_, touch)| touch)
    }

    /// The test process stands for the daemon its children descend from.
    fn owner() -> Owner {
        Owner::of_daemon().unwrap()
    }

    fn pids(list: &[ObservedProcess]) -> Vec<u32> {
        list.iter().map(|p| p.pid).collect()
    }

    struct Folders {
        _dir: tempfile::TempDir,
        root: PathBuf,
        outside: PathBuf,
        file: PathBuf,
    }

    fn folders() -> Folders {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().canonicalize().unwrap();
        let root = outside.join("workspace");
        std::fs::create_dir(&root).unwrap();
        let file = root.join("log.txt");
        std::fs::write(&file, "").unwrap();
        Folders {
            _dir: dir,
            root,
            outside,
            file,
        }
    }

    /// Waits until each child's script reached `exec sleep`: what it opened before is then held,
    /// and it bears the name `sleep`.
    #[cfg(target_os = "macos")]
    fn wait_for_sleep(children: &[&Child]) {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        for child in children {
            loop {
                let mut path = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
                // SAFETY: the buffer holds the number of bytes passed.
                let length = unsafe {
                    libc::proc_pidpath(
                        child.id() as i32,
                        path.as_mut_ptr().cast(),
                        path.len() as u32,
                    )
                };
                if length > 0 && path[..length as usize].ends_with(b"/sleep") {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "pid {} never ran sleep",
                    child.id()
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn processes_writing_in_the_workspace_are_its_own_only_when_they_descend_from_the_daemon() {
        let f = folders();
        let workspace = uuid::Uuid::now_v7().to_string();
        let working = spawn("exec sleep 30", &f.root, None, &f.file);
        let writer = spawn("exec 3>>\"$1\"; exec sleep 30", &f.outside, None, &f.file);
        let reader = spawn("exec 3<\"$1\"; exec sleep 30", &f.outside, None, &f.file);
        let elsewhere = spawn("exec sleep 30", &f.outside, None, &f.file);
        wait_for_sleep(&[&working, &writer, &reader, &elsewhere]);
        let roots = std::slice::from_ref(&f.root);
        let inventory = scan(&workspace, &owner(), roots).unwrap();
        assert_eq!(pids(&inventory.own), vec![working.id(), writer.id()]);
        assert!(inventory.external.is_empty(), "{:?}", inventory.external);
        // Under another daemon, the same programs belong to this test's session, whose leader
        // lives outside that daemon: external writers.
        let inventory = scan(&workspace, &Owner::Daemon(u32::MAX), roots).unwrap();
        assert!(inventory.own.is_empty());
        assert_eq!(
            touch_of(&inventory, working.id()),
            Some(&Touch::WorkingDirectory(f.root.clone()))
        );
        assert_eq!(
            touch_of(&inventory, writer.id()),
            Some(&Touch::WritableFile(f.file.clone()))
        );
        assert_eq!(
            touch_of(&inventory, reader.id()),
            None,
            "a reader writes nothing"
        );
        assert_eq!(touch_of(&inventory, elsewhere.id()), None);
        let refusal = inventory.external_refusal().unwrap().to_string();
        assert!(
            refusal.contains(&format!("sleep (pid {})", working.id())),
            "{refusal}"
        );
        end(vec![working, writer, reader, elsewhere]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn marked_processes_are_the_workspace_s_and_unmarked_writers_inside_are_external() {
        let f = folders();
        let workspace = uuid::Uuid::now_v7().to_string();
        let marked = spawn("exec sleep 30", &f.outside, Some(&workspace), &f.file);
        let other = spawn("exec sleep 30", &f.root, Some("another-workspace"), &f.file);
        let writer = spawn("exec 3>>\"$1\"; exec sleep 30", &f.outside, None, &f.file);
        let reader = spawn("exec 3<\"$1\"; exec sleep 30", &f.outside, None, &f.file);
        std::thread::sleep(Duration::from_millis(300));
        let inventory = scan(&workspace, &owner(), std::slice::from_ref(&f.root)).unwrap();
        assert_eq!(pids(&inventory.own), vec![marked.id()]);
        assert_eq!(
            touch_of(&inventory, other.id()),
            Some(&Touch::WorkingDirectory(f.root.clone()))
        );
        assert_eq!(
            touch_of(&inventory, writer.id()),
            Some(&Touch::WritableFile(f.file.clone()))
        );
        assert_eq!(
            touch_of(&inventory, reader.id()),
            None,
            "a reader writes nothing"
        );
        end(vec![marked, other, writer, reader]);
    }

    #[test]
    fn a_process_is_signalled_only_while_it_is_the_one_observed() {
        let f = folders();
        let workspace = uuid::Uuid::now_v7().to_string();
        let mut child = spawn("exec sleep 30", &f.root, Some(&workspace), &f.file);
        std::thread::sleep(Duration::from_millis(200));
        let observed = scan(&workspace, &owner(), std::slice::from_ref(&f.root))
            .unwrap()
            .own
            .pop()
            .unwrap();
        assert_eq!(observed.pid, child.id());
        let mut reused = observed.clone();
        reused.stamp.push_str("-earlier-process");
        assert!(!signal(&reused, libc::SIGKILL).unwrap());
        assert!(!is_gone(&observed), "a changed identity is never signalled");
        assert!(signal(&observed, libc::SIGKILL).unwrap());
        child.wait().unwrap();
        assert!(is_gone(&observed));
        assert!(!signal(&observed, libc::SIGKILL).unwrap());
    }

    /// Starts `sleep 60` working in `cwd` in a new session, detached from this test as an
    /// agent's background command is: `leader` is "dead" (the leader exits at once), "orphan"
    /// (the sleep leads its session itself) or "away" (a living leader sitting in `/`).
    #[cfg(target_os = "macos")]
    fn detached(cwd: &Path, leader: &str) -> u32 {
        let pidfile =
            std::env::temp_dir().join(format!("chauffeur-detached-{}", uuid::Uuid::now_v7()));
        let script = r#"
import os, sys, time
cwd, pidfile, leader = sys.argv[1], sys.argv[2], sys.argv[3]
if os.fork():
    os._exit(0)
os.setsid()
os.chdir(cwd if leader == "orphan" else "/")
child = os.getpid() if leader == "orphan" else os.fork()
if child == os.getpid() or child == 0:
    pid = os.getpid()
    if leader != "orphan":
        os.chdir(cwd)
    with open(pidfile + ".tmp", "w") as f:
        f.write(str(pid))
    os.rename(pidfile + ".tmp", pidfile)
    os.execv("/bin/sleep", ["sleep", "60"])
if leader == "away":
    os.waitpid(child, 0)
os._exit(0)
"#;
        let status = Command::new("python3")
            .args(["-I", "-c", script])
            .arg(cwd)
            .arg(&pidfile)
            .arg(leader)
            .status()
            .unwrap();
        assert!(status.success());
        for _ in 0..200 {
            if let Ok(pid) = std::fs::read_to_string(&pidfile) {
                std::fs::remove_file(&pidfile).unwrap();
                std::thread::sleep(Duration::from_millis(200));
                return pid.trim().parse().unwrap();
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the detached process did not start");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_detached_process_is_the_workspace_s_unless_a_living_program_leads_its_session() {
        let f = folders();
        let workspace = uuid::Uuid::now_v7().to_string();
        let roots = std::slice::from_ref(&f.root);
        let dead = detached(&f.root, "dead");
        let orphan = detached(&f.root, "orphan");
        let away = detached(&f.root, "away");
        // No process of this test descends from this daemon: only sessions decide.
        let inventory = scan(&workspace, &Owner::Daemon(u32::MAX), roots).unwrap();
        let own = pids(&inventory.own);
        assert!(own.contains(&dead), "its leader is gone: {own:?}");
        assert!(
            own.contains(&orphan),
            "an orphan leading its session in the workspace: {own:?}"
        );
        assert!(!own.contains(&away));
        assert_eq!(
            touch_of(&inventory, away),
            Some(&Touch::WorkingDirectory(f.root.clone())),
            "a living leader outside the workspace makes it someone else's"
        );
        for pid in [dead, orphan, away] {
            // SAFETY: test processes started above.
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
    }
}
