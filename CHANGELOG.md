# Changelog

All notable changes will be documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

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
