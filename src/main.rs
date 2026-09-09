mod archive;
mod backup;
mod config;
mod formatters;
mod models;
mod path_utils;
mod session_reader;

use archive::{ArchiveStore, ArchivedSession, local_copy};
use backup::human_days;
use clap::{Parser, Subcommand};
use config::{Config, RestoreMode};
use formatters::{
    format_detail_json, format_detail_text, format_detail_yaml, format_list_json, format_list_table,
};
use session_reader::SessionReader;
use std::fs::File;
use std::io::{self, BufRead, BufReader, IsTerminal, Write};
use std::path::PathBuf;
use std::process;
use std::sync::mpsc;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "chist",
    version = env!("CARGO_PKG_VERSION"),
    about = "View and manage Claude Code chat sessions",
    after_help = r#"Examples:
  chist list                              # List recent sessions
  chist list -l 10                        # List 10 most recent sessions
  chist list --project cog                # Filter by project name
  chist list -f json                      # Output as JSON
  chist list -a                           # Include /tmp sessions
  chist list -i 'search string'           # Search sessions (case-insensitive)
  chist list -i '(regex|pattern)' --regex # Search with regex
  chist get --last                        # Get latest session
  chist get lively-cooking-hejlsberg      # Get by slug
  chist get 3f4b4b02                      # Get by UUID prefix
  chist exec frolicking-stirring-unicorn   # Resume session in its project dir
  eval $(chist exec 3f4b4b02)              # Same, by UUID prefix
  chist ls -i 'search string' | chist -r    # Resume the first match
  chist ls -i 'search string' | chist exec  # Same, spelled as the subcommand
  chist -r my-alias -e 'git status'        # Run one prompt non-interactively
  chist backup                             # Back up now
  chist backup --status                    # When the last backup ran
  chist restore dw-contract                # Pull a reaped session out of a backup"#
)]
struct Cli {
    /// Path to config file (default: ~/.config/chist/config.yaml)
    #[arg(long)]
    config: Option<PathBuf>,

    /// Resume a session (shorthand for `exec <id>`). Bare `-r`, or `-r -`,
    /// takes the session from stdin: `chist ls -i foo | chist -r`.
    #[arg(
        short = 'r',
        long = "resume",
        num_args = 0..=1,
        default_missing_value = "-"
    )]
    resume: Option<String>,

    /// Execute a single prompt non-interactively (use with -r)
    #[arg(short = 'e', long = "execute", requires = "resume")]
    execute: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// List Claude Code sessions
    #[command(alias = "ls")]
    List {
        /// Maximum number of sessions to show
        #[arg(short, long)]
        limit: Option<usize>,

        /// Filter by project name (substring match)
        #[arg(short, long)]
        project: Option<String>,

        /// Search sessions for pattern (case-insensitive)
        #[arg(short = 'i', long = "search")]
        search: Option<String>,

        /// Treat -i pattern as regex
        #[arg(long = "regex")]
        regex: bool,

        /// Output format
        #[arg(short, long, value_parser = ["table", "json"])]
        format: Option<String>,

        /// Include sessions from /tmp directories
        #[arg(short = 'a', long = "all")]
        all: bool,
    },

    /// Resume a session: cd into its project and launch claude --resume
    #[command(alias = "e")]
    Exec {
        /// Session ID (UUID), UUID prefix, or slug
        id_or_slug: Option<String>,

        /// Exec into the last session
        #[arg(short, long)]
        last: bool,

        /// Fork the session instead of resuming in-place
        #[arg(short, long)]
        fork: bool,

        /// Execute a single prompt non-interactively (--print mode)
        #[arg(short = 'e', long = "execute")]
        execute: Option<String>,

        /// Include sessions from /tmp directories
        #[arg(short = 'a', long = "all")]
        all: bool,
    },

    /// Get detailed session information
    Get {
        /// Session ID (UUID), UUID prefix, or slug
        id_or_slug: Option<String>,

        /// Get the last session
        #[arg(short, long)]
        last: bool,

        /// Output format
        #[arg(short, long, value_parser = ["text", "json", "yaml"])]
        format: Option<String>,

        /// Include sessions from /tmp directories
        #[arg(short = 'a', long = "all")]
        all: bool,
    },

    /// Archive the Claude home to the backup directory
    Backup {
        /// Show where backups live and when the last one ran
        #[arg(short, long)]
        status: bool,

        /// Back up only if one is due, quietly (used by the background runner)
        #[arg(long, hide = true)]
        daemon: bool,
    },

    /// Restore a session from a backup archive back into ~/.claude
    Restore {
        /// Session ID (UUID), UUID prefix, slug, or text to search for
        query: Option<String>,

        /// Show matching archived sessions without restoring anything
        #[arg(short, long)]
        list: bool,

        /// Restore without asking
        #[arg(short, long)]
        yes: bool,

        /// Replace a session that is still in ~/.claude (the live copy is kept
        /// alongside as <uuid>.jsonl.replaced-<timestamp>)
        #[arg(long)]
        force: bool,
    },
}

