# chist

A small CLI for browsing the chat sessions Claude Code stores under `~/.claude/projects/`.

After a few weeks of using Claude Code I had hundreds of session files and no good way to find anything. `chist` is what I wrote to fix that for myself. It lists every session, lets you grep through them, and prints the right `cd` + `claude --resume` to drop you back into one.

## Install

If you have a Rust toolchain:

```
cargo install --git https://github.com/cogitogroupltd/chist
```

Or build from source:

```
git clone https://github.com/cogitogroupltd/chist
cd chist
cargo build --release
cp target/release/chist ~/.local/bin/
```

That's it. `chist --version` should now work.

## Use

### List your sessions

```
$ chist list
 ID        Alias                  Project                Branch   Status   Size     Msgs  Updated           Last Msg
 c00abb55  redis-migration-bug    ~/dev/myco/api         main     Running  911.5KB  401   2026-05-09 12:17  ok push it
 8b54e7bd  search-tuning          ~/dev/myco/web         main     Running    3.1MB  911   2026-05-09 12:16  one more pass on the tokenizer
 b28ffd10  postgres-upgrade-plan  ~/work/billing         main              1.8MB    660   2026-05-08 22:04  let's stage this on prod-replica first
 65c5fc64  investor-pitch         ~/Documents/decks               2.7MB    684                  2026-05-08 12:07  4 bullet points, drop the screenshot
```

Newest activity first. The `Status` column shows `Running` if a `claude` process is currently attached to the session. `chist list -l 100` shows more rows; `chist list -a` includes `/tmp` projects (excluded by default because they're usually noise).

### Search by content

`-i` matches the JSONL contents of every session — first prompt, every message, summary. Add `--regex` for a regex.

```
$ chist list -i 'redis migration'
 ID        Alias                  Project                Branch   Status   Size     Msgs  Updated           Last Msg
 c00abb55  redis-migration-bug    ~/dev/myco/api         main     Running  911.5KB  401   2026-05-09 12:17  ok push it
 6ca5dc20  cache-rewrite          ~/dev/myco/api         main              4.1MB    1399  2026-05-04 11:22  rolled back, see incident #482

$ chist list -i '(redis|postgres) migration' --regex
 ID        Alias                  Project                Branch   Status   Size     Msgs  Updated           Last Msg
 c00abb55  redis-migration-bug    ~/dev/myco/api         main     Running  911.5KB  401   2026-05-09 12:17  ok push it
 b28ffd10  postgres-upgrade-plan  ~/work/billing         main              1.8MB    660   2026-05-08 22:04  let's stage this on prod-replica first
 6ca5dc20  cache-rewrite          ~/dev/myco/api         main              4.1MB    1399  2026-05-04 11:22  rolled back, see incident #482
```

### Inspect a single session

You can pass a UUID prefix, the full UUID, or a slug.

```
$ chist get c00abb55
Session: redis-migration-bug (c00abb55-5251-4db9-a6bb-8d72ea832873)

Project: ~/dev/myco/api
Started: 2026-05-08 19:39:29
Duration: 47 minutes

Messages:
  User: 166
  Assistant: 238
  Total: 404

Tools Used:
  Bash: 62
  Edit: 18
  Read: 14
  Write: 1

Git Activity:
  Branch: main
  Commits: 2
  Pushes: 1

Token Usage:
  Input: 436
  Output: 154,795
  Total: 155,231

First Prompt:
  the redis migration is failing on the staging replica — getting a CROSSSLOT
  error on the bulk MGET. can you take a look at scripts/migrate_keys.py?
```

`chist get --last` shows the most recently active session. `-f json` gives you a full structured dump suitable for piping into `jq`.

### Resume a session

`exec` doesn't run `claude` itself — it prints the shell command that would. That keeps it safe to inspect, pipe, or alias.

```
$ chist exec c00abb55
cd ~/dev/myco/api && claude -r c00abb55-5251-4db9-a6bb-8d72ea832873

$ eval "$(chist exec c00abb55)"
# claude opens with the conversation restored
```

If you don't want to type `eval` every time, drop a small wrapper into your shell:

```
chist() {
  case "$1" in
    exec|-r|--resume)
      local cmd; cmd=$(command chist "$@")
      [[ -n "$cmd" ]] && eval "$cmd" ;;
    *) command chist "$@" ;;
  esac
}
```

A more elaborate version is in [`docs/shell-wrapper.zsh`](docs/shell-wrapper.zsh) — it adds `save`/`restore` for konsole tabs.

## Why this exists

Claude Code already has `claude --resume` and a built-in picker, but the picker only shows sessions that started in the *current* working directory. If I worked on something three weeks ago and don't remember which project I was in, the built-in tooling can't help. `chist` reads the JSONL files directly, so it sees every session everywhere, and you can search by content rather than by directory.

## Configuration

`chist` looks for an optional YAML config at `~/.config/chist/config.yaml`. Everything is optional:

```yaml
claude_home: ~/.claude        # where Claude Code stores its data
allowed_projects:             # whitelist; if absent, all projects show up
  - ~/dev/**
  - ~/tmp/**
defaults:
  list_limit: 50              # default for `chist list` (CLI -l overrides)
  format: table               # or "json"
```

The legacy paths `~/.chist.yaml` and `~/.claudehist.yaml` are also read if the new file isn't there, so older configs keep working.

The cache (slug lookups) lives at `~/.cache/chist/`. It's safe to delete; it'll regenerate.

## Caveats

- `chist` reads JSONL files. It doesn't talk to the Anthropic API.
- It assumes Claude Code's on-disk format (the layout under `~/.claude/projects/`). When that format changes, `chist` will need a patch.
- Slugs are heuristics; resume by UUID prefix if you need certainty.

## Licence

MIT. See [LICENSE](LICENSE).
