//! Reading sessions back out of backup archives.
//!
//! A backup directory holds, per run date, a tarball of the Claude home and a
//! small JSON index of the sessions it contained:
//!
//! ```text
//! kube_aws_zsh_hist_22-06-2026.tar.gz
//! claude_sessions_22-06-2026.json
//! ```
//!
//! Because the index is small and the tarball is not, a lookup reads indexes
//! only; the tarball is opened once, for the session actually wanted.

use crate::config::BackupConfig;
use crate::path_utils::path_to_claude_dir_name;
use chrono::NaiveDate;
use serde::Deserialize;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// One session as recorded in a backup index. Every field is optional so that
/// indexes written by older versions of chist still parse.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct IndexEntry {
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub project_path: String,
    #[serde(default)]
    pub first_prompt: String,
    #[serde(default)]
    pub last_message: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub last_activity: String,
}

/// A session found in an archive, and the archive holding it.
#[derive(Debug, Clone)]
pub struct ArchivedSession {
    pub entry: IndexEntry,
    /// Date of the backup run this copy came from.
    pub archive_date: NaiveDate,
    pub tarball: PathBuf,
}

impl ArchivedSession {
    pub fn label(&self) -> String {
        match self.entry.slug {
            Some(ref s) if !s.is_empty() => format!("{} ({})", s, &self.short_id()),
            _ => self.short_id(),
        }
    }

    /// The date the session was last used, as `YYYY-MM-DD`.
    pub fn last_active(&self) -> Option<&str> {
        let stamp = if self.entry.last_activity.is_empty() {
            return None;
        } else {
            &self.entry.last_activity
        };
        stamp.get(..10)
    }

    pub fn short_id(&self) -> String {
        self.entry.session_id.chars().take(8).collect::<String>()
    }
}

#[derive(Clone)]
pub struct ArchiveStore {
    dir: PathBuf,
    index_prefix: String,
}

