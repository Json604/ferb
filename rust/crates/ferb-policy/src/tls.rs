//! policy/tls.py: whether a shell command (or an AppleScript's `do shell script`) turns off TLS certificate checks,
//! read from the command's own words and environment assignments as written (L39). It only ever adds a question.

use crate::rx::once;
use crate::{py, pyurl, shlex};

const OPERATORS: &[&str] = &[";", "&&", "||", "|", "&", "\n", "(", ")", ";;", "|&"];
const WRAPPERS: &[&str] = &[
    "env",
    "sudo",
    "command",
    "exec",
    "time",
    "nohup",
    "nice",
    "caffeinate",
    "xargs",
    "builtin",
    "doas",
    "timeout",
    "gtimeout",
    "arch",
    "stdbuf",
    "watch",
    "chronic",
    "unbuffer",
];
const FALSE: &[&str] = &["false", "0", "no", "off"];
const NOTHING: &[&str] = &["", "/dev/null"];
const CURL_VALUED: &str = "AbcCdDeEFHKmoPQrtTuUwxXyYz";
const SSH_VALUED: &str = "BbcDEeFIiJLlmOoPpQRSWw";
const CURL_CONFIG: &str = "a config file FERB can't read";

fn valued(wrapper: &str) -> &'static str {
    match wrapper {
        "nice" => "n",
        "sudo" => "ugCDhprtUT",
        "doas" => "uC",
        "env" => "uSCP",
        "time" => "fo",
        "xargs" => "nILPsEdJR",
        "caffeinate" => "tw",
        "timeout" | "gtimeout" => "sk",
        "arch" => "ed",
        "stdbuf" => "ioe",
        "watch" => "nq",
        "exec" => "a",
        _ => "",
    }
}

fn leading(wrapper: &str) -> usize {
    match wrapper {
        "timeout" | "gtimeout" => 1,
        _ => 0,
    }
}

/// tls.py:61 `TlsOff`: each switch as written, and the sites it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsOff {
    pub how: Vec<String>,
    pub hosts: Vec<String>,
}

fn is_false(s: &str) -> bool {
    FALSE.contains(&py::lower(s).as_str())
}

fn base(word: &str) -> &str {
    py::rpartition(word, "/").2
}

fn shells(program: &str) -> bool {
    let rest = ["ba", "z", "da", "k", "fi"]
        .iter()
        .find_map(|p| program.strip_prefix(p).filter(|r| *r == "sh"))
        .is_some();
    program == "sh" || rest
}

/// `_ASSIGNMENT.fullmatch`: (name, value).
fn assignment(word: &str) -> Option<(&str, &str)> {
    let (name, value) = word.split_once('=')?;
    let mut cs = name.chars();
    let first = cs.next()?;
    if !(first.is_ascii_alphabetic() || first == '_')
        || !cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    Some((name, value))
}

fn ident_at(s: &str, i: usize) -> usize {
    let b = s.as_bytes();
    if i < b.len() && (b[i].is_ascii_alphabetic() || b[i] == b'_') {
        let mut j = i + 1;
        while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
            j += 1;
        }
        j
    } else {
        i
    }
}

/// `_HEREDOC.finditer(line)`: (start, word) of each `<<WORD`, `<<-WORD`, `<< 'WORD'`.
fn heredocs(line: &str) -> Vec<(usize, String)> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < b.len() {
        if !(b[i] == b'<' && b[i + 1] == b'<') {
            i += 1;
            continue;
        }
        let try_from = |dash: bool| -> Option<(usize, String)> {
            let mut j = i + 2;
            if dash {
                if b.get(j) != Some(&b'-') {
                    return None;
                }
                j += 1;
            }
            while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
                j += 1;
            }
            let quote = b.get(j).copied().filter(|q| *q == b'\'' || *q == b'"');
            for q in [quote, None] {
                let start = if q.is_some() { j + 1 } else { j };
                let end = ident_at(line, start);
                if end == start {
                    continue;
                }
                match q {
                    Some(q) if b.get(end) == Some(&q) => {
                        return Some((end + 1, line[start..end].to_string()))
                    }
                    Some(_) => continue,
                    None => return Some((end, line[start..end].to_string())),
                }
            }
            None
        };
        match try_from(true).or_else(|| try_from(false)) {
            Some((end, word)) => {
                out.push((i, word));
                i = end;
            }
            None => i += 1,
        }
    }
    out
}

