//! Whether a shell command plainly sends a file to a remote host, read from the command's own words as written: the
//! narrow shell side of the sink rule (GATE-205, GATE-215; the owner's decision of 2026-11-14). Shapes only, never a
//! judgment of intent (rule 4): a network sender's argument that names a file to send, and where it goes.
//!
//! Recognised (each after `env`/`sudo`/`nohup`… wrappers and `NAME=value` words, and inside `sh -c`/`bash -c`/`eval`):
//! - curl: `-F name=@path` / `name=<path` (and `--form`), `-T path` / `--upload-file path`, `-d @path` and every
//!   `--data*` / `--json` with `@path`, `--data-urlencode [name]@path`; the host is the command's URL (or `--url`).
//! - wget: `--post-file path`, `--body-file path`.
//! - httpie (`http`, `https`, `xh`, `xhs`): an item `field@path`, `field=@path` or `field:=@path`.
//! - scp and rsync: local paths copied to a `[user@]host:path` (rsync also `host::module`, `rsync://host/…`) last.
//! - sftp: `put`/`reput`/`mput` lines given on its input (a here-document, a here-string or `echo … |`).
//! - nc/ncat/netcat, ssh, socat, curl `-T -`/`@-`, httpie: a file given on the input (`< path`, or `cat path |`,
//!   `tar c… path |`), sent to the command's host; socat also `OPEN:path`/`FILE:path` to `TCP:host:port`.
//!
//! Not recognised (documented limits): an upload written inside a script file or an interpreter one-liner (python,
//! node, ruby, perl), a here-document fed to an interpreter, cloud and service CLIs (aws, gcloud, gh, rclone, git
//! push), a host held in a shell variable, and ssh's remote command. The proxy still records every host they reach.

use crate::{py, tls};

/// One send the command's words show: the program and switch as written, the files it names, the hosts it reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSend {
    pub how: String,
    pub paths: Vec<String>,
    pub hosts: Vec<String>,
}

const CURL_FILE_LONG: &[&str] = &[
    "--data",
    "--data-binary",
    "--data-ascii",
    "--data-urlencode",
    "--json",
    "--form",
    "--upload-file",
];
/// curl's long options that take a value (the value is never the URL).
const CURL_LONG_VALUED: &[&str] = &[
    "--data-raw",
    "--form-string",
    "--header",
    "--user-agent",
    "--referer",
    "--user",
    "--cookie",
    "--cookie-jar",
    "--output",
    "--request",
    "--config",
    "--cert",
    "--key",
    "--cacert",
    "--capath",
    "--proxy",
    "--proxy-user",
    "--resolve",
    "--connect-to",
    "--max-time",
    "--connect-timeout",
    "--retry",
    "--retry-delay",
    "--retry-max-time",
    "--write-out",
    "--interface",
    "--limit-rate",
    "--max-filesize",
    "--range",
    "--continue-at",
    "--dump-header",
    "--trace",
    "--trace-ascii",
    "--stderr",
    "--oauth2-bearer",
    "--aws-sigv4",
    "--unix-socket",
    "--abstract-unix-socket",
    "--mail-from",
    "--mail-rcpt",
    "--mail-auth",
    "--quote",
    "--local-port",
    "--dns-servers",
    "--doh-url",
    "--variable",
    "--keepalive-time",
    "--speed-limit",
    "--speed-time",
    "--time-cond",
    "--max-redirs",
    "--output-dir",
    "--netrc-file",
    "--preproxy",
    "--noproxy",
    "--socks5",
    "--socks5-hostname",
    "--socks4",
    "--socks4a",
    "--url-query",
];
const HTTPIE: &[&str] = &["http", "https", "xh", "xhs"];
const NETCAT: &[&str] = &["nc", "ncat", "netcat"];
const NC_VALUED: &str = "IiOPpsTVwXxqe";
const SCP_VALUED: &str = "cFiJloPSXD";
const RSYNC_VALUED: &str = "eBfTM";
const SFTP_VALUED: &str = "BbcDFiJloPRSs";

/// One simple command: its words, what its input is, and the operator after it.
#[derive(Default, Debug)]
struct Segment {
    args: Vec<String>,
    /// `< path`
    input: Option<String>,
    /// a here-string or here-document's text
    text: Option<String>,
    op: String,
}