impl ArchiveStore {
    pub fn new(config: &BackupConfig) -> Self {
        ArchiveStore {
            dir: config.dir.clone(),
            index_prefix: config.index_prefix.clone(),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn exists(&self) -> bool {
        self.dir.is_dir()
    }

    /// Backup indexes, newest first.
    pub fn indexes(&self) -> Vec<(NaiveDate, PathBuf)> {
        self.dated_files(Some(&self.index_prefix), ".json")
    }

    /// Backup tarballs, newest first. Any `*_DD-MM-YYYY.tar.gz` counts, so a
    /// renamed `archive_prefix` does not orphan older archives.
    pub fn tarballs(&self) -> Vec<(NaiveDate, PathBuf)> {
        self.dated_files(None, ".tar.gz")
    }

    /// The date of the most recent backup, by filename not mtime — a cloud-sync
    /// mount rewrites mtimes, but the name is what the run itself stamped.
    pub fn latest_backup_date(&self) -> Option<NaiveDate> {
        self.tarballs().first().map(|(d, _)| *d)
    }

    fn dated_files(&self, prefix: Option<&str>, suffix: &str) -> Vec<(NaiveDate, PathBuf)> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };

        let mut found: Vec<(NaiveDate, PathBuf)> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                let stem = name.strip_suffix(suffix)?;
                let stem = match prefix {
                    Some(p) => stem.strip_prefix(&format!("{p}_"))?,
                    // No prefix given: take whatever sits after the last '_'.
                    None => stem.rsplit_once('_').map(|(_, d)| d)?,
                };
                let date = NaiveDate::parse_from_str(stem, "%d-%m-%Y").ok()?;
                Some((date, e.path()))
            })
            .collect();

        found.sort_by_key(|(date, _)| std::cmp::Reverse(*date));
        found
    }

    fn read_index(path: &Path) -> Vec<IndexEntry> {
        fs::read_to_string(path)
            .ok()
            .and_then(|c| serde_json::from_str::<Vec<IndexEntry>>(&c).ok())
            .unwrap_or_default()
    }

    /// Both kinds of match in a single pass over the indexes: sessions the
    /// query names (UUID or slug), and sessions that merely mention it.
    /// Two `find` calls would read every index twice, which on a cloud-sync
    /// mount is the difference between a pause and a stall.
    pub fn find_all(&self, query: &str) -> (Vec<ArchivedSession>, Vec<ArchivedSession>) {
        let indexes = self.load_indexes();
        let named = self.find_inner(query, true, &indexes);
        let named_ids: Vec<String> = named.iter().map(|h| h.entry.session_id.clone()).collect();
        let mentioned = self
            .find_inner(query, false, &indexes)
            .into_iter()
            .filter(|h| !named_ids.contains(&h.entry.session_id))
            .collect();
        (named, mentioned)
    }

    /// Find archived sessions matching `query`, freshest copy first.
    ///
    /// `exact` matches the way `chist get`/`exec` do — full UUID, UUID prefix,
    /// or slug. Otherwise the query is a case-insensitive substring over the
    /// slug, prompt, last message and summary, mirroring `chist list -i`.
    pub fn find(&self, query: &str, exact: bool) -> Vec<ArchivedSession> {
        self.find_inner(query, exact, &self.load_indexes())
    }

    /// Every index read once, paired with the tarball it describes. The backup
    /// directory is typically a cloud-sync mount, where a read_dir per index
    /// turns a lookup into a stall.
    fn load_indexes(&self) -> Vec<(NaiveDate, PathBuf, Vec<IndexEntry>)> {
        let tarballs = self.tarballs();
        self.indexes()
            .into_iter()
            .filter_map(|(date, index)| {
                // An index with no tarball beside it cannot be restored from.
                let tarball = tarballs.iter().find(|(d, _)| *d == date)?.1.clone();
                Some((date, tarball, Self::read_index(&index)))
            })
            .collect()
    }

    fn find_inner(
        &self,
        query: &str,
        exact: bool,
        indexes: &[(NaiveDate, PathBuf, Vec<IndexEntry>)],
    ) -> Vec<ArchivedSession> {
        let needle = query.to_lowercase();
        let mut hits: Vec<ArchivedSession> = Vec::new();

        for (date, tarball, entries) in indexes {
            let date = *date;
            for entry in entries.iter().cloned() {
                let matched = if exact {
                    entry.session_id == query
                        || (query.len() >= 4 && entry.session_id.starts_with(query))
                        || entry.slug.as_deref() == Some(query)
                } else {
                    // Deliberately not project_path: every session in a repo
                    // would match the repo's own name, and a directory is not
                    // what someone means when they name a session.
                    let haystack = [
                        entry.slug.clone().unwrap_or_default(),
                        entry.first_prompt.clone(),
                        entry.last_message.clone().unwrap_or_default(),
                        entry.summary.clone().unwrap_or_default(),
                    ]
                    .join("\n")
                    .to_lowercase();
                    haystack.contains(&needle)
                };

                if !matched {
                    continue;
                }
                // Indexes are walked newest first, so the first copy of a
                // session is the freshest one and later duplicates are stale.
                if hits.iter().any(|h| h.entry.session_id == entry.session_id) {
                    continue;
                }
                hits.push(ArchivedSession {
                    entry,
                    archive_date: date,
                    tarball: tarball.clone(),
                });
            }
        }

        hits
    }

    /// Extract a session's files from its tarball back into `claude_home`.
    /// Returns the restored `.jsonl` path.
    ///
    /// A session that is still on disk is never overwritten unless `overwrite`
    /// is set, and even then the live copy is moved aside rather than lost —
    /// the archived copy is by definition older, and the difference is somebody's
    /// conversation.
    pub fn restore(
        &self,
        session: &ArchivedSession,
        claude_home: &Path,
        overwrite: bool,
    ) -> io::Result<PathBuf> {
        let id = &session.entry.session_id;
        if id.is_empty() {
            return Err(io::Error::other("archived session has no id"));
        }

        let live = local_copy(claude_home, id);
        if live.is_some() && !overwrite {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "that session is still in ~/.claude — restoring would replace it",
            ));
        }

        // Unique per call: two restores must never share a staging directory,
        // or one will delete the other's extraction out from under it.
        let staging = std::env::temp_dir().join(format!(
            "chist-restore-{}-{}-{}",
            std::process::id(),
            session.short_id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default(),
        ));
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir_all(&staging)?;

        let result = (|| {
            extract(&session.tarball, &format!("*{id}*"), &staging)?;

            let jsonl = find_file(&staging, &format!("{id}.jsonl"))
                .ok_or_else(|| io::Error::other(format!("{id}.jsonl not found in archive")))?;

            // The directory the archive filed it under is Claude's own encoding
            // of the project path; keep it rather than re-deriving it.
            let project_dir = jsonl
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path_to_claude_dir_name(&session.entry.project_path));

            let dest_dir = claude_home.join("projects").join(project_dir);
            fs::create_dir_all(&dest_dir)?;

            // Never destroy the live copy: park it beside the restore so the
            // newer conversation is still there if this was the wrong call.
            if let Some(ref live) = live {
                let parked = live.with_extension(format!(
                    "jsonl.replaced-{}",
                    chrono::Local::now().format("%Y%m%dT%H%M%S")
                ));
                fs::rename(live, &parked)?;
            }

            // fs::copy stamps the destination with the current time, which is
            // what keeps Claude's cleanup pass from reaping it again at once.
            let dest = dest_dir.join(format!("{id}.jsonl"));
            fs::copy(&jsonl, &dest)?;

            // Newer sessions keep a sidecar directory of tool results.
            if let Some(sidecar) = find_dir(&staging, id) {
                copy_tree(&sidecar, &dest_dir.join(id))?;
            }

            Ok(dest)
        })();

        let _ = fs::remove_dir_all(&staging);
        result
    }
}

