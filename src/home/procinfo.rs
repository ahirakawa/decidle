//! Looking up a process's parent, name and start time, to record the claude process at
//! session start and to check, before a kill, that the pid still belongs to that claude.

pub const CLAUDE_NAME: &str = "claude";
const MAX_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcInfo {
    pub ppid: i64,
    /// Names the kernel reports for the process (short and long form on macOS).
    pub names: Vec<String>,
    /// Start time: microseconds since the epoch on macOS, clock ticks since boot on Linux.
    pub started: i64,
}

#[cfg(target_os = "macos")]
pub fn info(pid: i64) -> Option<ProcInfo> {
    let pid = libc::c_int::try_from(pid).ok().filter(|p| *p > 0)?;
    // SAFETY: proc_bsdinfo is plain data; proc_pidinfo writes at most `size` bytes into it.
    let mut bi: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let n = unsafe {
        libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, &mut bi as *mut _ as *mut libc::c_void, size)
    };
    if n != size {
        return None;
    }
    let text = |raw: &[libc::c_char]| {
        let bytes: Vec<u8> = raw.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let mut names = vec![text(&bi.pbi_comm), text(&bi.pbi_name)];
    names.extend(exec_names(pid));
    Some(ProcInfo {
        ppid: bi.pbi_ppid as i64,
        names,
        started: (bi.pbi_start_tvsec as i64) * 1_000_000 + bi.pbi_start_tvusec as i64,
    })
}

/// The last path component of the executed path and of argv[0]. The native claude is a
/// symlink `claude` to a file named after its version (e.g. `2.1.327`), and the kernel's
/// comm holds the version, so the name claude is only visible here.
#[cfg(target_os = "macos")]
fn exec_names(pid: libc::c_int) -> Vec<String> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size: libc::size_t = 0;
    // SAFETY: the first call only asks for the size; the second writes at most `size` bytes.
    if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0) } != 0 {
        return Vec::new();
    }
    let mut buf = vec![0u8; size];
    if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr() as *mut libc::c_void, &mut size, std::ptr::null_mut(), 0) } != 0 {
        return Vec::new();
    }
    buf.truncate(size);
    // Layout: argc (int), the executed path, NUL padding, then argv[0], argv[1], ...
    let Some(rest) = buf.get(std::mem::size_of::<libc::c_int>()..) else { return Vec::new() };
    rest.split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .take(2)
        .map(|s| {
            let s = String::from_utf8_lossy(s);
            s.rsplit('/').next().unwrap_or_default().to_string()
        })
        .collect()
}

#[cfg(target_os = "linux")]
pub fn info(pid: i64) -> Option<ProcInfo> {
    if pid <= 0 {
        return None;
    }
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (open, close) = (stat.find('(')?, stat.rfind(')')?);
    let name = stat.get(open + 1..close)?.to_string();
    let fields: Vec<&str> = stat.get(close + 1..)?.split_whitespace().collect();
    // Field 4 of stat(5) is ppid and field 22 is starttime; `fields` starts at field 3.
    Some(ProcInfo {
        ppid: fields.get(1)?.parse().ok()?,
        names: vec![name],
        started: fields.get(19)?.parse().ok()?,
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn info(_pid: i64) -> Option<ProcInfo> {
    None
}

fn named_claude(p: &ProcInfo) -> bool {
    p.names.iter().any(|n| n == CLAUDE_NAME)
}

/// The nearest ancestor of this process named claude, as (pid, start time).
pub fn find_claude_ancestor() -> Option<(i64, i64)> {
    // SAFETY: getppid has no preconditions.
    let mut pid = unsafe { libc::getppid() } as i64;
    for _ in 0..MAX_DEPTH {
        if pid <= 1 {
            return None;
        }
        let p = info(pid)?;
        if named_claude(&p) {
            return Some((pid, p.started));
        }
        pid = p.ppid;
    }
    None
}

/// Whether `pid` is still a claude process that started at `started`.
pub fn is_same_claude(pid: i64, started: i64) -> bool {
    pid > 1 && info(pid).is_some_and(|p| named_claude(&p) && p.started == started)
}
