use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(target_os = "macos")]
const O_NOFOLLOW: i32 = 0x0100;
#[cfg(target_os = "linux")]
const O_NOFOLLOW: i32 = 0x20000;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const O_NOFOLLOW: i32 = 0;

// Opening a FIFO without O_NONBLOCK waits for a writer, which would hang the hook.
#[cfg(target_os = "macos")]
const O_NONBLOCK: i32 = 0x0004;
#[cfg(target_os = "linux")]
const O_NONBLOCK: i32 = 0x800;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const O_NONBLOCK: i32 = 0;

/// Opens for reading without blocking on special files and returns the handle only if it is a regular file.
pub fn open_regular(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new().read(true).custom_flags(O_NONBLOCK).open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: 通常のファイルではない", path.display()),
        ));
    }
    Ok(file)
}

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

/// Where the gate keeps its log and per-session state. Not configurable on purpose.
pub fn data_dir() -> PathBuf {
    home().join(".local/share/agent-report-gate")
}

fn private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    // set_permissions follows links, so refuse a link planted where the folder should be.
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("{}: フォルダではない", path.display())));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

/// Appends one line with a single write so that parallel hooks do not interleave.
/// The file is private to the user and a symlink at its path is refused.
pub fn append_line(path: &Path, line: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        private_dir(parent)?;
    }
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(path)?;
    // A pipe, a device or a file that is also linked under another name is not our log.
    let meta = f.metadata()?;
    if !meta.is_file() || meta.nlink() != 1 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "log is not a private regular file"));
    }
    f.write_all(format!("{line}\n").as_bytes())
}

/// Reads a regular file of at most `limit` bytes. Pipes, devices and oversized files are errors,
/// so that a planted special file cannot hang the hook or exhaust memory.
pub fn read_regular(path: &Path, limit: u64) -> io::Result<String> {
    let invalid = |what: &str| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {what}", path.display()));
    let file = open_regular(path)?;
    let mut text = String::new();
    file.take(limit + 1).read_to_string(&mut text)?;
    if text.len() as u64 > limit {
        return Err(invalid("大きすぎる"));
    }
    Ok(text)
}

/// Writes through a new temporary file in the same folder and renames it over `path`.
/// The temporary file must not exist yet, and a rename replaces a symlink instead of writing through it.
pub fn write_atomic(path: &Path, bytes: &[u8], private_parent: bool) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no parent"))?;
    if private_parent {
        private_dir(parent)?;
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let tmp = parent.join(format!(".{name}.tmp-{}-{nanos}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW)
        .open(&tmp)?;
    file.write_all(bytes).and_then(|_| fs::rename(&tmp, path)).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// Removes control and bidirectional-override characters and shortens text taken from a report,
/// so that it cannot forge lines or hide content when shown in a terminal.
pub fn clip(text: &str, max_chars: usize) -> String {
    let clean: String = text
        .chars()
        .filter(|c| !c.is_control() && !matches!(*c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'))
        .collect();
    if clean.chars().count() <= max_chars {
        clean
    } else {
        format!("{}…", clean.chars().take(max_chars).collect::<String>())
    }
}

/// Current time as an ISO 8601 UTC string, without external crates.
pub fn now_iso() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}