/// The session's `.jsonl` inside a Claude home, if it is still there.
/// Sessions live at `projects/<encoded project dir>/<uuid>.jsonl`, so this
/// looks exactly there rather than walking the whole tree.
pub fn local_copy(claude_home: &Path, session_id: &str) -> Option<PathBuf> {
    let pattern = claude_home
        .join("projects/*")
        .join(format!("{session_id}.jsonl"));
    glob::glob(&pattern.to_string_lossy())
        .ok()?
        .flatten()
        .find(|p| p.is_file())
}

/// Pull members matching `pattern` out of a gzipped tarball.
fn extract(archive: &Path, pattern: &str, dest: &Path) -> io::Result<()> {
    let run = |wildcards: bool| -> io::Result<std::process::Output> {
        let mut cmd = Command::new("tar");
        cmd.arg("-xzf").arg(archive).arg("-C").arg(dest);
        if wildcards {
            // GNU tar needs this to treat the member name as a glob; bsdtar
            // globs by default and rejects the flag.
            cmd.arg("--wildcards");
        }
        cmd.arg(pattern).output()
    };

    let out = run(true)?;
    if out.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if stderr.contains("wildcards") || stderr.contains("nrecognized") || stderr.contains("nknown") {
        let retry = run(false)?;
        if retry.status.success() {
            return Ok(());
        }
        return Err(io::Error::other(
            String::from_utf8_lossy(&retry.stderr).trim().to_string(),
        ));
    }

    Err(io::Error::other(stderr.trim().to_string()))
}

fn find_file(root: &Path, name: &str) -> Option<PathBuf> {
    walk(root, &mut |p| {
        p.is_file() && p.file_name().is_some_and(|n| n == name)
    })
}

fn find_dir(root: &Path, name: &str) -> Option<PathBuf> {
    walk(root, &mut |p| {
        p.is_dir() && p.file_name().is_some_and(|n| n == name)
    })
}

fn walk(dir: &Path, pred: &mut dyn FnMut(&Path) -> bool) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if pred(&path) {
            return Some(path);
        }
        // file_type() does not follow symlinks, so a link cannot walk us out
        // of the directory we were asked about.
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            subdirs.push(path);
        }
    }
    subdirs.iter().find_map(|d| walk(d, pred))
}

