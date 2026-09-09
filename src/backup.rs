//! Keeping the backup directory fed, without ever making the user wait.
//!
//! Claude Code deletes sessions from `~/.claude/projects` once they pass
//! `cleanupPeriodDays`, so the only durable record of an old chat is a backup
//! taken while it still existed. Rather than rely on someone remembering to run
//! one, every chist invocation cheaply checks whether a backup is due and, if
//! it might be, hands the decision to a detached background process.
//!
//! The foreground never touches the backup directory: it is typically a
//! cloud-sync mount, and those block or fail in ways a CLI should not inherit.

use crate::archive::ArchiveStore;
use crate::config::{Config, expand_home};
use crate::formatters::format_list_json;
use crate::session_reader::SessionReader;
use chrono::{Local, NaiveDate};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

/// Set in the background child so it cannot spawn a backup of its own.
const NO_AUTO_ENV: &str = "CHIST_NO_AUTO_BACKUP";

/// How often the foreground is willing to even ask whether a backup is due.
const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// A lock older than this belonged to a run that died.
const STALE_LOCK: Duration = Duration::from_secs(6 * 60 * 60);

fn state_dir() -> PathBuf {
    std::env::var("XDG_STATE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/state"))
        .join("chist")
}

fn check_stamp() -> PathBuf {
    state_dir().join("last-backup-check")
}

fn lock_path() -> PathBuf {
    state_dir().join("backup.lock")
}

fn age(path: &Path) -> Option<Duration> {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
}

/// Fork a background backup if one might be due. Returns immediately; the work,
/// and every decision that needs the backup directory, happens in the child.
pub fn maybe_spawn(config: &Config) {
    if std::env::var_os(NO_AUTO_ENV).is_some() || !config.backup.auto {
        return;
    }

    // The only foreground filesystem access: one stat on a local file.
    if age(&check_stamp()).is_some_and(|a| a < CHECK_INTERVAL) {
        return;
    }
    // Stamp first, so a child that fails immediately cannot become a hot loop.
    let stamp = check_stamp();
    if let Some(parent) = stamp.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&stamp, Local::now().to_rfc3339());

    let Ok(exe) = std::env::current_exe() else {
        return;
    };

    // Backups are bulk IO on someone's laptop; get out of their way.
    let spawn = |program: &str, args: &[&str]| {
        Command::new(program)
            .args(args)
            .env(NO_AUTO_ENV, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    };

    let exe = exe.to_string_lossy().to_string();
    if spawn("nice", &["-n", "19", &exe, "backup", "--daemon"]).is_err() {
        let _ = spawn(&exe, &["backup", "--daemon"]);
    }
}

pub struct Outcome {
    pub archive: PathBuf,
    pub index: PathBuf,
    pub bytes: u64,
    pub skipped: Option<String>,
}

/// Run a backup. With `force` false this is a no-op unless one is due.
pub fn run(config: &Config, force: bool) -> io::Result<Outcome> {
    let store = ArchiveStore::new(&config.backup);
    let today = Local::now().date_naive();

    if !store.exists() {
        if !force {
            return Err(io::Error::other(format!(
                "backup directory {} does not exist",
                store.dir().display()
            )));
        }
        fs::create_dir_all(store.dir())?;
    }

    if !force
        && let Some(days) = days_since_last(&store, today)
        && days < config.backup.interval_days as i64
    {
        return Ok(Outcome {
            archive: PathBuf::new(),
            index: PathBuf::new(),
            bytes: 0,
            skipped: Some(format!(
                "last backup was {days}d ago, interval is {}d",
                config.backup.interval_days
            )),
        });
    }

    let _lock = Lock::acquire()?;

    let date = today.format("%d-%m-%Y").to_string();
    let index = store
        .dir()
        .join(format!("{}_{}.json", config.backup.index_prefix, date));
    let archive = store
        .dir()
        .join(format!("{}_{}.tar.gz", config.backup.archive_prefix, date));

    write_index(config, &index)?;
    let bytes = write_archive(config, &archive)?;

    Ok(Outcome {
        archive,
        index,
        bytes,
        skipped: None,
    })
}

fn days_since_last(store: &ArchiveStore, today: NaiveDate) -> Option<i64> {
    store
        .latest_backup_date()
        .map(|d| (today - d).num_days())
        .filter(|d| *d >= 0)
}

/// The session index that makes a later lookup cheap: without it, finding a
/// session means decompressing every archive in the directory.
fn write_index(config: &Config, path: &Path) -> io::Result<()> {
    let reader = SessionReader::new(&config.claude_home);
    let sessions = reader.list_sessions(
        Some(config.backup.index_limit),
        None,
        config.allowed_projects.as_deref(),
        false,
    );
    fs::write(path, format_list_json(&sessions))
}