fn main() {
    let cli = Cli::parse();

    // -r <id> is a shorthand for `exec <id>`
    if let Some(ref id) = cli.resume {
        let config = Config::load(cli.config.as_deref());
        backup::maybe_spawn(&config);
        cmd_exec(
            &config,
            Some(id.clone()),
            false,
            false,
            false,
            cli.execute.as_deref(),
        );
        return;
    }

    let Some(command) = cli.command else {
        print_banner();
        let _ = Cli::parse_from(["claudehist", "--help"]);
        return;
    };

    let config = Config::load(cli.config.as_deref());

    // Backups are the one command that must not trigger a backup of its own.
    if !matches!(command, Commands::Backup { .. }) {
        backup::maybe_spawn(&config);
    }

    match command {
        Commands::List {
            limit,
            project,
            search,
            regex,
            format,
            all,
        } => cmd_list(&config, limit, project, search, regex, format, all),
        Commands::Exec {
            id_or_slug,
            last,
            fork,
            execute,
            all,
        } => cmd_exec(&config, id_or_slug, last, fork, all, execute.as_deref()),
        Commands::Get {
            id_or_slug,
            last,
            format,
            all,
        } => cmd_get(&config, id_or_slug, last, format, all),
        Commands::Backup { status, daemon } => cmd_backup(&config, status, daemon),
        Commands::Restore {
            query,
            list,
            yes,
            force,
        } => cmd_restore(&config, query, list, yes, force),
    }
}

fn print_banner() {
    println!(
        r#"
╔═══════════════════════════════════════╗
║   chist — Claude Code session browser     ║
║   View Claude Code chat sessions      ║
╚═══════════════════════════════════════╝
"#
    );
}

fn cmd_exec(
    config: &Config,
    id_or_slug: Option<String>,
    last: bool,
    fork: bool,
    include_tmp: bool,
    execute: Option<&str>,
) {
    let reader = SessionReader::new(&config.claude_home);

    // `-` means "read the id from stdin", and so does a bare `-r` or `exec`
    // at the end of a pipe: `chist ls -i foo | chist -r`.
    let from_stdin = id_or_slug.as_deref() == Some("-")
        || (id_or_slug.is_none() && !last && !io::stdin().is_terminal());
    let id_or_slug = if from_stdin {
        let input = io::read_to_string(io::stdin()).unwrap_or_default();
        match first_session_id(&input) {
            Some(id) => Some(id),
            None => {
                eprintln!("No session in input");
                process::exit(1);
            }
        }
    } else {
        id_or_slug
    };

    // Find the session — try get_session (detail) first, fall back to scanning summaries
    let (session_id, project_path) = if last {
        if let Some(s) = reader.get_last_session(config.allowed_projects.as_deref(), include_tmp) {
            (s.session_id, s.project_path)
        } else {
            eprintln!("No sessions found.");
            process::exit(1);
        }
    } else if let Some(ref id) = id_or_slug {
        match locate(&reader, config, id, include_tmp) {
            Some(found) => found,
            None => {
                // Not on disk any more — Claude may have reaped it. Look in the
                // backups before giving up.
                let matches = search_backups(config, id);
                if restore_from_archive(config, id, &matches).is_some()
                    && let Some(found) = locate(&reader, config, id, include_tmp)
                {
                    found
                } else {
                    eprintln!("Session not found: {}", id);
                    suggest_local(config, &reader, id, Some(&matches));
                    process::exit(1);
                }
            }
        }
    } else {
        eprintln!("Error: Must specify session ID/slug or use --last");
        process::exit(1);
    };

    let resume_dir = project_path;

    // Output shell commands to stdout for eval
    let out = io::stdout();
    let mut out = out.lock();
    if let Some(prompt) = execute {
        let _ = writeln!(
            out,
            "cd {} && claude {} {} -p {}",
            shell_escape(&resume_dir),
            if fork { "-rf" } else { "-r" },
            shell_escape(&session_id),
            shell_escape(prompt),
        );
    } else {
        let _ = writeln!(
            out,
            "cd {} && claude {} {}",
            shell_escape(&resume_dir),
            if fork { "-rf" } else { "-r" },
            shell_escape(&session_id),
        );
    }
}