fn copy_tree(src: &Path, dest: &Path) -> io::Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)?.flatten() {
        let path = entry.path();
        let target = dest.join(entry.file_name());
        if path.is_dir() {
            copy_tree(&path, &target)?;
        } else {
            fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RestoreMode;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("chist-archtest-{}-{}", std::process::id(), tag));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn store_at(dir: &Path) -> ArchiveStore {
        ArchiveStore::new(&BackupConfig {
            dir: dir.to_path_buf(),
            auto: false,
            interval_days: 7,
            archive_prefix: "kube_aws_zsh_hist".into(),
            index_prefix: "claude_sessions".into(),
            index_limit: 500,
            include: vec![],
            restore: RestoreMode::Ask,
        })
    }

    /// Build a backup directory holding one session, exactly as `chist backup`
    /// would write it, and read it back out again.
    fn seed(dir: &Path, date: &str, session_id: &str, slug: &str) -> PathBuf {
        let home = dir.join("fakehome");
        let project_dir = home.join(".claude/projects/-home-alice-tmp");
        fs::create_dir_all(project_dir.join(session_id)).unwrap();
        fs::write(
            project_dir.join(format!("{session_id}.jsonl")),
            "{\"type\":\"user\"}\n",
        )
        .unwrap();
        fs::write(project_dir.join(session_id).join("tool.txt"), "result").unwrap();

        fs::write(
            dir.join(format!("claude_sessions_{date}.json")),
            serde_json::to_string(&serde_json::json!([{
                "session_id": session_id,
                "slug": slug,
                "project_path": "/home/alice/tmp",
                "project_name": "alice/tmp",
                "start_time": "2026-05-29T06:42:47.754Z",
                "first_prompt": "review the consultancy agreement",
                "last_message": "Wheres the file?",
                "summary": null,
                "last_activity": "2026-06-01T19:30:07.013Z"
            }]))
            .unwrap(),
        )
        .unwrap();

        let tarball = dir.join(format!("kube_aws_zsh_hist_{date}.tar.gz"));
        let status = Command::new("tar")
            .arg("-czf")
            .arg(&tarball)
            .arg("-C")
            .arg(&home)
            .arg(".claude")
            .status()
            .unwrap();
        assert!(status.success());
        fs::remove_dir_all(&home).unwrap();
        tarball
    }

    #[test]
    fn finds_and_restores_a_session_from_an_archive() {
        let dir = scratch("restore");
        let id = "b966b18e-66f0-49cc-b4f3-b00abd220457";
        seed(&dir, "22-06-2026", id, "dw-contract");
        let store = store_at(&dir);

        // by slug, by uuid prefix, and by free-text search
        assert_eq!(store.find("dw-contract", true).len(), 1);
        assert_eq!(store.find("b966b18e", true).len(), 1);
        assert_eq!(store.find("consultancy", false).len(), 1);
        assert!(store.find("no-such-session", true).is_empty());

        let hit = store.find("dw-contract", true).remove(0);
        assert_eq!(
            hit.archive_date,
            NaiveDate::from_ymd_opt(2026, 6, 22).unwrap()
        );

        let claude_home = dir.join("restored/.claude");
        let restored = store.restore(&hit, &claude_home, false).unwrap();
        assert_eq!(
            restored,
            claude_home
                .join("projects/-home-alice-tmp")
                .join(format!("{id}.jsonl"))
        );
        assert!(restored.exists());
        // the tool-results sidecar comes back too
        assert!(
            claude_home
                .join("projects/-home-alice-tmp")
                .join(id)
                .join("tool.txt")
                .exists()
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn refuses_to_overwrite_a_session_that_is_still_live() {
        let dir = scratch("noclobber");
        let id = "b966b18e-66f0-49cc-b4f3-b00abd220457";
        seed(&dir, "22-06-2026", id, "dw-contract");
        let store = store_at(&dir);
        let hit = store.find("dw-contract", true).remove(0);

        let claude_home = dir.join("restored/.claude");
        let live_dir = claude_home.join("projects/-home-alice-tmp");
        fs::create_dir_all(&live_dir).unwrap();
        let live = live_dir.join(format!("{id}.jsonl"));
        fs::write(&live, "a newer conversation than the archive holds").unwrap();

        let err = store.restore(&hit, &claude_home, false).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read_to_string(&live).unwrap(),
            "a newer conversation than the archive holds",
            "the live session must be untouched"
        );

        // Forced, the live copy is parked rather than destroyed.
        store.restore(&hit, &claude_home, true).unwrap();
        let parked: Vec<_> = fs::read_dir(&live_dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".replaced-"))
            .collect();
        assert_eq!(parked.len(), 1, "the replaced session must be kept");
        assert_eq!(
            fs::read_to_string(parked[0].path()).unwrap(),
            "a newer conversation than the archive holds"
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prefers_the_freshest_copy_of_a_duplicated_session() {
        let dir = scratch("freshest");
        let id = "b966b18e-66f0-49cc-b4f3-b00abd220457";
        seed(&dir, "02-06-2026", id, "dw-contract");
        seed(&dir, "22-06-2026", id, "dw-contract");

        let hits = store_at(&dir).find("dw-contract", true);
        assert_eq!(hits.len(), 1, "the same session must not be offered twice");
        assert_eq!(
            hits[0].archive_date,
            NaiveDate::from_ymd_opt(2026, 6, 22).unwrap()
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_directory_name_does_not_match_every_session_in_it() {
        let dir = scratch("projectmatch");
        seed(
            &dir,
            "22-06-2026",
            "b966b18e-66f0-49cc-b4f3-b00abd220457",
            "dw-contract",
        );
        let store = store_at(&dir);

        // The seeded session lives in /home/alice/tmp. Typing the directory
        // name must not offer it as though it were a session called that.
        assert!(
            store.find("tmp", false).is_empty(),
            "project path must not be part of the search haystack"
        );
        // Content still matches.
        assert_eq!(store.find("consultancy", false).len(), 1);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ignores_an_index_with_no_tarball_beside_it() {
        let dir = scratch("orphan");
        seed(
            &dir,
            "22-06-2026",
            "aaaaaaaa-0000-0000-0000-000000000000",
            "orphan",
        );
        fs::remove_file(dir.join("kube_aws_zsh_hist_22-06-2026.tar.gz")).unwrap();

        assert!(store_at(&dir).find("orphan", true).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn latest_backup_date_reads_the_filename_not_the_mtime() {
        let dir = scratch("latest");
        seed(
            &dir,
            "02-06-2026",
            "aaaaaaaa-0000-0000-0000-000000000000",
            "old",
        );
        seed(
            &dir,
            "22-06-2026",
            "bbbbbbbb-0000-0000-0000-000000000000",
            "new",
        );

        assert_eq!(
            store_at(&dir).latest_backup_date(),
            Some(NaiveDate::from_ymd_opt(2026, 6, 22).unwrap())
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
