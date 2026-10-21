//! Whether a shell command may reach the network, so approvals can gate it as
//! [`SideEffectClass::External`] rather than a local mutation.
//!
//! A heuristic over the command text, not a parser or a sandbox. The command
//! is cut at every shell separator (`;`, `&`, `|`, newline, parentheses,
//! braces, backticks) with quotes dropped and line continuations joined, so
//! `sh -c '...'`, `$(...)`, pipelines and `&&` chains are read as their pieces,
//! and `{curl,-s,URL}` as its words. It is read a second time with quotes kept
//! as the shell reads them, so a quoted value or target stays one word, and
//! either reading counts. Redirections after the command word are dropped
//! (`curl>out`, `ls 2>/dev/null`); after a redirection or assignment before it
//! (`2>/dev/null curl`, `FOO=1 curl`), any later word may be the command. A
//! piece is networked when:
//!
//! - its command word is a network client (`curl`, `ssh`, `gh`, ...);
//! - its command word is `git` or a package manager and a later word is one of
//!   its network subcommands (`git push`, `npm install`, ...), or one that
//!   installs by default is run bare (`yarn`, `bundle`);
//! - it is `rsync` with a `host:path` or `rsync://` argument;
//! - its command word is a wrapper that runs another command (`sudo`, `env`,
//!   `xargs`, `sh`, ...) and any later word would qualify on its own;
//! - it is `python -m <module>` and the module would qualify (`python -m pip install`); or
//! - it names `/dev/tcp/` or `/dev/udp/`.
//!
//! Misreads lean towards gating: a quoted separator can make an argument look
//! like a command, and a piece of over 512 words or a command of over 64 KiB
//! is gated unread. Not seen:
//! deliberate obfuscation (`cur?`, `$'\x63url'`), network use inside scripts,
//! interpreters (`python -c`), variables holding a command, or builds that
//! fetch dependencies (`cargo build`).

use serde_json::Value;

use crate::tool::SideEffectClass;

/// Programs whose ordinary use talks to another host.
const NETWORK_CLIENTS: &[&str] = &[
    "aria2c",
    "aws",
    "az",
    "curl",
    "dig",
    "ftp",
    "gcloud",
    "gh",
    "glab",
    "host",
    "http",
    "https",
    "kubectl",
    "lftp",
    "mosh",
    "nc",
    "ncat",
    "netcat",
    "nmap",
    "nslookup",
    "ping",
    "ping6",
    "rclone",
    "scp",
    "sftp",
    "socat",
    "ssh",
    "telnet",
    "traceroute",
    "wget",
    "whois",
    "xh",
];

/// Programs that run the command given in their arguments.
const WRAPPERS: &[&str] = &[
    "bash", "builtin", "command", "corepack", "dash", "doas", "env", "eval", "exec", "find",
    "fish", "ionice", "ksh", "nice", "nohup", "parallel", "sh", "stdbuf", "sudo", "time",
    "timeout", "watch", "xargs", "zsh",
];

/// Shell keywords that may precede a command word.
const KEYWORDS: &[&str] = &["!", "do", "elif", "else", "if", "then", "until", "while"];

/// `git` subcommands that contact a remote; `remote` and `submodule` do so on `update`.
const GIT_NETWORK: &[&str] = &["clone", "fetch", "ls-remote", "pull", "push"];