fn segments(text: &str, mut bodies: std::vec::IntoIter<(String, String)>) -> Vec<Segment> {
    let mut out = vec![Segment::default()];
    let mut redirect: Option<String> = None;
    for w in tls::words(text) {
        if tls::OPERATORS.contains(&w.as_str()) {
            if let Some(last) = out.last_mut() {
                last.op = w;
            }
            out.push(Segment::default());
            redirect = None;
            continue;
        }
        if !w.is_empty() && w.chars().all(|c| c == '<' || c == '>') {
            if w == "<<" || w == "<<-" {
                if let Some(last) = out.last_mut() {
                    last.text = bodies.next().map(|(_, b)| b);
                }
                redirect = Some("<<".into());
            } else {
                redirect = Some(w);
            }
            continue;
        }
        let last = out.last_mut().expect("one segment at least");
        match redirect.take().as_deref() {
            Some("<") => last.input = Some(w),
            Some("<<<") => last.text = Some(w),
            Some(_) => {} // an output file, or a here-document's word
            None => last.args.push(w),
        }
    }
    out.into_iter()
        .filter(|s| !s.args.is_empty() || s.input.is_some())
        .collect()
}

/// The command's words after its `NAME=value` words and wrappers (`env`, `sudo`, `nohup`, `timeout 5`…).
fn program_args(cmd: &[String]) -> &[String] {
    let mut i = 0;
    while i < cmd.len() {
        let word = &cmd[i];
        if tls::assignment(word).is_some() && !word.starts_with('-') {
            i += 1;
            continue;
        }
        let wrapper = tls::base(word);
        if !tls::WRAPPERS.contains(&wrapper) {
            break;
        }
        i += 1;
        while i < cmd.len() && cmd[i].starts_with('-') && cmd[i] != "--" {
            if cmd[i].starts_with("--") {
                i += 1;
                continue;
            }
            let flags: Vec<char> = cmd[i].chars().skip(1).collect();
            let v = tls::valued(wrapper);
            let takes = flags.iter().position(|c| v.contains(*c)) == Some(flags.len() - 1);
            i += if takes { 2 } else { 1 };
        }
        if i < cmd.len() && cmd[i] == "--" {
            i += 1;
        }
        while i < cmd.len() && tls::assignment(&cmd[i]).is_some() {
            i += 1;
        }
        let remaining = cmd.len().saturating_sub(i + 1);
        i += tls::leading(wrapper).min(remaining);
    }
    &cmd[i.min(cmd.len())..]
}

/// A loopback host, or one written as a shell variable FERB can't read (`$H`): not a destination it can name.
fn local_host(h: &str) -> bool {
    h.contains(['$', '{', '}', '`'])
        || h == "localhost"
        || h.ends_with(".localhost")
        || h.parse::<std::net::IpAddr>()
            .is_ok_and(|a| a.is_loopback() || a.is_unspecified())
}

/// A URL's host, or a bare `host.tld/path` when `bare` (a dot or a scheme is needed: an option's value isn't a host).
fn url_host(word: &str, bare: bool) -> Option<String> {
    if word.starts_with('-') || word.starts_with('/') || word.starts_with('@') {
        return None;
    }
    let schemed = word.contains("://");
    if !schemed && !(bare && word.split('/').next().is_some_and(|h| h.contains('.'))) {
        return None;
    }
    tls::host(word).filter(|h| !local_host(h))
}

/// A file a sender reads: not its standard input, not nothing.
fn file(p: &str) -> Option<String> {
    let p = py::strip(p);
    (!p.is_empty() && p != "-" && p != "." && p != "/dev/stdin" && p != "/dev/null")
        .then(|| p.to_string())
}

/// The value after a short option cluster (`-F x`, `-Fx`, `-sSF x`): (option char, value, words used).
fn short_value(args: &[String], i: usize, valued: &str) -> Option<(char, Option<String>, usize)> {
    let a = &args[i];
    let flags: Vec<char> = a.chars().skip(1).collect();
    let at = flags.iter().position(|c| valued.contains(*c))?;
    let rest: String = flags[at + 1..].iter().collect();
    if !rest.is_empty() {
        Some((flags[at], Some(rest), 1))
    } else {
        Some((flags[at], args.get(i + 1).cloned(), 2))
    }
}

