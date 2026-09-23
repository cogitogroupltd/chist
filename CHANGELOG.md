# Changelog

All notable changes will be documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- `-H` / `--host <name>` reaches sessions on another machine's claude-runner
  through its client, `clrn`. `chist -r <alias> -H <host>` prints the clrn
  command that opens the session, for the shell wrapper to eval, just as a local
  resume prints `cd … && claude --resume`. `chist ls -H <host> [project]` lists
  what that runner holds. Hosts come from a new `hosts:` map in the config, or
  are taken as a tailnet machine name or a URL.
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
- `chist ls <pattern>` filters by session id, alias or project path, so
  `chist ls fior` finds every session under a fior path or with a fior alias.
  The pattern is a case-insensitive regex; one that will not compile — `*fior`
  — falls back to a substring match, so the glob spelling works too. It ANDs
  with `--project` and with `-i`, and `--limit` counts what matched.
- `chist -r` reads the session ID from stdin, so
  `chist ls -i 'something' | chist -r` resumes the first match. A bare `exec`
  at the end of a pipe does the same; `-r -` and `exec -` are the explicit
  spellings, and are the ones to use when stdin is a terminal. The ID is taken from the first column of
  the first result row, which also accepts a bare UUID piped in. An input with
  no session exits 1.
- `chist -rf <session>` resumes a session by forking it, leaving the original
  where it was. `-fr` works too, as does `chist exec <session> --fork` and a
  bare `-rf` at the end of a pipe. The fork is named after its parent —
  `<alias>-fork`, numbered `-fork-2` onward if that is taken — so the two do not
  sit in the listing under one name.
- `chist ls -i <pattern>` prints the lines that matched, grep style: a heading
  per session, then each hit with its role, timestamp and the matching line,
  windowed around the match and highlighted on a terminal. `-m/--max-matches`
  sets how many lines to show per session (5 by default, `0` for all), and
  `--tools` widens the search to tool calls, tool output and thinking. `-f json`
  carries the matches and their byte offsets. The session id still leads each
  heading, so `chist ls -i foo | chist -r` is unaffected.

### Changed
- `-i` now searches assistant replies as well as your own prompts, and no longer
  searches tool results by default — a hit inside a pasted file or a command's
  output is noise once the matching line is on screen. `--tools` restores the
  old reach.
- A search defaults to the new grep output rather than the session table;
  `-f table` asks for the table back.
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
- `--fork` never forked. It emitted `claude -rf <id>`, but `claude` has no `-f`,
  so the cluster was read as `-r f`: it resumed whatever session matched the
  text "f" and passed the real id in as a prompt. It now emits
  `claude --fork-session`.
- `chist -rf <session>` resumed a session matching "f" rather than forking, for
  the same reason on chist's own side: `-r` takes an optional value, so clap
  read `-rf` as `-r=f`. The cluster is now split before parsing.
- A failed lookup suggested `chist restore <id>` for archived sessions that are
  still in `~/.claude`. Restore correctly declines to touch a live session, so
  the suggested command did nothing — and the row now says
  `(still in ~/.claude)` and suggests `chist -r <id>`, which is what was wanted.
- Project directories whose path components contain a `.`, `_`, space or
  parenthesis decoded to the wrong path — Claude replaces every
  non-alphanumeric character with a dash, not just `/`, so
  `.../fiordc.aigateway.fior.group` came back as `.../fiordc/aigateway/fior/group`
  and `chist -r` cd'd nowhere. Components are now recovered by matching real
  directory entries against their own encoded form.
- `chist list` truncated the branch and last-message columns by byte index,
  which panicked on multibyte text.
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