/// Install, update and publish subcommands of the common package managers.
const PACKAGE_MANAGERS: &[(&str, &[&str])] = &[
    ("apk", &["add", "fetch", "update", "upgrade"]),
    (
        "apt",
        &["download", "full-upgrade", "install", "update", "upgrade"],
    ),
    (
        "apt-get",
        &[
            "download",
            "dist-upgrade",
            "install",
            "source",
            "update",
            "upgrade",
        ],
    ),
    (
        "brew",
        &["fetch", "install", "reinstall", "tap", "update", "upgrade"],
    ),
    ("bun", &["add", "i", "install", "publish", "update", "x"]),
    ("bundle", &["add", "install", "update"]),
    ("bunx", &[]),
    (
        "cargo",
        &[
            "add", "fetch", "install", "login", "publish", "search", "update", "yank",
        ],
    ),
    ("composer", &["install", "require", "update"]),
    ("conda", &["create", "install", "update"]),
    ("dnf", &["download", "install", "update", "upgrade"]),
    ("docker", &["login", "pull", "push"]),
    ("gem", &["fetch", "install", "push", "update"]),
    ("go", &["download", "get", "install"]),
    (
        "npm",
        &[
            "add", "ci", "exec", "i", "install", "publish", "update", "upgrade", "x",
        ],
    ),
    ("npx", &[]),
    ("pacman", &["-S", "-Sy", "-Syu", "-Syyu", "-Su"]),
    ("pip", &["download", "install"]),
    ("pip3", &["download", "install"]),
    ("pipx", &["install", "run", "upgrade"]),
    (
        "pnpm",
        &["add", "dlx", "i", "install", "publish", "update", "upgrade"],
    ),
    ("podman", &["login", "pull", "push"]),
    ("poetry", &["add", "install", "publish", "update"]),
    ("uv", &["add", "install", "lock", "pip", "sync", "tool"]),
    ("uvx", &[]),
    ("yarn", &["add", "dlx", "install", "publish", "upgrade"]),
    ("yum", &["install", "update", "upgrade"]),
];

/// Package managers that install the project's dependencies when run with no subcommand.
const BARE_INSTALLERS: &[&str] = &["bundle", "yarn"];

/// Longer pieces are gated unread: it bounds the suffix scans, quadratic in a
/// piece's words, on model-written text.
const MAX_PIECE_WORDS: usize = 512;

/// Longer commands are gated unread, so the work per command stays bounded.
const MAX_COMMAND_BYTES: usize = 64 * 1024;

/// Stands in for a redirection operator (`>`, `2>&1`, `<<`, ...) once the
/// command is flattened; never typed in a real command.
const REDIRECT: char = '\u{1}';

/// Stands in for quoted whitespace or an operator inside a word once the
/// command is flattened with quotes kept; never typed in a real command.
const IN_WORD: char = '\u{2}';

/// The class to gate a call under: a mutating call whose `command` (or the
/// `chars` typed into a shell) may reach the network is external. Never lowers it.
pub fn gated_class(declared: SideEffectClass, input: &Value) -> SideEffectClass {
    if declared != SideEffectClass::Mutation {
        return declared;
    }
    let networked = ["command", "chars"]
        .iter()
        .filter_map(|field| input.get(*field).and_then(Value::as_str))
        .any(touches_network);
    let class = if networked {
        SideEffectClass::External
    } else {
        declared
    };
    assert!(class >= declared, "a gate class may only be raised");
    class
}

/// Whether any piece of `command` may reach the network (see the module docs).
pub fn touches_network(command: &str) -> bool {
    if command.len() > MAX_COMMAND_BYTES {
        return true;
    }
    if command.contains("/dev/tcp/") || command.contains("/dev/udp/") {
        return true;
    }
    // Quotes dropped sees commands quoted for `sh -c` or `"$(...)"`; quotes kept
    // keeps a quoted word whole, so it never poses as the command. Either counts.
    [false, true].into_iter().any(|keep_quotes| {
        flatten(command, keep_quotes).lines().any(|piece| {
            let words: Vec<&str> = piece.split_whitespace().collect();
            piece_touches_network(&words)
        })
    })
}

/// `command` with continuations joined, one piece per line and each
/// redirection operator a [`REDIRECT`] word. Quotes are dropped, or with
/// `keep_quotes` read as the shell does: quoted and escaped text stays in its word.
fn flatten(command: &str, keep_quotes: bool) -> String {
    // `$'...'` and `$"..."` are quoting; a backslash-newline joins two lines.
    let joined = command
        .replace("\\\n", "")
        .replace("$'", "'")
        .replace("$\"", "\"");
    let mut flattened = String::with_capacity(joined.len() + 8);
    let mut chars = joined
        .chars()
        .filter(|c| !matches!(*c, REDIRECT | IN_WORD))
        .peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        if keep_quotes {
            match (quote, c) {
                (None, '\'' | '"') => quote = Some(c),
                (Some(open), _) if c == open => quote = None,
                (None | Some('"'), '\\') => flattened.extend(chars.next().map(in_word)),
                (Some(_), _) => flattened.push(in_word(c)),
                (None, _) => {}
            }
            if quote.is_some() || matches!(c, '\'' | '"' | '\\') {
                continue;
            }
        } else if matches!(c, '\'' | '"' | '\\') {
            continue;
        }
        match c {
            ',' => flattened.push(' '),
            '<' | '>' => {
                // `>>`, `<<<`, `>&2`, `>|` and `<>` are one operator.
                while chars
                    .next_if(|n| matches!(n, '<' | '>' | '&' | '|'))
                    .is_some()
                {}
                flattened.extend([' ', REDIRECT, ' ']);
            }
            '&' if chars.next_if_eq(&'>').is_some() => {
                chars.next_if_eq(&'>');
                flattened.extend([' ', REDIRECT, ' ']);
            }
            ';' | '&' | '|' | '(' | ')' | '{' | '}' | '`' | '\n' => flattened.push('\n'),
            other => flattened.push(other),
        }
    }
    flattened
}