/// Paths are passed to tar absolute, so members keep their full shape
/// (`home/alice/.claude/...`) and stay interchangeable with archives written
/// by hand from a shell.
fn write_archive(config: &Config, archive: &Path) -> io::Result<u64> {
    let mut paths: Vec<String> = Vec::new();
    for pattern in &config.backup.include {
        let expanded = expand_home(pattern);
        if expanded.contains('*') || expanded.contains('?') {
            if let Ok(hits) = glob::glob(&expanded) {
                paths.extend(hits.flatten().map(|p| p.to_string_lossy().to_string()));
            }
        } else if Path::new(&expanded).exists() {
            paths.push(expanded);
        }
    }

    if paths.is_empty() {
        return Err(io::Error::other(
            "nothing to back up — every configured include path is missing",
        ));
    }

    // Write beside the target and rename, so an interrupted run never leaves
    // something that looks like a complete archive.
    let partial = archive.with_extension("gz.partial");
    let out = Command::new("tar")
        .arg("-czf")
        .arg(&partial)
        .args(&paths)
        .stdin(Stdio::null())
        .output()?;

    // tar exits 1 when a file changed while being read — expected on a live
    // home directory, and not a reason to throw the archive away. 2 is fatal.
    if out.status.code() == Some(2) || (!out.status.success() && out.status.code() != Some(1)) {
        let _ = fs::remove_file(&partial);
        return Err(io::Error::other(
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .last()
                .unwrap_or("tar failed")
                .to_string(),
        ));
    }

    fs::rename(&partial, archive)?;
    Ok(fs::metadata(archive).map(|m| m.len()).unwrap_or(0))
}

/// A whole-machine lock, so two chist invocations cannot both start a backup.
struct Lock(PathBuf);

impl Lock {
    fn acquire() -> io::Result<Self> {
        let path = lock_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        if age(&path).is_some_and(|a| a > STALE_LOCK) {
            let _ = fs::remove_file(&path);
        }
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => Ok(Lock(path)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "a backup is already running",
            )),
            Err(e) => Err(e),
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Human-readable state of the backup directory.
pub fn status(config: &Config) -> String {
    let store = ArchiveStore::new(&config.backup);
    let today = Local::now().date_naive();
    let mut out = String::new();

    out.push_str(&format!("Backup directory : {}\n", store.dir().display()));
    out.push_str(&format!(
        "Automatic        : {}\n",
        if config.backup.auto {
            format!("on, every {}d", config.backup.interval_days)
        } else {
            "off".to_string()
        }
    ));

    if !store.exists() {
        out.push_str("Status           : directory does not exist (run `chist backup --now`)\n");
        return out;
    }

    let tarballs = store.tarballs();
    match tarballs.first() {
        Some((date, path)) => {
            let days = (today - *date).num_days();
            let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            out.push_str(&format!(
                "Latest archive   : {} ({}, {})\n",
                path.file_name().unwrap_or_default().to_string_lossy(),
                human_days(days),
                human_bytes(size),
            ));
            out.push_str(&format!("Archives         : {}\n", tarballs.len()));
            let due = config.backup.interval_days as i64 - days;
            out.push_str(&format!(
                "Next backup      : {}\n",
                if due <= 0 {
                    "due now".to_string()
                } else {
                    format!("in {due}d")
                }
            ));
        }
        None => out.push_str("Latest archive   : none yet\n"),
    }

    let indexes = store.indexes().len();
    out.push_str(&format!("Session indexes  : {indexes}\n"));
    if lock_path().exists() {
        out.push_str("Status           : a backup is running now\n");
    }
    out
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

pub fn human_days(days: i64) -> String {
    match days {
        d if d <= 0 => "today".to_string(),
        1 => "yesterday".to_string(),
        d if d < 30 => format!("{d} days ago"),
        d if d < 365 => format!("{} months ago", d / 30),
        d => format!("{} years ago", d / 365),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_render_at_a_readable_scale() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(109_372_433), "104.3 MB");
    }

    #[test]
    fn days_read_as_english() {
        assert_eq!(human_days(0), "today");
        assert_eq!(human_days(1), "yesterday");
        assert_eq!(human_days(9), "9 days ago");
        assert_eq!(human_days(95), "3 months ago");
        assert_eq!(human_days(800), "2 years ago");
    }

    #[test]
    fn lock_is_exclusive_and_released_on_drop() {
        let _ = fs::remove_file(lock_path());
        let held = Lock::acquire().expect("first acquire");
        assert!(Lock::acquire().is_err(), "second acquire must fail");
        drop(held);
        let again = Lock::acquire().expect("acquire after release");
        drop(again);
        assert!(!lock_path().exists());
    }
}