/// `-F name=@path;type=…` / `name=<path` → path.
fn form_file(value: &str) -> Option<String> {
    let (_, eq, rest) = py::partition(value, "=");
    if !eq {
        return None;
    }
    let rest = rest.strip_prefix('@').or_else(|| rest.strip_prefix('<'))?;
    let rest = rest.trim_matches('"');
    file(rest.split(';').next().unwrap_or(""))
}

/// `-d @path`, `--data-urlencode name@path` → path.
fn data_file(option: &str, value: &str) -> Option<String> {
    if option == "--data-urlencode" {
        let (before, at, after) = py::partition(value, "@");
        return (at && !before.contains('=')).then(|| file(after)).flatten();
    }
    value.strip_prefix('@').and_then(file)
}

fn curl(args: &[String], stdin: Option<&str>) -> Option<FileSend> {
    let mut paths = Vec::new();
    let mut how = Vec::new();
    let mut urls = Vec::new();
    let mut reads_stdin = false;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--" {
            urls.extend(args[i + 1..].iter().filter_map(|w| url_host(w, true)));
            break;
        }
        if let Some(long) = a.strip_prefix("--").map(|_| a.as_str()) {
            let (name, eq, inline) = py::partition(long, "=");
            let value = if eq {
                Some(inline.to_string())
            } else {
                args.get(i + 1).cloned()
            };
            let used = if eq { 1 } else { 2 };
            if CURL_FILE_LONG.contains(&name) {
                let v = value.unwrap_or_default();
                let found = match name {
                    "--form" => form_file(&v),
                    "--upload-file" => file(&v),
                    _ => data_file(name, &v),
                };
                reads_stdin |= v == "-" || v.ends_with("@-") || v.ends_with("=<-");
                if let Some(p) = found {
                    paths.push(p);
                    how.push(format!("curl {name}"));
                }
                i += used;
            } else if name == "--url" {
                urls.extend(value.and_then(|v| url_host(&v, true)));
                i += used;
            } else if CURL_LONG_VALUED.contains(&name) {
                i += used;
            } else {
                i += 1;
            }
            continue;
        }
        if a.starts_with('-') && a.len() > 1 {
            match short_value(args, i, tls::CURL_VALUED) {
                Some((c, value, used)) => {
                    let v = value.unwrap_or_default();
                    let found = match c {
                        'F' => form_file(&v),
                        'T' => file(&v),
                        'd' => data_file("-d", &v),
                        _ => None,
                    };
                    if matches!(c, 'F' | 'T' | 'd') {
                        reads_stdin |= v == "-" || v.ends_with("@-") || v.ends_with("=<-");
                    }
                    if let Some(p) = found {
                        paths.push(p);
                        how.push(format!("curl -{c}"));
                    }
                    i += used;
                }
                None => i += 1,
            }
            continue;
        }
        urls.extend(url_host(a, true));
        i += 1;
    }
    if reads_stdin {
        if let Some(p) = stdin.and_then(file) {
            paths.push(p);
            how.push("curl reading a file on its input".into());
        }
    }
    sent(how, paths, urls)
}

fn wget(args: &[String]) -> Option<FileSend> {
    let mut paths = Vec::new();
    let mut how = Vec::new();
    let mut urls = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let (name, eq, inline) = py::partition(a, "=");
        if name == "--post-file" || name == "--body-file" {
            let v = if eq {
                Some(inline.to_string())
            } else {
                args.get(i + 1).cloned()
            };
            if let Some(p) = v.as_deref().and_then(file) {
                paths.push(p);
                how.push(format!("wget {name}"));
            }
            i += if eq { 1 } else { 2 };
            continue;
        }
        if !a.starts_with('-') {
            urls.extend(url_host(a, true));
        }
        i += 1;
    }
    sent(how, paths, urls)
}