fn bodies(command: &str) -> (String, Vec<(String, String)>) {
    let lines: Vec<&str> = command.split('\n').collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut found = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        kept.push(line);
        i += 1;
        for (start, word) in heredocs(line) {
            let dash = line[start..].starts_with("<<-");
            let mut body = Vec::new();
            while i < lines.len() && (if dash { py::strip(lines[i]) } else { lines[i] }) != word {
                body.push(lines[i]);
                i += 1;
            }
            i += 1;
            found.push((line.to_string(), body.join("\n")));
        }
    }
    (kept.join("\n"), found)
}

fn words(text: &str) -> Vec<String> {
    match shlex::split(text) {
        Ok(w) => w,
        Err(shlex::Unclosed) => once!(plain, r"&&|\|\||[;&|()\n]|[^{S};&|()]+")
            .texts(text)
            .into_iter()
            .map(|w| w.trim_matches(|c| c == '\'' || c == '"').to_string())
            .collect(),
    }
}

fn piped(words: Vec<String>) -> Vec<(Vec<String>, String)> {
    let mut out: Vec<(Vec<String>, String)> = vec![(Vec::new(), String::new())];
    for w in words {
        if OPERATORS.contains(&w.as_str()) {
            if let Some(last) = out.last_mut() {
                last.1 = w;
            }
            out.push((Vec::new(), String::new()));
        } else if !(!w.is_empty() && w.chars().all(|c| c == '<' || c == '>')) {
            if let Some(last) = out.last_mut() {
                last.0.push(w);
            }
        }
    }
    out.into_iter().filter(|(c, _)| !c.is_empty()).collect()
}

fn stripped(cmd: &[String]) -> &[String] {
    let mut i = 0;
    while i < cmd.len() && (assignment(&cmd[i]).is_some() || WRAPPERS.contains(&base(&cmd[i]))) {
        i += 1;
    }
    &cmd[i..]
}

fn printed(cmd: &[String]) -> Option<String> {
    let args = stripped(cmd);
    let first = args.first()?;
    let program = base(first);
    if program != "echo" && program != "printf" {
        return None;
    }
    let mut rest = &args[1..];
    while let Some(r) = rest.first() {
        if r.starts_with('-') && py::char_len(r) > 1 && args[0] != "printf" {
            rest = &rest[1..];
        } else {
            break;
        }
    }
    Some(rest.join(" ").replace("\\n", "\n").replace("\\t", "\t"))
}

fn reads_stdin_shell(cmd: &[String]) -> bool {
    let args = stripped(cmd);
    let Some(first) = args.first() else {
        return false;
    };
    if !shells(base(first)) {
        return false;
    }
    let rest = &args[1..];
    rest.iter()
        .all(|a| a.starts_with('-') && !a.trim_start_matches('-').contains('c'))
        || rest.iter().any(|a| a == "-s")
}

fn ssh_command(args: &[String]) -> Option<String> {
    let mut i = 0;
    while i < args.len() && args[i].starts_with('-') && args[i] != "--" {
        let flags: Vec<char> = args[i].chars().skip(1).collect();
        let valued = flags.iter().position(|c| SSH_VALUED.contains(*c));
        i += if valued.is_some_and(|v| v == flags.len().wrapping_sub(1)) {
            2
        } else {
            1
        };
    }
    if i < args.len() && args[i] == "--" {
        i += 1;
    }
    (i + 1 < args.len()).then(|| args[i + 1..].join(" "))
}

fn host(value: &str) -> Option<String> {
    let value = py::strip(value);
    if value.is_empty() {
        return None;
    }
    let url = if value.contains("://") {
        value.to_string()
    } else {
        format!("//{value}")
    };
    let parsed = pyurl::urlsplit(&url).ok()?;
    let h = py::lower(&parsed.hostname().unwrap_or_default());
    (!h.is_empty()).then_some(h)
}

