# chist

A small CLI for browsing the chat sessions Claude Code stores under `~/.claude/projects/`.

After a few weeks of using Claude Code I had hundreds of session files and no good way to find anything. `chist` is what I wrote to fix that for myself. It lists every session, lets you grep through them, and prints the right `cd` + `claude --resume` to drop you back into one.

## Install

If you have a Rust toolchain:

    cargo install --git https://github.com/cogitogroupltd/chist

Or build from source:

    git clone https://github.com/cogitogroupltd/chist
    cd chist
    cargo build --release
    cp target/release/chist ~/.local/bin/

That's it. `chist --version` should now work.

## Use

List your sessions, newest first:

    chist list

Find every session that mentions a phrase:

    chist list -i 'redis migration'
    chist list -i '(redis|postgres) migration' --regex

Inspect one:

    chist get <uuid-prefix>
    chist get my-session-slug
    chist get --last

Resume one. The `exec` subcommand prints a shell command that `cd`s into the session's project directory and runs `claude --resume`:

    eval "$(chist exec <uuid-prefix>)"

Or, if you don't want to type `eval` every time, drop a small wrapper into your shell:

    chist() {
      case "$1" in
        exec|-r|--resume)
          local cmd; cmd=$(command chist "$@")
          [[ -n "$cmd" ]] && eval "$cmd" ;;
        *) command chist "$@" ;;
      esac
    }

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
