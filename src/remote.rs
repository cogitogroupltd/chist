//! `--host`: sessions on another machine's claude-runner, reached through
//! `clrn` (cog-claude-anywhere-rust/clrn). chist makes no HTTP calls itself.
//! For `-r`, it prints a clrn command, and the shell wrapper evals that just
//! as it evals the local `cd … && claude --resume`.

use std::collections::HashMap;

/// The port `claude-runner --serve` listens on unless told otherwise.
const RUNNER_PORT: u16 = 8282;

/// Resolve a `--host` value to a runner URL. The config's `hosts:` map wins.
/// Otherwise a URL is used as given, `name:port` gets a scheme, and a bare
/// name gets the default runner port. On a tailnet with MagicDNS, a machine's
/// name is enough.
pub fn url_for(hosts: &HashMap<String, String>, host: &str) -> String {
    let target = hosts.get(host).map(String::as_str).unwrap_or(host);
    if target.contains("://") {
        target.trim_end_matches('/').to_string()
    } else if target.contains(':') {
        format!("http://{target}")
    } else {
        format!("http://{target}:{RUNNER_PORT}")
    }
}

/// The command that opens `query` on the remote runner in clrn.
pub fn resume_command(url: &str, query: &str) -> String {
    format!("clrn --url {} -r {}", crate::shell_escape(url), crate::shell_escape(query))
}

/// A missing clrn would otherwise surface as `command not found` from the
/// shell wrapper's eval, which says nothing about where to get it.
pub fn require_clrn() {
    let found = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join("clrn").is_file()))
        .unwrap_or(false);
    if !found {
        eprintln!(
            "chist: --host needs clrn on PATH. Install it from cog-claude-anywhere-rust: \
             cargo install --path clrn"
        );
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_resolution() {
        let hosts = HashMap::from([("george".to_string(), "100.112.92.94".to_string())]);
        assert_eq!(url_for(&hosts, "george"), "http://100.112.92.94:8282");
        assert_eq!(url_for(&hosts, "cogito"), "http://cogito:8282");
        assert_eq!(url_for(&hosts, "box:9000"), "http://box:9000");
        assert_eq!(url_for(&hosts, "https://r.example.com/"), "https://r.example.com");
    }

    #[test]
    fn resume_command_quotes_what_needs_it() {
        assert_eq!(
            resume_command("http://cogito:8282", "aws migration"),
            "clrn --url 'http://cogito:8282' -r 'aws migration'"
        );
    }
}
