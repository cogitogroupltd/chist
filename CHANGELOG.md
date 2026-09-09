# Changelog

All notable changes will be documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- `chist -r` reads the session ID from stdin, so
  `chist ls -i 'something' | chist -r` resumes the first match. `-r -` and
  `exec -` are the explicit spellings. The ID is taken from the first column of
  the first result row, which also accepts a bare UUID piped in. An input with
  no session exits 1.

### Fixed
- Project directories whose path components contain a `.`, `_`, space or
  parenthesis decoded to the wrong path — Claude replaces every
  non-alphanumeric character with a dash, not just `/`, so
  `.../fiordc.aigateway.fior.group` came back as `.../fiordc/aigateway/fior/group`
  and `chist -r` cd'd nowhere. Components are now recovered by matching real
  directory entries against their own encoded form.
- `chist list` truncated the branch and last-message columns by byte index,
  which panicked on multibyte text.

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