/// `c` as quoted text: whitespace and shell operators become [`IN_WORD`].
fn in_word(c: char) -> char {
    let special = c.is_whitespace() || ";&|(){}`<>,".contains(c);
    if special { IN_WORD } else { c }
}

fn is_redirect(word: &str) -> bool {
    word.len() == REDIRECT.len_utf8() && word.starts_with(REDIRECT)
}

/// `words` without redirections: each operator, its target and a leading
/// file descriptor (`2` in `2>/dev/null`) go, so they never pose as the command.
fn without_redirections<'a>(words: &[&'a str]) -> Vec<&'a str> {
    let is_operator = |word: &&str| is_redirect(word);
    let mut kept = Vec::with_capacity(words.len());
    let mut i = 0;
    while i < words.len() {
        let fd_before_operator = words[i].bytes().all(|b| b.is_ascii_digit())
            && words.get(i + 1).is_some_and(is_operator);
        if fd_before_operator {
            i += 1;
        } else if is_operator(&words[i]) {
            i += 2;
        } else {
            kept.push(words[i]);
            i += 1;
        }
    }
    kept
}

/// One separator-free piece: its command word, with its later words.
fn piece_touches_network(words: &[&str]) -> bool {
    if words.len() > MAX_PIECE_WORDS {
        return true;
    }
    let Some(start) = words.iter().position(|w| !is_prefix_word(w)) else {
        return false;
    };
    let leading_redirect = is_redirect(words[start])
        || words.get(start + 1).is_some_and(|w| is_redirect(w))
            && words[start].bytes().all(|b| b.is_ascii_digit());
    let after_assignment = words[..start].iter().any(|w| w.contains('='));
    if leading_redirect || after_assignment {
        // A dropped quote may have hidden where the target or value ends, so
        // any later word may be the command.
        let rest: Vec<&str> = words[start..]
            .iter()
            .copied()
            .filter(|w| !is_redirect(w))
            .collect();
        return (0..rest.len()).any(|i| runs_network_program(&rest[i..]));
    }
    let words = without_redirections(&words[start..]);
    assert!(!words.is_empty(), "the command word is no redirection");
    let program = program_name(words[0]);
    let rest = &words[1..];
    if WRAPPERS.contains(&program) {
        return (0..rest.len()).any(|i| runs_network_program(&rest[i..]));
    }
    runs_network_program(&words)
}

/// Whether `words`, read as a command and its arguments, reaches the network.
fn runs_network_program(words: &[&str]) -> bool {
    let Some((first, mut args)) = words.split_first() else {
        return false;
    };
    let mut program = program_name(first);
    // `python [options] -m MODULE args` (or `-mMODULE`) is read as `MODULE args`.
    while program.starts_with("python") {
        let Some(flag) = args.iter().position(|a| a.starts_with("-m")) else {
            return false;
        };
        let module = &args[flag]["-m".len()..];
        let rest = &args[flag + 1..];
        assert!(rest.len() < args.len(), "each step consumes the flag");
        (program, args) = match (module, rest.split_first()) {
            ("", Some((name, after))) => (program_name(name), after),
            ("", None) => return false,
            (module, _) => (module, rest),
        };
    }
    if NETWORK_CLIENTS.contains(&program) {
        return true;
    }
    if BARE_INSTALLERS.contains(&program) && !names_a_subcommand(args) {
        return true;
    }
    if program == "git" {
        let updates =
            args.contains(&"update") && (args.contains(&"remote") || args.contains(&"submodule"));
        return updates || args.iter().any(|a| GIT_NETWORK.contains(a));
    }
    if program == "rsync" {
        return args.iter().any(|a| a.contains(':') && !a.starts_with('-'));
    }
    match PACKAGE_MANAGERS.iter().find(|(name, _)| *name == program) {
        Some((_, [])) => true,
        Some((_, subcommands)) => args.iter().any(|a| subcommands.contains(a)),
        None => false,
    }
}