fn httpie(program: &str, args: &[String], stdin: Option<&str>) -> Option<FileSend> {
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    let mut rest = positional.as_slice();
    if rest
        .first()
        .is_some_and(|m| !m.is_empty() && m.chars().all(|c| c.is_ascii_uppercase()))
    {
        rest = &rest[1..];
    }
    let (url, items) = rest.split_first()?;
    let host = if url.contains("://") {
        tls::host(url)
    } else {
        url_host(url, true).or_else(|| tls::host(url).filter(|h| h.contains('.')))
    }
    .filter(|h| !local_host(h));
    let mut paths = Vec::new();
    for item in items {
        // an item's separator is its first `:`, `=` or `@`; a file is `@`, `=@` or `:=@`
        let cut = item.find([':', '=', '@']);
        let Some(at) = cut else { continue };
        let tail = &item[at..];
        let p = tail
            .strip_prefix(":=@")
            .or_else(|| tail.strip_prefix("=@"))
            .or_else(|| tail.strip_prefix('@'));
        if let Some(p) = p.and_then(file) {
            paths.push(p);
        }
    }
    let mut how = vec![format!("{program} item@file"); paths.len()];
    if let Some(p) = stdin.and_then(file) {
        paths.push(p);
        how.push(format!("{program} reading a file on its input"));
    }
    sent(how, paths, host.into_iter().collect())
}

/// `[user@]host:path`, `host::module`, `rsync://host/…`, `scp://host/…` → host.
fn remote(word: &str) -> Option<String> {
    if word.contains("://") {
        return tls::host(word).filter(|h| !local_host(h));
    }
    let (before, colon, _) = py::partition(word, ":");
    if !colon || before.is_empty() || before.contains('/') {
        return None;
    }
    let h = py::rpartition(before, "@").2;
    let h = h.trim_start_matches('[').trim_end_matches(']');
    (!h.is_empty() && !local_host(&py::lower(h))).then(|| py::lower(h))
}

fn positional<'a>(args: &'a [String], valued: &str) -> Vec<&'a String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--" {
            out.extend(args[i + 1..].iter());
            break;
        }
        if a.starts_with("--") {
            i += 1;
            continue;
        }
        if a.starts_with('-') && a.len() > 1 {
            i += match short_value(args, i, valued) {
                Some((_, _, used)) => used,
                None => 1,
            };
            continue;
        }
        out.push(a);
        i += 1;
    }
    out
}

fn copy_to_remote(program: &str, args: &[String], valued: &str) -> Option<FileSend> {
    let words = positional(args, valued);
    let (dest, sources) = words.split_last()?;
    let host = remote(dest)?;
    let paths: Vec<String> = sources
        .iter()
        .filter(|s| remote(s).is_none())
        .filter_map(|s| file(s))
        .collect();
    sent(vec![program.to_string(); paths.len()], paths, vec![host])
}

/// sftp: the `put` lines on its input.
fn sftp(args: &[String], text: Option<&str>) -> Option<FileSend> {
    let host = positional(args, SFTP_VALUED)
        .first()
        .and_then(|w| remote(&format!("{w}:")))?;
    let mut paths = Vec::new();
    for line in text.unwrap_or("").lines() {
        let words = tls::words(line);
        let Some((first, rest)) = words.split_first() else {
            continue;
        };
        if ["put", "reput", "mput"].contains(&py::lower(first).as_str()) {
            let files: Vec<&String> = rest.iter().filter(|w| !w.starts_with('-')).collect();
            let take = if py::lower(first) == "mput" {
                files.len()
            } else {
                files.len().min(1)
            };
            paths.extend(files.into_iter().take(take).filter_map(|f| file(f)));
        }
    }
    sent(vec!["sftp put".to_string(); paths.len()], paths, vec![host])
}

/// nc/ncat/netcat or ssh sending a file on their input to their host.
fn on_input(program: &str, args: &[String], valued: &str, stdin: Option<&str>) -> Option<FileSend> {
    let p = stdin.and_then(file)?;
    let host = positional(args, valued)
        .into_iter()
        .find(|w| !w.chars().all(|c| c.is_ascii_digit()))
        .and_then(|w| remote(&format!("{w}:")))?;
    sent(
        vec![format!("{program} reading a file on its input")],
        vec![p],
        vec![host],
    )
}

