# Changelog

All notable changes will be documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- `chist backup` — archives the Claude home to a backup directory as
  `<prefix>_DD-MM-YYYY.tar.gz`, alongside a `claude_sessions_DD-MM-YYYY.json`
  index of the sessions it contains. `--status` reports where archives live and
  when the last one ran.
- Automatic backups. Every invocation does one stat on a local stamp file and,
  at most hourly, forks a detached low-priority process that decides whether a
  backup is due. The archive directory is never touched in the foreground, so a
  stalled cloud-sync mount cannot wedge the CLI. A lock file prevents overlap.
  The default interval is `cleanupPeriodDays / 4`, capped at 7 days, so no
  session can be created and reaped between two backups.
- Archive fallback on a miss. `chist -r`, `exec` and `get` now search the
  backup indexes when a session is not on disk, and offer to restore it —
  the prompt goes to stderr and reads `/dev/tty`, so it works inside
  `eval $(chist -r ...)`. `chist list -i` reports archive matches without
  restoring. Restored sessions get a fresh mtime so Claude's cleanup pass does
  not immediately reap them again.
- `chist restore <query>` restores a session explicitly; `--list` shows what the
  archives hold. When a session appears in several archives the freshest copy
  wins. A session still present in `~/.claude` is never replaced by an older
  archived copy without `--force`, and even then the live copy is kept as
  `<uuid>.jsonl.replaced-<timestamp>`.
- `backup:` section in the config, plus `CHIST_BACKUP_DIR` and
  `CHIST_NO_AUTO_BACKUP`.

### Changed
- The config parser handles nested maps generally, rather than only `defaults:`.
- A failed lookup now says what *is* on disk. `chist -r <name>` that matches
  nothing lists local sessions whose slug or project path contains the query,
  which is the answer when someone types a directory name rather than a session
  name, and names archived sessions that merely mention the text without
  offering to restore them.
- The archive restore prompt only fires for a session the query actually names
  (UUID or slug). A full-text hit is a search result, not an intent.
- Archived sessions are no longer matched on `project_path`: every session in a
  repository matched that repository's own name.
- A lookup reads each backup index once rather than twice.

### Fixed
- `chist -r <query>` could offer to restore a session that was still on disk,
  under a message saying it was not, and then fail with the contradiction
  ("Not in ~/.claude any more" followed by "that session is still in
  ~/.claude"). Sessions present locally are no longer offered.

## [0.3.0] — 2026-05-08

First public release. The tool was previously developed as `cog-claudehist`
inside an internal monorepo; this is the same code, renamed and cleaned up
for open use.

### Added
- Default config path is now `~/.config/chist/config.yaml`. The legacy
  `~/.chist.yaml` and `~/.claudehist.yaml` paths are still read as fallbacks.
- `chist exec` now resolves to the JSONL's most recently recorded `cwd`
  rather than the directory `claude` was originally launched in. Sessions
  that `cd`'d into a sub-project mid-conversation now resume in the right
  place.

### Fixed
- `chist list` showed the session start time in the "Updated" column. It now
  shows the timestamp of the last message, which is what you'd expect.
- The list sort order matches the displayed column (last-activity descending).

### Changed
- `chist exec` now emits `claude --resume` rather than `c --resume`, so the
  output works for users who don't have a personal `c` alias for `claude`.