/// Whether some argument is a subcommand: not a flag, nor the value of a
/// preceding `--long` flag (`yarn --cwd web` still runs the default install).
fn names_a_subcommand(args: &[&str]) -> bool {
    args.iter().enumerate().any(|(i, arg)| {
        let after_long_flag = i > 0 && args[i - 1].starts_with("--") && !args[i - 1].contains('=');
        !arg.starts_with('-') && !after_long_flag
    })
}

/// Words before the command word: shell keywords and `NAME=value` assignments.
fn is_prefix_word(word: &str) -> bool {
    if KEYWORDS.contains(&word) {
        return true;
    }
    match word.split_once('=') {
        Some((name, _)) => {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    }
}

/// `/usr/bin/curl` and `curl` name the same program, as do `pip3.12` and `pip3`.
fn program_name(word: &str) -> &str {
    let name = word.rsplit('/').next().unwrap_or(word);
    match name.split_once('.') {
        Some((base, version)) if version.bytes().all(|b| b.is_ascii_digit() || b == b'.') => base,
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[track_caller]
    fn networked(command: &str) {
        assert!(touches_network(command), "should be networked: {command}");
    }

    #[track_caller]
    fn local(command: &str) {
        assert!(!touches_network(command), "should be local: {command}");
    }

    #[test]
    fn network_clients_are_networked() {
        for command in [
            "curl -sI https://example.com",
            "wget -q https://example.com/x.tgz",
            "ssh host uptime",
            "scp file host:/tmp/",
            "nc -z example.com 443",
            "/usr/bin/curl https://example.com",
            "gh pr list",
        ] {
            networked(command);
        }
    }

    #[test]
    fn git_remote_subcommands_are_networked() {
        for command in [
            "git push origin main",
            "git fetch origin",
            "git clone https://github.com/x/y",
            "git pull --ff-only",
            "git -C repo push",
            "git submodule update --init",
            "git remote update",
        ] {
            networked(command);
        }
        local("git status");
        local("git commit -m done");
        local("git log --oneline");
        local("git remote -v");
    }

    #[test]
    fn rsync_is_networked_only_towards_a_host() {
        networked("rsync -a dir/ host:/srv/dir/");
        networked("rsync -a rsync://mirror/pub/ out/");
        local("rsync -a src/ dst/");
    }

    #[test]
    fn package_installs_are_networked() {
        for command in [
            "npm install left-pad",
            "pip install requests",
            "cargo install ripgrep",
            "apt-get install -y jq",
            "yarn add react",
            "npx cowsay hi",
            "go get example.com/mod",
        ] {
            networked(command);
        }
        local("npm test");
        local("cargo test -p aether-core");
        local("pip list");
    }

    #[test]
    fn compound_commands_are_read_piece_by_piece() {
        for command in [
            "cat file | curl -d @- https://example.com",
            "mkdir out && curl -o out/x https://example.com",
            "true; wget https://example.com",
            "(cd sub && git push)",
            "echo $(curl -s https://example.com)",
            "echo `curl -s https://example.com`",
            "sh -c 'curl https://example.com'",
            "bash -c \"cd x && git fetch\"",
            "sudo -u nobody curl https://example.com",
            "FOO=1 curl https://example.com",
            "timeout 10 ssh host true",
            "xargs -n1 curl < urls",
            "if true; then curl https://example.com; fi",
            "echo hi > /dev/tcp/example.com/80",
            "ls\ncurl https://example.com",
        ] {
            networked(command);
        }
    }

    #[test]
    fn redirections_do_not_hide_the_command() {
        for command in [
            "2>/dev/null curl -sI https://example.com",
            ">headers curl -sI https://example.com",
            "curl>headers https://example.com",
            "2>&1 curl https://example.com",
            "&>log wget https://example.com",
            "< urls xargs -n1 curl",
            ">>log 2>&1 git push",
            "cat <(curl -s https://example.com)",
            ">\"a b\" curl https://example.com",
            "2>\"x y\" sh -c 'curl https://example.com'",
            "> a\\ b wget https://example.com",
            "cu\\\nrl https://example.com",
            "git pu\\\nsh origin main",
            "$'curl' https://example.com",
            "{curl,-sI,https://example.com}",
            "GIT_SSH_COMMAND=\"ssh -i /tmp/key\" git fetch origin",
            "FOO=\"a b\" curl -sI https://example.com",
            "FOO=\"x;y\" curl https://example.com",
            "FOO='a > b' wget https://example.com",
            "> a\\;b curl https://example.com",
            "echo \"$(FOO=\"a b\" curl -s https://example.com)\"",
        ] {
            networked(command);
        }
        for command in [
            "echo hi > curl",
            "cat < ssh.md",
            "ls 2>/dev/null >wget.log",
            "grep curl file 2>&1 | head",
            "echo $HOME $? $(pwd)",
            "cut -d, -f1 curl.csv",
            "RUST_LOG=debug cargo test",
            "echo 'a; b' \"c | d\"",
        ] {
            local(command);
        }
    }

    #[test]
    fn default_and_versioned_installers_are_networked() {
        for command in [
            "yarn",
            "yarn --frozen-lockfile",
            "bundle",
            "pip3.12 install requests",
            "/usr/bin/python3 -m pip install requests",
            "python -m pip download x",
            "python -mpip install requests",
            "yarn --cwd frontend",
            "npm exec --yes --package cowsay -- cowsay hello",
            "npm x cowsay",
            "bunx cowsay hi",
            "go mod download",
            "corepack pnpm install",
            "corepack yarn install",
        ] {
            networked(command);
        }
        local("yarn test");
        local("python3 -m pytest");
        local("python3 -mpytest");
        local("yarn --cwd frontend test");
        local("npm run build");
        local("python script.py");
    }

    #[test]
    fn oversized_pieces_are_gated_without_deep_recursion() {
        networked(&"python -m ".repeat(20_000));
        networked(&format!("sudo {}", "a ".repeat(100_000)));
        local(&"echo a; ".repeat(5_000));
        networked(&"echo a; ".repeat(20_000));
        let many_pieces = format!("sudo {}x;", "python -m ".repeat(250)).repeat(1200);
        let started = std::time::Instant::now();
        networked(&many_pieces);
        assert!(started.elapsed().as_secs() < 2, "{:?}", started.elapsed());
        // Just under both caps: every suffix of every piece is read.
        let piece = format!("sudo {}x;", "python -m git ".repeat(170));
        let near_cap = piece.repeat(MAX_COMMAND_BYTES / piece.len());
        assert!(near_cap.len() <= MAX_COMMAND_BYTES);
        let started = std::time::Instant::now();
        local(&near_cap);
        assert!(started.elapsed().as_secs() < 2, "{:?}", started.elapsed());
    }

    #[test]
    fn network_words_as_arguments_stay_local() {
        for command in [
            "grep curl file",
            "grep -rn 'git push' docs",
            "echo wget",
            "cat ssh.md",
            "rg --type rust curl",
            "mkdir out && echo done",
            "ls -la",
            "",
        ] {
            local(command);
        }
    }

    #[test]
    fn only_shell_mutations_are_raised() {
        let curl = json!({ "command": "curl https://example.com" });
        let grep = json!({ "command": "grep curl file" });
        assert_eq!(
            gated_class(SideEffectClass::Mutation, &curl),
            SideEffectClass::External
        );
        assert_eq!(
            gated_class(SideEffectClass::Mutation, &grep),
            SideEffectClass::Mutation
        );
        // A read-only tool that happens to carry a command keeps its class.
        assert_eq!(
            gated_class(SideEffectClass::ReadOnly, &curl),
            SideEffectClass::ReadOnly
        );
        assert_eq!(
            gated_class(SideEffectClass::External, &grep),
            SideEffectClass::External
        );
        assert_eq!(
            gated_class(SideEffectClass::Mutation, &json!({ "path": "x" })),
            SideEffectClass::Mutation
        );
        assert_eq!(
            gated_class(
                SideEffectClass::Mutation,
                &json!({ "process_id": "p", "chars": "git push\n" })
            ),
            SideEffectClass::External
        );
    }
}
