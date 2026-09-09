mod config;
mod formatters;
mod models;
mod path_utils;
mod session_reader;

use clap::{Parser, Subcommand};
use config::Config;
use formatters::{
    format_detail_json, format_detail_text, format_detail_yaml, format_list_json, format_list_table,
};
use session_reader::SessionReader;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process;

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
  chist -r my-alias -e 'git status'        # Run one prompt non-interactively"#
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
}

fn main() {
    let cli = Cli::parse();

    // -r <id> is a shorthand for `exec <id>`
    if let Some(ref id) = cli.resume {
        let config = Config::load(cli.config.as_deref());
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

    // `-` means "read the id from stdin", so `chist ls -i foo | chist -r -` works.
    let id_or_slug = if id_or_slug.as_deref() == Some("-") {
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
        if let Some(s) = reader.get_session(id, config.allowed_projects.as_deref(), include_tmp) {
            (s.session_id, s.project_path)
        } else {
            // Fall back: scan all session summaries for matching ID prefix or slug
            let sessions =
                reader.list_sessions(None, None, config.allowed_projects.as_deref(), include_tmp);
            let found = sessions.iter().find(|s| {
                s.session_id.starts_with(id.as_str()) || s.slug.as_deref() == Some(id.as_str())
            });
            if let Some(s) = found {
                (s.session_id.clone(), s.project_path.clone())
            } else {
                eprintln!("Session not found: {}", id);
                process::exit(1);
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

    let sessions = if let Some(pattern) = search {
        reader.search_sessions(
            &pattern,
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

    let session = if last {
        reader.get_last_session(config.allowed_projects.as_deref(), include_tmp)
    } else if let Some(ref id) = id_or_slug {
        reader.get_session(id, config.allowed_projects.as_deref(), include_tmp)
    } else {
        eprintln!("Error: Must specify session ID/slug or use --last");
        process::exit(1);
    };

    let Some(session) = session else {
        if last {
            eprintln!("No sessions found.");
        } else {
            eprintln!(
                "Session not found: {}",
                id_or_slug.as_deref().unwrap_or("?")
            );
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