/// `re.sub(r'\\(.)', r'\1', text)`.
fn unescape(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&n) = chars.peek() {
                if n != '\n' {
                    out.push(n);
                    chars.next();
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

fn dedup(items: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter(|x| seen.insert(x.clone()))
        .collect()
}

/// `applescript_checks_off`: the same facts for the shell commands an AppleScript runs.
pub fn applescript_checks_off(script: &str) -> Option<TlsOff> {
    let re = once!(
        fancy,
        r#"(?i:do)[{S}]+(?i:shell)[{S}]+(?i:scr)[iI\x{130}\x{131}](?i:pt)[{S}]+"((?:[^"\\]|\\.)*)""#
    );
    let found: Vec<Option<TlsOff>> = re
        .all(script)
        .into_iter()
        .filter_map(|g| g[1].map(|(a, b)| certificate_checks_off(&unescape(&script[a..b]))))
        .collect();
    let how = dedup(found.iter().flatten().flat_map(|f| f.how.clone()).collect());
    if how.is_empty() {
        return None;
    }
    Some(TlsOff {
        how,
        hosts: dedup(
            found
                .iter()
                .flatten()
                .flat_map(|f| f.hosts.clone())
                .collect(),
        ),
    })
}

/// `certificate_checks_off`: the command's switches that turn TLS certificate checks off, as written, and the sites
/// they concern; None when it has none.
pub fn certificate_checks_off(command: &str) -> Option<TlsOff> {
    checks_off(command, 0)
}

fn python_name(program: &str) -> bool {
    // python(\d+(\.\d+)?)?
    let Some(rest) = program.strip_prefix("python") else {
        return false;
    };
    if rest.is_empty() {
        return true;
    }
    let (a, b) = match rest.split_once('.') {
        Some((a, b)) => (a, Some(b)),
        None => (rest, None),
    };
    !a.is_empty()
        && a.chars().all(py::is_decimal)
        && b.is_none_or(|b| !b.is_empty() && b.chars().all(py::is_decimal))
}

fn pip_name(program: &str) -> bool {
    if program == "uv" || program == "pipx" {
        return true;
    }
    let Some(rest) = program.strip_prefix("pip") else {
        return false;
    };
    rest.is_empty() || python_name(&format!("python{rest}"))
}

fn checks_off(command: &str, depth: usize) -> Option<TlsOff> {
    let (text, bodies_found) = bodies(command);
    let mut how: Vec<String> = Vec::new();
    let mut hosts: Vec<String> = Vec::new();
    let piped = piped(words(&text.replace('`', "\n")));
    let mut every_env: Vec<(String, String)> = Vec::new();
    let mut nested: Vec<String> = Vec::new();
    for (k, (cmd, op)) in piped.iter().enumerate() {
        if op == "|" && k + 1 < piped.len() && reads_stdin_shell(&piped[k + 1].0) {
            if let Some(p) = printed(cmd) {
                nested.push(p);
            }
        }
    }
    for (cmd, _op) in &piped {
        let mut env: Vec<(String, String)> = Vec::new();
        let mut inner: Vec<String> = Vec::new();
        let mut i = 0;
        while i < cmd.len() {
            let word = &cmd[i];
            let b = base(word);
            if let Some((name, value)) = assignment(word).filter(|_| !word.starts_with('-')) {
                env.push((name.to_string(), value.to_string()));
                i += 1;
            } else if ["export", "declare", "local", "readonly"].contains(&word.as_str())
                || WRAPPERS.contains(&b)
            {
                let wrapper = b.to_string();
                i += 1;
                while i < cmd.len() && cmd[i].starts_with('-') && cmd[i] != "--" {
                    if wrapper == "arch" && cmd[i] == "-arch" {
                        i += 2;
                        continue;
                    }
                    if cmd[i].starts_with("--") {
                        i += 1;
                        continue;
                    }
                    let flags: Vec<char> = cmd[i].chars().skip(1).collect();
                    let v = valued(&wrapper);
                    let at = flags.iter().position(|c| v.contains(*c));
                    let takes = at.is_some_and(|a| a == flags.len() - 1) && i + 1 < cmd.len();
                    if let Some(a) = at {
                        if wrapper == "env" && flags[a] == 'S' {
                            let rest: String = flags[a + 1..].iter().collect();
                            inner.push(if !rest.is_empty() {
                                rest
                            } else {
                                cmd.get(i + 1).cloned().unwrap_or_default()
                            });
                        }
                    }
                    i += if takes { 2 } else { 1 };
                }
                if i < cmd.len() && cmd[i] == "--" {
                    i += 1;
                }
                let remaining = (cmd.len() as i64 - i as i64 - 1).max(0) as usize;
                i += leading(&wrapper).min(remaining);
                if wrapper == "watch" && i + 1 == cmd.len() && cmd[i].contains(' ') {
                    inner.push(cmd[i].clone());
                }
            } else {
                break;
            }
        }
        for (name, value) in &env {
            environment(name, value, &mut how, &mut hosts);
        }
        every_env.extend(env.iter().cloned());
        let args: &[String] = if i <= cmd.len() { &cmd[i..] } else { &[] };
        if let Some(first) = args.first() {
            let program = base(first);
            if shells(program) {
                if args.len() >= 2 {
                    for k in 1..args.len() - 1 {
                        let a = &args[k];
                        if a.starts_with('-') && !a.starts_with("--") && a.contains('c') {
                            inner.push(args[k + 1].clone());
                            break;
                        }
                    }
                }
            } else if program == "eval" {
                inner.push(args[1..].join(" "));
            } else if program == "ssh" {
                if let Some(remote) = ssh_command(&args[1..]).filter(|r| !r.is_empty()) {
                    inner.push(remote);
                }
            } else if program == "osascript" && args.len() >= 2 {
                let script: Vec<&str> = (1..args.len() - 1)
                    .filter(|&k| args[k] == "-e")
                    .map(|k| args[k + 1].as_str())
                    .collect();
                let script = script.join("\n");
                if !script.is_empty() {
                    if let Some(found) = applescript_checks_off(&script) {
                        how.extend(found.how);
                        hosts.extend(found.hosts);
                    }
                }
            }
        }
        if depth < 3 {
            for line in &inner {
                if let Some(found) = checks_off(line, depth + 1) {
                    how.extend(found.how);
                    hosts.extend(found.hosts);
                }
            }
        }
        let Some(first) = args.first() else {
            continue;
        };
        let program = base(first).to_string();
        let rest = &args[1..];
        anywhere(rest, &mut how, &mut hosts);
        if program == "curl" {
            curl(rest, &mut how);
        } else if program == "wget" {
            for a in rest {
                if a == "--no-check-certificate"
                    || (a.starts_with("--check-certificate=")
                        && is_false(a.split_once('=').map_or("", |x| x.1)))
                {
                    how.push(a.clone());
                }
            }
            for (k, a) in rest.iter().enumerate() {
                let value: Option<&str> = if a.starts_with("-e") && a.len() > 2 {
                    Some(&a[2..])
                } else if a.starts_with("--execute=") {
                    a.split_once('=').map(|x| x.1)
                } else if (a == "-e" || a == "--execute") && k + 1 < rest.len() {
                    Some(rest[k + 1].as_str())
                } else {
                    None
                };
                let compact = value.unwrap_or("").replace(' ', "");
                let (setting, _, flag) = py::partition(&compact, "=");
                if py::lower(setting) == "check_certificate" && is_false(flag) {
                    how.push(format!("-e {}", value.unwrap_or("None")));
                }
            }
        } else if (program == "http" || program == "https")
            && rest.iter().filter(|a| a.starts_with("--verify=")).any(|a| {
                is_false(a.split_once('=').map_or("", |x| x.1))
                    || py::lower(a.split_once('=').map_or("", |x| x.1)) == "no"
            })
        {
            how.extend(rest.iter().filter(|a| a.starts_with("--verify=")).cloned());
        } else if program == "aria2c" {
            how.extend(
                rest.iter()
                    .filter(|a| {
                        a.starts_with("--check-certificate=")
                            && is_false(a.split_once('=').map_or("", |x| x.1))
                    })
                    .cloned(),
            );
        } else if program == "conda"
            && rest.iter().any(|a| a == "ssl_verify")
            && rest.iter().any(|a| is_false(a))
        {
            how.push("config --set ssl_verify false".into());
        } else if program == "git" {
            git(rest, &mut how);
        } else if ["npm", "yarn", "pnpm", "npx"].contains(&program.as_str()) {
            npm(rest, &mut how);
        } else if python_name(&program) || program == "node" {
            let mut code = String::new();
            if program != "node"
                && rest.len() >= 2
                && rest[..rest.len() - 1].iter().any(|a| a == "-c")
            {
                let at = rest.iter().position(|a| a == "-c").unwrap_or(0);
                code = rest[at + 1].clone();
            } else if rest.len() >= 2 {
                if let Some(k) = (0..rest.len() - 1)
                    .find(|&k| ["-e", "--eval", "-p", "--print"].contains(&rest[k].as_str()))
                {
                    code = rest[k + 1].clone();
                }
            }
            if code.is_empty() && program == "node" {
                if let Some(a) = rest
                    .iter()
                    .find(|a| a.starts_with("--eval=") || a.starts_with("--print="))
                {
                    code = a.split_once('=').map_or(String::new(), |x| x.1.to_string());
                }
            }
            if !code.is_empty() {
                if let Some(m) = code_match(&code, program == "node") {
                    how.push(format!("{program} code: {m}"));
                }
            }
        }
        if pip_name(&program)
            || (python_name(&program) && rest.len() >= 2 && rest[0] == "-m" && rest[1] == "pip")
        {
            let sub = if rest.len() >= 2 && rest[0] == "-m" && rest[1] == "pip" {
                &rest[2..]
            } else {
                rest
            };
            pip_config(sub, &mut how, &mut hosts);
            for (k, a) in rest.iter().enumerate() {
                let cert = if let Some(c) = a.strip_prefix("--cert=") {
                    Some(c.to_string())
                } else if a == "--cert" && k + 1 < rest.len() {
                    Some(rest[k + 1].clone())
                } else {
                    None
                };
                if let Some(cert) = cert {
                    if NOTHING.contains(&py::strip(&cert)) {
                        how.push(format!(
                            "--cert {}",
                            if cert.is_empty() { "\"\"" } else { &cert }
                        ));
                    }
                }
            }
        }
        hosts.extend(
            rest.iter()
                .filter(|a| a.contains("://"))
                .filter_map(|a| host(a)),
        );
    }
    for (line, body) in &bodies_found {
        let feeds = once!(
            fancy,
            r"(?:^|[{S}/;&|(])(python(?:[{D}]+(?:\.[{D}]+)?)?|node)(?=[{S}<]|\z)"
        )
        .first(line);
        let fed = feeds.and_then(|g| g[1]).map(|(a, b)| &line[a..b]);
        if let Some(said) = fed.and_then(|f| code_match(body, f == "node")) {
            how.push(format!("{} code: {said}", fed.unwrap_or_default()));
        } else if once!(fancy, r"(?:^|[{S}/;&|(])(?:ba|z|da|k|fi)?sh(?=[{S}<]|\z)")
            .first(line)
            .is_some()
        {
            nested.push(body.clone());
        }
    }
    if depth < 3 {
        for line in &nested {
            if let Some(found) = checks_off(line, depth + 1) {
                how.extend(found.how);
                hosts.extend(found.hosts);
            }
        }
    }
    git_environment(&every_env, &mut how);
    if how.is_empty() {
        return None;
    }
    Some(TlsOff {
        how: dedup(how),
        hosts: dedup(hosts),
    })
}

fn code_match(code: &str, node: bool) -> Option<String> {
    let re = if node {
        once!(
            plain,
            r"rejectUnauthorized[{S}]*:[{S}]*false|NODE_TLS_REJECT_UNAUTHORIZED"
        )
    } else {
        once!(
            fancy,
            r"_create_unverified_context|{B}verify[{S}]*=[{S}]*False{B}|{B}CERT_NONE{B}|{B}check_hostname[{S}]*=[{S}]*False{B}"
        )
    };
    re.first(code)
        .and_then(|g| g[0])
        .map(|(a, b)| code[a..b].to_string())
}

fn split_hosts(value: &str) -> Vec<String> {
    // re.split(r"[\s,]+", value)
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_sep = false;
    for c in value.chars() {
        if py::is_space(c) || c == ',' {
            if !in_sep {
                out.push(std::mem::take(&mut cur));
                in_sep = true;
            }
        } else {
            in_sep = false;
            cur.push(c);
        }
    }
    out.push(cur);
    out.iter().filter_map(|v| host(v)).collect()
}

fn environment(name: &str, value: &str, how: &mut Vec<String>, hosts: &mut Vec<String>) {
    let upper = py::upper(name);
    let v = py::strip(value);
    if ["PIP_TRUSTED_HOST", "UV_INSECURE_HOST", "GOINSECURE"].contains(&upper.as_str())
        && !v.is_empty()
    {
        how.push(format!("{name}={value}"));
        hosts.extend(split_hosts(value));
    } else if upper == "GIT_SSL_NO_VERIFY"
        || (["NODE_TLS_REJECT_UNAUTHORIZED", "PYTHONHTTPSVERIFY"].contains(&upper.as_str())
            && v == "0")
        || (["NPM_CONFIG_STRICT_SSL", "CONDA_SSL_VERIFY"].contains(&upper.as_str()) && is_false(v))
    {
        how.push(format!("{name}={value}"));
    } else if [
        "REQUESTS_CA_BUNDLE",
        "SSL_CERT_FILE",
        "CURL_CA_BUNDLE",
        "PIP_CERT",
    ]
    .contains(&upper.as_str())
        && NOTHING.contains(&v)
    {
        how.push(format!(
            "{name}={}",
            if value.is_empty() { "\"\"" } else { value }
        ));
    }
}

fn git_key(key: &str) -> bool {
    // http(\..+)?\.sslverify, re.I, fullmatch
    let c: Vec<char> = key.chars().collect();
    let pre: Vec<char> = "http".chars().collect();
    let suf: Vec<char> = ".sslverify".chars().collect();
    if c.len() < pre.len() + suf.len() {
        return false;
    }
    let ci = |x: char, y: char| if y == '.' { x == '.' } else { py::ci_eq(x, y) };
    if !c[..4].iter().zip(&pre).all(|(x, y)| ci(*x, *y)) {
        return false;
    }
    if !c[c.len() - suf.len()..]
        .iter()
        .zip(&suf)
        .all(|(x, y)| ci(*x, *y))
    {
        return false;
    }
    let mid = &c[4..c.len() - suf.len()];
    mid.is_empty() || (mid[0] == '.' && mid.len() >= 2 && !mid[1..].contains(&'\n'))
}

fn git_environment(env: &[(String, String)], how: &mut Vec<String>) {
    let mut values: Vec<(String, String)> = Vec::new();
    for (name, value) in env {
        let up = py::upper(name);
        match values.iter_mut().find(|(n, _)| *n == up) {
            Some(slot) => slot.1 = value.clone(),
            None => values.push((up, value.clone())),
        }
    }
    let get = |k: &str| values.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    for (name, key) in &values {
        if let Some(n) = name.strip_prefix("GIT_CONFIG_KEY_") {
            if git_key(py::strip(key)) {
                let value = get(&format!("GIT_CONFIG_VALUE_{n}")).unwrap_or_default();
                if is_false(py::strip(&value)) {
                    how.push(format!("{name}={key} GIT_CONFIG_VALUE_{n}={value}"));
                }
            }
        }
    }
    let params = get("GIT_CONFIG_PARAMETERS").unwrap_or_default();
    let re = once!(
        fancy,
        r"((?i:http)(?:\.[^'{S}=]+)?(?i:\.sslver)[iI\x{130}\x{131}](?i:fy))[{S}]*=[{S}]*'?([{W}]+)"
    );
    for g in re.all(&params) {
        if let Some((a, b)) = g[2] {
            if is_false(&params[a..b]) {
                how.push(format!("GIT_CONFIG_PARAMETERS={params}"));
                break;
            }
        }
    }
}

fn anywhere(args: &[String], how: &mut Vec<String>, hosts: &mut Vec<String>) {
    for (k, a) in args.iter().enumerate() {
        let (flag, eq, value) = py::partition(a, "=");
        if flag == "--unsafely-ignore-certificate-errors" {
            how.push(a.clone());
            hosts.extend(value.split(',').filter_map(host));
            continue;
        }
        if flag == "--tls-verify" || flag == "--tlsverify" {
            let setting = if eq {
                value.to_string()
            } else {
                args.get(k + 1).cloned().unwrap_or_default()
            };
            if is_false(&setting) {
                how.push(format!("{flag}={setting}"));
            }
            continue;
        }
        if flag == "enableStrictSsl" || (a == "enableStrictSsl" && k + 1 < args.len()) {
            let setting = if eq {
                value.to_string()
            } else {
                args.get(k + 1).cloned().unwrap_or_default()
            };
            if is_false(&setting) {
                how.push(format!("enableStrictSsl {setting}"));
            }
            continue;
        }
        if flag == "--trusted-host" || flag == "--allow-insecure-host" {
            let value = if eq {
                value.to_string()
            } else {
                args.get(k + 1).cloned().unwrap_or_default()
            };
            how.push(py::strip(&format!("{flag} {value}")).to_string());
            if let Some(h) = host(&value) {
                hosts.push(h);
            }
        } else if (flag == "--insecure-skip-tls-verify" && (!eq || !is_false(value)))
            || a == "--no-verify-ssl"
        {
            how.push(a.clone());
        }
    }
}

fn curl(args: &[String], how: &mut Vec<String>) {
    let mut skip = false;
    for (k, a) in args.iter().enumerate() {
        if skip {
            skip = false;
            continue;
        }
        if ["--insecure", "--proxy-insecure", "--doh-insecure"].contains(&a.as_str()) {
            how.push(a.clone());
        } else if a == "--config" || a.starts_with("--config=") {
            how.push(if a.contains('=') {
                format!("{a} ({CURL_CONFIG})")
            } else {
                format!(
                    "--config {} ({CURL_CONFIG})",
                    args.get(k + 1).map_or("", String::as_str)
                )
            });
            skip = !a.contains('=');
        } else if a.starts_with('-') && !a.starts_with("--") && py::char_len(a) > 1 {
            let chars: Vec<char> = a.chars().collect();
            for (j, ch) in chars[1..].iter().enumerate() {
                if *ch == 'k' {
                    how.push(a.clone());
                    break;
                }
                if *ch == 'K' {
                    let named: String = chars[j + 2..].iter().collect();
                    let named = if named.is_empty() {
                        args.get(k + 1).cloned().unwrap_or_default()
                    } else {
                        named
                    };
                    how.push(format!("-K {named} ({CURL_CONFIG})"));
                    skip = j == chars.len() - 2;
                    break;
                }
                if CURL_VALUED.contains(*ch) {
                    skip = j == chars.len() - 2;
                    break;
                }
            }
        }
    }
}

fn git(args: &[String], how: &mut Vec<String>) {
    for (k, a) in args.iter().enumerate() {
        if a == "-c" && k + 1 < args.len() {
            let (key, _, value) = py::partition(&args[k + 1], "=");
            if git_key(key) && is_false(py::strip(value)) {
                how.push(format!("-c {}", args[k + 1]));
            }
        }
    }
    if args.first().map(String::as_str) == Some("config") {
        let rest: Vec<&String> = args[1..].iter().filter(|a| !a.starts_with("--")).collect();
        if rest.len() >= 2 && git_key(rest[0]) && is_false(rest[1]) {
            how.push(format!("config {} {}", rest[0], rest[1]));
        }
    }
}

fn pip_config(args: &[String], how: &mut Vec<String>, hosts: &mut Vec<String>) {
    let words: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    if words.len() < 3 || words[0] != "config" || words[1] != "set" {
        return;
    }
    let (key, _, value) = py::partition(words[2], "=");
    let value = if value.is_empty() {
        words.get(3).map_or("", |w| w.as_str())
    } else {
        value
    };
    let name = py::lower(py::rpartition(key, ".").2);
    if name == "trusted-host" {
        how.push(py::strip(&format!("config set {key} {value}")).to_string());
        hosts.extend(split_hosts(value));
    } else if name == "cert" && NOTHING.contains(&py::strip(value)) {
        how.push(format!(
            "config set {key} {}",
            if value.is_empty() { "\"\"" } else { value }
        ));
    }
}

fn npm(args: &[String], how: &mut Vec<String>) {
    for (k, a) in args.iter().enumerate() {
        if a == "--no-strict-ssl"
            || (a.starts_with("--strict-ssl=") && is_false(a.split_once('=').map_or("", |x| x.1)))
            || (a == "--strict-ssl" && k + 1 < args.len() && is_false(&args[k + 1]))
        {
            how.push(a.clone());
        }
    }
    let config = args.iter().position(|a| a == "config");
    let rest = match config {
        Some(i) => &args[i + 1..],
        None => args,
    };
    if rest.first().map(String::as_str) == Some("set") {
        let (key, value) = if rest.len() > 1 {
            let (k, _, v) = py::partition(&rest[1], "=");
            (k.to_string(), v.to_string())
        } else {
            (String::new(), String::new())
        };
        let value = if value.is_empty() {
            rest.get(2).cloned().unwrap_or_default()
        } else {
            value
        };
        if key == "strict-ssl" && is_false(&value) {
            how.push(format!(
                "{}set strict-ssl {value}",
                if config.is_some() { "config " } else { "" }
            ));
        }
    }
}