/// Resolve an id/slug to (session_id, project_path) using the detail lookup
/// first and a scan of session summaries second.
fn locate(
    reader: &SessionReader,
    config: &Config,
    id: &str,
    include_tmp: bool,
) -> Option<(String, String)> {
    if let Some(s) = reader.get_session(id, config.allowed_projects.as_deref(), include_tmp) {
        return Some((s.session_id, s.project_path));
    }
    let sessions =
        reader.list_sessions(None, None, config.allowed_projects.as_deref(), include_tmp);
    sessions
        .iter()
        .find(|s| s.session_id.starts_with(id) || s.slug.as_deref() == Some(id))
        .map(|s| (s.session_id.clone(), s.project_path.clone()))
}

/// Pull the first session ID out of piped `chist ls` output.
/// The ID is the first column, so take the first line whose leading token is
/// UUID-shaped — that skips the header, "No sessions found.", and the wrapped
/// continuation rows of the Last Msg column.
fn first_session_id(input: &str) -> Option<String> {
    input
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .find(|token| token.len() >= 8 && token.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
        .map(str::to_string)
}

fn shell_escape(s: &str) -> String {
    // If the string is safe, return as-is; otherwise single-quote it
    if s.chars()
        .all(|c| c.is_alphanumeric() || c == '/' || c == '-' || c == '_' || c == '.')
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn cmd_list(
    config: &Config,
    limit: Option<usize>,
    project: Option<String>,
    search: Option<String>,
    use_regex: bool,
    format: Option<String>,
    include_tmp: bool,
) {
    let reader = SessionReader::new(&config.claude_home);
    let limit = limit.unwrap_or(config.default_list_limit);
    let output_format = format.as_deref().unwrap_or(&config.default_format);

    let sessions = if let Some(ref pattern) = search {
        reader.search_sessions(
            pattern,
            use_regex,
            true,
            Some(limit),
            config.allowed_projects.as_deref(),
            include_tmp,
        )
    } else {
        reader.list_sessions(
            Some(limit),
            project.as_deref(),
            config.allowed_projects.as_deref(),
            include_tmp,
        )
    };

    if sessions.is_empty() {
        println!("No sessions found.");
        // A search that finds nothing locally is exactly when a reaped session
        // is being looked for. Say what the backups hold, without restoring.
        if let Some(pattern) = search {
            report_archive_matches(config, &pattern);
        }
        return;
    }

    match output_format {
        "json" => println!("{}", format_list_json(&sessions)),
        _ => println!("{}", format_list_table(&sessions)),
    }
}

fn cmd_get(
    config: &Config,
    id_or_slug: Option<String>,
    last: bool,
    format: Option<String>,
    include_tmp: bool,
) {
    let reader = SessionReader::new(&config.claude_home);

    let mut session = if last {
        reader.get_last_session(config.allowed_projects.as_deref(), include_tmp)
    } else if let Some(ref id) = id_or_slug {
        reader.get_session(id, config.allowed_projects.as_deref(), include_tmp)
    } else {
        eprintln!("Error: Must specify session ID/slug or use --last");
        process::exit(1);
    };

    let mut matches = None;
    if session.is_none()
        && let Some(ref id) = id_or_slug
    {
        let found = search_backups(config, id);
        if restore_from_archive(config, id, &found).is_some() {
            session = reader.get_session(id, config.allowed_projects.as_deref(), include_tmp);
        }
        matches = Some(found);
    }

    let Some(session) = session else {
        if last {
            eprintln!("No sessions found.");
        } else {
            let id = id_or_slug.as_deref().unwrap_or("?");
            eprintln!("Session not found: {id}");
            suggest_local(config, &reader, id, matches.as_ref());
        }
        process::exit(1);
    };

    let output_format = format.as_deref().unwrap_or("text");
    match output_format {
        "json" => println!("{}", format_detail_json(&session)),
        "yaml" => println!("{}", format_detail_yaml(&session)),
        _ => println!("{}", format_detail_text(&session)),
    }
}

fn cmd_backup(config: &Config, status: bool, daemon: bool) {
    if status {
        print!("{}", backup::status(config));
        return;
    }

    if daemon {
        // Background run: due-check inside, no output, never a non-zero exit
        // that something could trip over.
        let _ = backup::run(config, false);
        return;
    }

    eprintln!("Backing up to {} …", config.backup.dir.display());
    match backup::run(config, true) {
        Ok(outcome) => {
            if let Some(reason) = outcome.skipped {
                println!("Skipped: {reason}");
                return;
            }
            println!(
                "✓ {} ({})",
                outcome.archive.display(),
                backup::human_bytes(outcome.bytes)
            );
            println!("✓ {}", outcome.index.display());
        }
        Err(e) => {
            eprintln!("Backup failed: {e}");
            process::exit(1);
        }
    }
}

fn cmd_restore(config: &Config, query: Option<String>, list_only: bool, yes: bool, force: bool) {
    let store = ArchiveStore::new(&config.backup);

    let Some(query) = query else {
        eprintln!("Error: Must specify a session ID, slug, or search text");
        process::exit(1);
    };

    eprintln!("Searching backups in {} …", store.dir().display());
    if !store.exists() {
        eprintln!(
            "No backup directory at {} — nothing to restore from.",
            store.dir().display()
        );
        process::exit(1);
    }

    let mut hits = store.find(&query, true);
    if hits.is_empty() {
        hits = store.find(&query, false);
    }

    if hits.is_empty() {
        println!("No archived session matches {query:?}.");
        return;
    }

    if list_only || hits.len() > 1 {
        println!("{}", describe(&hits));
        if hits.len() > 1 {
            println!("\nNarrow it down with a UUID prefix or slug to restore one.");
        }
        return;
    }

    let hit = &hits[0];

    // The archived copy is always older than the live one. Restoring over a
    // session that is still on disk throws away whatever has happened since,
    // so it takes an explicit --force and never happens by way of -y.
    if let Some(existing) = local_copy(&config.claude_home, &hit.entry.session_id)
        && !force
    {
        let local = std::fs::metadata(&existing).map(|m| m.len()).unwrap_or(0);
        eprintln!(
            "{} is still in ~/.claude ({}, {}).",
            hit.label(),
            backup::human_bytes(local),
            existing.display(),
        );
        eprintln!(
            "The archived copy is older{}. Nothing restored — pass --force to replace it \n\
             (the live copy is kept as <uuid>.jsonl.replaced-<timestamp>).",
            match hit.last_active() {
                Some(d) => format!(", last used {d}"),
                None => String::new(),
            }
        );
        process::exit(1);
    }

    if !yes && config.backup.restore != RestoreMode::Auto && !confirm_restore(hit) {
        println!("Left it in the archive.");
        return;
    }

    match store.restore(hit, &config.claude_home, force) {
        Ok(path) => {
            println!("✓ Restored {} → {}", hit.label(), path.display());
            println!(
                "  chist -r {}",
                hit.entry.slug.clone().unwrap_or_else(|| hit.short_id())
            );
        }
        Err(e) => {
            eprintln!("Restore failed: {e}");
            process::exit(1);
        }
    }
}

/// Print local sessions that plausibly answer a failed lookup. Someone who
/// types a directory name rather than a session name gets pointed at the
/// sessions in that directory, instead of a bare "not found".
fn suggest_local(
    config: &Config,
    reader: &SessionReader,
    query: &str,
    matches: Option<&ArchiveMatches>,
) {
    let needle = query.to_lowercase();
    let sessions = reader.list_sessions(None, None, config.allowed_projects.as_deref(), true);

    let local: Vec<_> = sessions
        .iter()
        .filter(|s| {
            s.project_path.to_lowercase().contains(&needle)
                || s.slug
                    .as_deref()
                    .is_some_and(|slug| slug.to_lowercase().contains(&needle))
        })
        .take(5)
        .collect();

    if !local.is_empty() {
        eprintln!("\nDid you mean one of these, already in ~/.claude?");
        for s in &local {
            eprintln!(
                "  {:<10}  {:<26}  {}  {}",
                &s.session_id[..8.min(s.session_id.len())],
                s.slug.clone().unwrap_or_else(|| "—".into()),
                s.last_activity.get(..10).unwrap_or("—"),
                s.project_path,
            );
        }
        eprintln!(
            "\n  chist -r {}",
            &local[0].session_id[..8.min(local[0].session_id.len())]
        );
    }

    // Archived sessions that merely mention the text: worth naming, not worth
    // a restore prompt.
    let mentioned: Vec<_> = matches
        .map(|m| m.mentioned.iter().take(5).collect())
        .unwrap_or_default();
    if !mentioned.is_empty() {
        eprintln!("\nArchived sessions mentioning {query:?}:");
        for h in &mentioned {
            eprintln!(
                "  {:<10}  {:<26}  {}  {}",
                h.short_id(),
                h.entry.slug.clone().unwrap_or_else(|| "—".into()),
                h.last_active().unwrap_or("—"),
                truncate(&h.entry.first_prompt, 40),
            );
        }
        eprintln!("\n  chist restore {}", mentioned[0].short_id());
    }
}

/// What the backups have to say about a query.
#[derive(Default)]
struct ArchiveMatches {
    /// The query is this session's slug or UUID prefix — it names the session.
    named: Vec<ArchivedSession>,
    /// The query merely appears in this session's text.
    mentioned: Vec<ArchivedSession>,
}

/// Search the backups without letting an unresponsive backup directory wedge
/// the caller. Returns `None` if the search did not answer in time — cloud-sync
/// mounts stall, and a missing session is not worth hanging a terminal over.
fn find_archived(store: &ArchiveStore, query: &str, budget: Duration) -> Option<ArchiveMatches> {
    let (tx, rx) = mpsc::channel();
    let store = store.clone();
    let query = query.to_string();
    std::thread::spawn(move || {
        // exists() touches the mount too, so it belongs inside the budget.
        let matches = if store.exists() {
            let (named, mentioned) = store.find_all(&query);
            ArchiveMatches { named, mentioned }
        } else {
            ArchiveMatches::default()
        };
        let _ = tx.send(matches);
    });

    rx.recv_timeout(budget).ok()
}

/// Look for `id` in the backups and, with the user's agreement, put it back.
/// Returns the restored session id. All chatter goes to stderr: `exec` writes a
/// shell command to stdout and callers eval it.
/// Ask the backups about a missing session, once, within a budget.
fn search_backups(config: &Config, id: &str) -> ArchiveMatches {
    if config.backup.restore == RestoreMode::Never {
        return ArchiveMatches::default();
    }
    let store = ArchiveStore::new(&config.backup);
    match find_archived(&store, id, Duration::from_secs(15)) {
        Some(matches) => matches,
        None => {
            eprintln!(
                "(backups at {} did not respond — not searched)",
                store.dir().display()
            );
            ArchiveMatches::default()
        }
    }
}

fn restore_from_archive(config: &Config, id: &str, matches: &ArchiveMatches) -> Option<String> {
    if config.backup.restore == RestoreMode::Never {
        return None;
    }
    let store = ArchiveStore::new(&config.backup);

    // Only a session the query actually names is worth interrupting for. A
    // session that merely mentions the text is a search result, not an intent,
    // and one still on disk cannot be restored at all — offering either would
    // contradict the "not in ~/.claude any more" it is offered under.
    let hit = matches
        .named
        .iter()
        .find(|h| local_copy(&config.claude_home, &h.entry.session_id).is_none())?;
    let _ = id;

    if config.backup.restore != RestoreMode::Auto && !confirm_restore(hit) {
        return None;
    }

    eprintln!("Extracting from {} …", file_name(&hit.tarball));
    match store.restore(hit, &config.claude_home, false) {
        Ok(path) => {
            eprintln!("✓ Restored {} → {}", hit.label(), path.display());
            Some(hit.entry.session_id.clone())
        }
        Err(e) => {
            eprintln!("Restore failed: {e}");
            None
        }
    }
}

fn report_archive_matches(config: &Config, pattern: &str) {
    if config.backup.restore == RestoreMode::Never {
        return;
    }
    let store = ArchiveStore::new(&config.backup);
    let Some(matches) = find_archived(&store, pattern, Duration::from_secs(10)) else {
        return;
    };
    let hits: Vec<ArchivedSession> = matches.named.into_iter().chain(matches.mentioned).collect();
    if hits.is_empty() {
        return;
    }
    println!(
        "\n{} in the backups at {}:\n{}",
        if hits.len() == 1 {
            "1 archived session matches".to_string()
        } else {
            format!("{} archived sessions match", hits.len())
        },
        store.dir().display(),
        describe(&hits),
    );
    println!("\nRestore one with: chist restore <slug|uuid>");
}

fn describe(hits: &[ArchivedSession]) -> String {
    hits.iter()
        .take(20)
        .map(|h| {
            format!(
                "  {:<10}  {:<24}  {:<12}  {}",
                h.short_id(),
                h.entry.slug.clone().unwrap_or_else(|| "—".into()),
                h.last_active().unwrap_or("—"),
                truncate(&h.entry.first_prompt, 48),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate(s: &str, max: usize) -> String {
    let clean = s.replace('\n', " ");
    if clean.chars().count() <= max {
        return clean;
    }
    format!("{}…", clean.chars().take(max - 1).collect::<String>())
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
}

/// Ask on the controlling terminal. Reads /dev/tty rather than stdin so the
/// prompt still works inside `$(chist -r …)` command substitution.
fn confirm_restore(hit: &ArchivedSession) -> bool {
    eprintln!(
        "Not in ~/.claude any more. Found {} in {} (backed up {}{}).",
        hit.label(),
        file_name(&hit.tarball),
        human_days((chrono::Local::now().date_naive() - hit.archive_date).num_days()),
        match hit.last_active() {
            Some(d) => format!(", last used {d}"),
            None => String::new(),
        },
    );
    if !hit.entry.first_prompt.is_empty() {
        eprintln!("  “{}”", truncate(&hit.entry.first_prompt, 68));
    }
    eprint!("Restore it? [Y/n] ");
    let _ = io::stderr().flush();

    let Ok(tty) = File::open("/dev/tty") else {
        eprintln!(
            "\n(no terminal to ask on — run `chist restore {}`)",
            hit.short_id()
        );
        return false;
    };
    let mut answer = String::new();
    if BufReader::new(tty).read_line(&mut answer).is_err() {
        return false;
    }
    matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_escape_quotes_only_when_needed() {
        assert_eq!(shell_escape("/home/alice/tmp"), "/home/alice/tmp");
        assert_eq!(shell_escape("a b"), "'a b'");
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
    }

    #[test]
    fn truncate_counts_characters_not_bytes() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("a\nb", 10), "a b");
        assert_eq!(truncate("ünïcödé is fine here", 6), "ünïcö…");
    }
}

#[cfg(test)]
mod stdin_tests {
    use super::first_session_id;

    const TABLE: &str = " ID        Alias      Project                                        Branch  Status   Msgs  Last Msg\n\
 27810e3c             ...fior/fiordc.aigateway.fior.group            main    Running  4683  <task-notification>\n\
                                                                                            <task-id>bfkt7i83...\n\
 aef33748  hardening  ...fior/fiordc.aigateway.fior.group            main             4706  <command-name>/rename\n";

    #[test]
    fn takes_the_first_row_of_a_table() {
        assert_eq!(first_session_id(TABLE).as_deref(), Some("27810e3c"));
    }

    #[test]
    fn accepts_a_bare_id() {
        assert_eq!(
            first_session_id("aef33748-2512-44e6-b7e6-7adfd7720f34\n").as_deref(),
            Some("aef33748-2512-44e6-b7e6-7adfd7720f34")
        );
    }

    #[test]
    fn rejects_empty_and_no_results() {
        assert_eq!(first_session_id(""), None);
        assert_eq!(first_session_id("No sessions found.\n"), None);
        assert_eq!(first_session_id(" ID  Alias  Project\n"), None);
    }
}

#[cfg(test)]
mod cli_tests {
    use super::Cli;
    use clap::Parser;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).unwrap()
    }

    #[test]
    fn bare_resume_flag_reads_stdin() {
        assert_eq!(parse(&["chist", "-r"]).resume.as_deref(), Some("-"));
        assert_eq!(parse(&["chist", "--resume"]).resume.as_deref(), Some("-"));
    }

    #[test]
    fn explicit_dash_still_reads_stdin() {
        assert_eq!(parse(&["chist", "-r", "-"]).resume.as_deref(), Some("-"));
    }

    #[test]
    fn resume_with_a_value_is_unchanged() {
        assert_eq!(
            parse(&["chist", "-r", "3f4b4b02"]).resume.as_deref(),
            Some("3f4b4b02")
        );
    }

    #[test]
    fn bare_resume_composes_with_execute() {
        let cli = parse(&["chist", "-r", "-e", "git status"]);
        assert_eq!(cli.resume.as_deref(), Some("-"));
        assert_eq!(cli.execute.as_deref(), Some("git status"));
    }

    #[test]
    fn bare_exec_subcommand_takes_no_id() {
        let cli = parse(&["chist", "exec"]);
        assert!(matches!(
            cli.command,
            Some(super::Commands::Exec {
                id_or_slug: None,
                ..
            })
        ));
    }
}