fn socat(args: &[String], stdin: Option<&str>) -> Option<FileSend> {
    let mut paths = Vec::new();
    let mut hosts = Vec::new();
    let mut reads_stdin = false;
    for a in args
        .iter()
        .filter(|a| !a.starts_with('-') || a.as_str() == "-")
    {
        let (kind, _, rest) = py::partition(a, ":");
        let kind = py::upper(kind);
        if a == "-" || kind == "STDIN" || kind == "STDIO" {
            reads_stdin = true;
        } else if ["OPEN", "FILE", "GOPEN", "OPEN-READ"].contains(&kind.as_str()) {
            paths.extend(file(rest.split(',').next().unwrap_or("")));
        } else if [
            "TCP",
            "TCP4",
            "TCP6",
            "TCP-CONNECT",
            "OPENSSL",
            "SSL",
            "UDP",
            "UDP4",
            "UDP6",
        ]
        .contains(&kind.as_str())
        {
            hosts.extend(remote(&format!(
                "{}:",
                rest.split(':').next().unwrap_or("")
            )));
        }
    }
    if reads_stdin {
        paths.extend(stdin.and_then(file));
    }
    sent(vec!["socat".to_string(); paths.len()], paths, hosts)
}

fn sent(how: Vec<String>, paths: Vec<String>, hosts: Vec<String>) -> Option<FileSend> {
    (!paths.is_empty() && !hosts.is_empty()).then(|| FileSend {
        how: tls::dedup(how).join(", "),
        paths: tls::dedup(paths),
        hosts: tls::dedup(hosts),
    })
}

/// The files a piped `cat path…` or `tar c… path…` before a sender puts on its input.
fn piped_file(prev: &Segment) -> Option<String> {
    let args = program_args(&prev.args);
    let (first, rest) = args.split_first()?;
    match tls::base(first) {
        "cat" => rest
            .iter()
            .find(|a| !a.starts_with('-'))
            .and_then(|a| file(a)),
        "tar" | "bsdtar" | "gtar" => {
            let creates = rest.first().is_some_and(|f| {
                let f = f.trim_start_matches('-');
                f.starts_with('c') || f.contains('c') && !f.contains('x') && !f.contains('t')
            });
            if !creates {
                return None;
            }
            positional(&rest[1..], "fCTX")
                .into_iter()
                .find_map(|a| file(a))
        }
        _ => None,
    }
}

/// `file_sends(command)`: each send of a file to a remote host the command's words show (see the module's list).
pub fn file_sends(command: &str) -> Vec<FileSend> {
    sends_at(command, 0)
}

fn sends_at(command: &str, depth: usize) -> Vec<FileSend> {
    let (text, bodies) = tls::bodies(command);
    let segs = segments(&text.replace('`', "\n"), bodies.into_iter());
    let mut out = Vec::new();
    for (k, seg) in segs.iter().enumerate() {
        let args = program_args(&seg.args);
        let Some(first) = args.first() else { continue };
        let program = tls::base(first);
        let rest = &args[1..];
        let piped_in = (k > 0 && segs[k - 1].op == "|")
            .then(|| piped_file(&segs[k - 1]))
            .flatten();
        let printed_in = (k > 0 && segs[k - 1].op == "|")
            .then(|| tls::printed(&segs[k - 1].args))
            .flatten();
        let stdin = seg.input.clone().or(piped_in);
        if depth < 3 {
            if tls::shells(program) {
                if let Some(at) = rest
                    .iter()
                    .position(|a| a.starts_with('-') && !a.starts_with("--") && a.contains('c'))
                {
                    if let Some(inner) = rest.get(at + 1) {
                        out.extend(sends_at(inner, depth + 1));
                    }
                }
                continue;
            }
            if program == "eval" {
                out.extend(sends_at(&rest.join(" "), depth + 1));
                continue;
            }
        }
        let found = match program {
            "curl" => curl(rest, stdin.as_deref()),
            "wget" => wget(rest),
            p if HTTPIE.contains(&p) => httpie(p, rest, stdin.as_deref()),
            "scp" => copy_to_remote("scp", rest, SCP_VALUED),
            "rsync" => copy_to_remote("rsync", rest, RSYNC_VALUED),
            "sftp" => sftp(rest, seg.text.as_deref().or(printed_in.as_deref())),
            p if NETCAT.contains(&p) => on_input(p, rest, NC_VALUED, stdin.as_deref()),
            "ssh" => on_input("ssh", rest, tls::SSH_VALUED, stdin.as_deref()),
            "socat" => socat(rest, stdin.as_deref()),
            _ => None,
        };
        out.extend(found);
    }
    out
}
