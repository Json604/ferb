"""Owns the certificate-check floor's facts (L39; owner, 2026-10-25: "yes" to asking before any command that turns off
certificate checks): whether a shell command turns off TLS certificate verification, read from the command's own
words and environment assignments as written. Must not judge intent: it classifies the text, like the destructive
floor (rule 5); FERB asks the owner (policy/confirm.py, `floor:tls_off`) and the owner decides.

What counts, in a command or text sent to a running shell (each simple command after `;`, `&&`, `||`, `|`, `&`, a
newline; a heredoc's body goes with the command it feeds):
- pip / uv: `--trusted-host`, `--allow-insecure-host`, a `--cert` that names nothing (empty or /dev/null), and the
  environment's PIP_TRUSTED_HOST, UV_INSECURE_HOST, PIP_CERT (empty or /dev/null);
- curl `-k` / `--insecure` (also inside grouped short options: `-sSLk`); wget `--no-check-certificate` or
  `--check-certificate=off`;
- git `-c http[.<url>].sslVerify=false`, `git config … http[.<url>].sslVerify false`, and GIT_SSL_NO_VERIFY;
- NODE_TLS_REJECT_UNAUTHORIZED=0, and `node -e` code with `rejectUnauthorized: false`;
- npm / yarn / pnpm `strict-ssl false` (`config set`, `--strict-ssl=false`, `--no-strict-ssl`), and
  npm_config_strict_ssl=false;
- PYTHONHTTPSVERIFY=0, and inline Python (`python -c`, or a heredoc fed to python) with
  `ssl._create_unverified_context`, `verify=False`, `CERT_NONE` or `check_hostname = False`;
- REQUESTS_CA_BUNDLE, SSL_CERT_FILE or CURL_CA_BUNDLE set to empty or /dev/null;
- kubectl / helm `--insecure-skip-tls-verify`, aws `--no-verify-ssl`, and GOINSECURE;
- (step 6b) a heredoc fed to a shell (`sh <<EOF … EOF`), text piped into a shell (`printf '…' | sh`, `echo … | bash`),
  a remote command (`ssh host curl -k …`), `wget -e check_certificate=off` (`--execute`), git's GIT_CONFIG_COUNT /
  GIT_CONFIG_KEY_n / GIT_CONFIG_VALUE_n and GIT_CONFIG_PARAMETERS, `node --eval=…` / `--print=…`, deno
  `--unsafely-ignore-certificate-errors`, docker / podman / buildah / skopeo `--tls-verify=false` (`--tlsverify=false`),
  curl `--proxy-insecure`, `--doh-insecure` and a config file it reads (`-K` / `--config`: FERB can't see inside, so
  it counts), httpie `--verify=no`, aria2c `--check-certificate=false`, yarn `enableStrictSsl false`, and conda's
  `ssl_verify false` / CONDA_SSL_VERIFY.
It only ever adds a question. Residual: a script or configuration file that turns checks off inside (a .py file run by
name, pip.conf, .npmrc, .curlrc, .wgetrc, a gitconfig written earlier), a flag built at run time (`f=-k; curl $f`),
and programs not listed."""

import re
import shlex
from dataclasses import dataclass
from urllib.parse import urlsplit

_OPERATORS = {";", "&&", "||", "|", "&", "\n", "(", ")", ";;", "|&"}
_WRAPPERS = {"env", "sudo", "command", "exec", "time", "nohup", "nice", "caffeinate", "xargs", "builtin", "doas",
             "timeout", "gtimeout", "arch", "stdbuf", "watch", "chronic", "unbuffer"}
# A wrapper's options that take a value (the next word, or the rest of the word): nice -n 10, sudo -u me, env -u NAME,
# time -o file, xargs -n 1, caffeinate -t 60. env -S's value is a command line of its own (read as one).
_VALUED = {"nice": set("n"), "sudo": set("ugCDhprtUT"), "doas": set("uC"), "env": set("uSCP"), "time": set("fo"),
           "xargs": set("nILPsEdJR"), "caffeinate": set("tw"), "timeout": set("sk"), "gtimeout": set("sk"),
           "arch": set("ed"), "stdbuf": set("ioe"), "watch": set("nq"), "exec": set("a")}
_LEADING = {"timeout": 1, "gtimeout": 1}       # positionals before the command (timeout's duration)
_SHELLS = re.compile(r"(ba|z|da|k|fi)?sh")
_ASSIGNMENT = re.compile(r"([A-Za-z_][A-Za-z0-9_]*)=(.*)", re.S)
_HEREDOC = re.compile(r"<<-?[ \t]*(['\"]?)([A-Za-z_][A-Za-z0-9_]*)\1")
_FALSE = {"false", "0", "no", "off"}
_NOTHING = {"", "/dev/null"}
# curl's short options that take a value (curl(1)): in a group like -sSo, the rest of the group is that value
_CURL_VALUED = set("AbcCdDeEFHKmoPQrtTuUwxXyYz")
_PYTHON_CODE = re.compile(r"_create_unverified_context|\bverify\s*=\s*False\b|\bCERT_NONE\b|"
                          r"\bcheck_hostname\s*=\s*False\b")
_NODE_CODE = re.compile(r"rejectUnauthorized\s*:\s*false|NODE_TLS_REJECT_UNAUTHORIZED")
_PYTHON = re.compile(r"python(\d+(\.\d+)?)?")
_PIP = re.compile(r"pip(\d+(\.\d+)?)?|uv|pipx")
_GIT_KEY = re.compile(r"http(\..+)?\.sslverify", re.I)


@dataclass(frozen=True)
class TlsOff:
    """How a command turns certificate checks off (`how`: each switch as written) and the sites it names for them
    (`hosts`: the hosts it trusts or reaches; empty when it names none)."""
    how: tuple[str, ...]
    hosts: tuple[str, ...]


def _bodies(command: str) -> tuple[str, list[tuple[str, str]]]:
    """The command without its heredoc bodies, and each body with the line whose `<<` feeds it."""
    lines, kept, bodies, i = command.split("\n"), [], [], 0
    while i < len(lines):
        line = lines[i]
        kept.append(line)
        i += 1
        for m in _HEREDOC.finditer(line):
            dash, word, body = line[m.start():m.start() + 3] == "<<-", m.group(2), []
            while i < len(lines) and (lines[i].strip() if dash else lines[i]) != word:
                body.append(lines[i])
                i += 1
            i += 1                                         # the delimiter line
            bodies.append((line, "\n".join(body)))
    return "\n".join(kept), bodies


def _words(text: str) -> list[str]:
    """Shell words and operators (a newline is an operator); a text shlex can't split (an unclosed quote) is split
    on whitespace and the operators, quotes dropped."""
    try:
        lex = shlex.shlex(text, posix=True, punctuation_chars=";&|<>()\n")
        lex.whitespace, lex.whitespace_split, lex.commenters = " \t\r", True, ""
        return list(lex)
    except ValueError:
        return [w.strip("'\"") for w in re.findall(r"&&|\|\||[;&|()\n]|[^\s;&|()]+", text)]


def _piped(words: list[str]) -> list[tuple[list[str], str]]:
    """Each simple command with the operator after it ("" at the end)."""
    out: list[tuple[list[str], str]] = [([], "")]
    for w in words:
        if w in _OPERATORS:
            out[-1] = (out[-1][0], w)
            out.append(([], ""))
        elif not (w and set(w) <= set("<>")):              # a redirection's arrow; its target stays a word
            out[-1][0].append(w)
    return [(c, op) for c, op in out if c]


def _stripped(cmd: list[str]) -> list[str]:
    """A command without its leading assignments and wrappers (env, sudo, nice…), roughly: for pipeline checks."""
    i = 0
    while i < len(cmd) and (_ASSIGNMENT.fullmatch(cmd[i]) or cmd[i].rpartition("/")[2] in _WRAPPERS):
        i += 1
    return cmd[i:]


def _printed(cmd: list[str]) -> str | None:
    """The text an `echo` or `printf` writes (its words after options; printf's and echo -e's `\\n` as newlines)."""
    args = _stripped(cmd)
    if not args or args[0].rpartition("/")[2] not in ("echo", "printf"):
        return None
    rest = args[1:]
    while rest and rest[0].startswith("-") and len(rest[0]) > 1 and args[0] != "printf":
        rest = rest[1:]
    return " ".join(rest).replace("\\n", "\n").replace("\\t", "\t")


def _reads_stdin_shell(cmd: list[str]) -> bool:
    """A shell reading its commands from standard input: `sh`, `bash -s`, never `sh -c …` or `sh script`."""
    args = _stripped(cmd)
    if not args or not _SHELLS.fullmatch(args[0].rpartition("/")[2]):
        return False
    rest = args[1:]
    return all(a.startswith("-") and "c" not in a.lstrip("-") for a in rest) or "-s" in rest


_SSH_VALUED = set("BbcDEeFIiJLlmOoPpQRSWw")


def _ssh_command(args: list[str]) -> str | None:
    """The remote command of `ssh [options] host command…` (its words after the host), else None."""
    i = 0
    while i < len(args) and args[i].startswith("-") and args[i] != "--":
        flags = args[i][1:]
        valued = next((k for k, c in enumerate(flags) if c in _SSH_VALUED), None)
        i += 2 if valued is not None and valued == len(flags) - 1 else 1
    if i < len(args) and args[i] == "--":
        i += 1
    return " ".join(args[i + 1:]) if i + 1 < len(args) else None


def _host(value: str) -> str | None:
    value = value.strip()
    if not value:
        return None
    try:
        parsed = urlsplit(value if "://" in value else "//" + value)
        return (parsed.hostname or "").lower() or None
    except ValueError:
        return None


_DO_SHELL = re.compile(r'do\s+shell\s+script\s+"((?:[^"\\]|\\.)*)"', re.I)


def applescript_checks_off(script: str) -> TlsOff | None:
    """The same facts for the shell commands an AppleScript runs (`do shell script "…"`, AppleScript's own escapes)."""
    found = [certificate_checks_off(re.sub(r'\\(.)', r'\1', m.group(1))) for m in _DO_SHELL.finditer(script)]
    how = tuple(dict.fromkeys(h for f in found if f for h in f.how))
    return TlsOff(how, tuple(dict.fromkeys(h for f in found if f for h in f.hosts))) if how else None


def certificate_checks_off(command: str, depth: int = 0) -> TlsOff | None:
    """The command's switches that turn TLS certificate checks off, as written, and the sites they concern; None when
    it has none. Reads the text only (rule 5); it never runs or resolves anything. A shell's -c code, eval's words and
    env -S's command line are read as commands too (a few levels deep), and backquotes split commands like $( )."""
    text, bodies = _bodies(command)
    how: list[str] = []
    hosts: list[str] = []
    piped = _piped(_words(text.replace("`", "\n")))
    every_env: list[tuple[str, str]] = []                 # assignments anywhere (export … ; export … ; git …)
    nested: list[str] = []                                # text piped into a shell (printf '…' | sh)
    for k, (cmd, op) in enumerate(piped):
        if op == "|" and k + 1 < len(piped) and _reads_stdin_shell(piped[k + 1][0]) and \
                (printed := _printed(cmd)) is not None:
            nested.append(printed)
    for cmd, _op in piped:
        env: list[tuple[str, str]] = []
        inner: list[str] = []                             # command lines a wrapper or shell carries (read as commands)
        i = 0
        while i < len(cmd):                               # leading assignments, `export`, and wrappers like env/sudo
            word, base = cmd[i], cmd[i].rpartition("/")[2]
            m = _ASSIGNMENT.fullmatch(word)
            if m and not word.startswith("-"):
                env.append((m.group(1), m.group(2)))
                i += 1
            elif word in ("export", "declare", "local", "readonly") or base in _WRAPPERS:
                wrapper, i = base, i + 1
                while i < len(cmd) and cmd[i].startswith("-") and cmd[i] != "--":   # its options, valued ones too
                    if wrapper == "arch" and cmd[i] == "-arch":     # arch -arch arm64
                        i += 2
                        continue
                    if cmd[i].startswith("--"):                     # a long option: its value is after "="
                        i += 1
                        continue
                    flags = cmd[i][1:]
                    valued = next((k for k, c in enumerate(flags) if c in _VALUED.get(wrapper, set())), None)
                    takes = valued is not None and valued == len(flags) - 1 and i + 1 < len(cmd)
                    if valued is not None and wrapper == "env" and flags[valued] == "S":
                        inner.append(flags[valued + 1:] or (cmd[i + 1] if i + 1 < len(cmd) else ""))
                    i += 2 if takes else 1
                if i < len(cmd) and cmd[i] == "--":
                    i += 1
                i += min(_LEADING.get(wrapper, 0), max(0, len(cmd) - i - 1))
                if wrapper == "watch" and i == len(cmd) - 1 and " " in cmd[i]:   # watch 'curl -k …': a command line
                    inner.append(cmd[i])
            else:
                break
        for name, value in env:
            _environment(name, value, how, hosts)
        every_env += env
        args = cmd[i:]
        if args:
            program = args[0].rpartition("/")[2]
            if _SHELLS.fullmatch(program):                # sh -c '…', bash -lc '…': the code is a command line
                for k, a in enumerate(args[1:-1], 1):
                    if a.startswith("-") and not a.startswith("--") and "c" in a:
                        inner.append(args[k + 1])
                        break
            elif program == "eval":
                inner.append(" ".join(args[1:]))
            elif program == "ssh" and (remote := _ssh_command(args[1:])):   # ssh host curl -k …: the remote command
                inner.append(remote)
            elif program == "osascript":                  # osascript -e '…do shell script "…"…'
                script = "\n".join(args[k + 1] for k, a in enumerate(args[1:-1], 1) if a == "-e")
                if script and (found := applescript_checks_off(script)) is not None:
                    how += found.how
                    hosts += found.hosts
        for line in inner if depth < 3 else ():
            if (found := certificate_checks_off(line, depth + 1)) is not None:
                how += found.how
                hosts += found.hosts
        if not args:
            continue
        program = args[0].rpartition("/")[2]
        rest = args[1:]
        _anywhere(rest, how, hosts)
        if program == "curl":
            _curl(rest, how)
        elif program == "wget":
            how += [a for a in rest if a == "--no-check-certificate" or
                    (a.startswith("--check-certificate=") and a.split("=", 1)[1].lower() in _FALSE)]
            for k, a in enumerate(rest):                  # wget -e check_certificate=off (a .wgetrc command)
                value = (a[2:] if a.startswith("-e") and len(a) > 2 else a.split("=", 1)[1] if a.startswith(
                    "--execute=") else rest[k + 1] if a in ("-e", "--execute") and k + 1 < len(rest) else None)
                setting, _, flag = (value or "").replace(" ", "").partition("=")
                if setting.lower() == "check_certificate" and flag.lower() in _FALSE:
                    how.append(f"-e {value}")
        elif program in ("http", "https") and any(a.split("=", 1)[1].lower() in _FALSE | {"no"}
                                                  for a in rest if a.startswith("--verify=")):
            how += [a for a in rest if a.startswith("--verify=")]
        elif program in ("aria2c",):
            how += [a for a in rest if a.startswith("--check-certificate=") and a.split("=", 1)[1].lower() in _FALSE]
        elif program == "conda" and "ssl_verify" in rest and any(a.lower() in _FALSE for a in rest):
            how.append("config --set ssl_verify false")
        elif program == "git":
            _git(rest, how)
        elif program in ("npm", "yarn", "pnpm", "npx"):
            _npm(rest, how)
        elif _PYTHON.fullmatch(program) or program == "node":
            code = rest[rest.index("-c") + 1] if program != "node" and "-c" in rest[:-1] else \
                next((rest[k + 1] for k, a in enumerate(rest[:-1]) if a in ("-e", "--eval", "-p", "--print")), "")
            if not code and program == "node":           # node --eval=… / --print=…
                code = next((a.split("=", 1)[1] for a in rest if a.startswith(("--eval=", "--print="))), "")
            pattern = _NODE_CODE if program == "node" else _PYTHON_CODE
            if code and (match := pattern.search(code)):
                how.append(f"{program} code: {match.group(0)}")
        if _PIP.fullmatch(program) or (_PYTHON.fullmatch(program) and rest[:2] == ["-m", "pip"]):
            _pip_config(rest[2:] if rest[:2] == ["-m", "pip"] else rest, how, hosts)
            for k, a in enumerate(rest):
                cert = a.split("=", 1)[1] if a.startswith("--cert=") else (
                    rest[k + 1] if a == "--cert" and k + 1 < len(rest) else None)
                if cert is not None and cert.strip() in _NOTHING:
                    how.append(f"--cert {cert or chr(34) * 2}")
        hosts += [h for a in rest if "://" in a and (h := _host(a))]
    for line, body in bodies:                             # a heredoc fed to python or node: its code
        feeds = re.search(r"(?:^|[\s/;&|(])(python(?:\d+(?:\.\d+)?)?|node)(?=[\s<]|$)", line)
        if feeds and (said := (_NODE_CODE if feeds.group(1) == "node" else _PYTHON_CODE).search(body)):
            how.append(f"{feeds.group(1)} code: {said.group(0)}")
        elif re.search(r"(?:^|[\s/;&|(])(?:ba|z|da|k|fi)?sh(?=[\s<]|$)", line):   # a heredoc fed to a shell
            nested.append(body)
    for line in nested if depth < 3 else ():
        if (found := certificate_checks_off(line, depth + 1)) is not None:
            how += found.how
            hosts += found.hosts
    _git_environment(every_env, how)
    if not how:
        return None
    return TlsOff(tuple(dict.fromkeys(how)), tuple(dict.fromkeys(hosts)))


def _environment(name: str, value: str, how: list[str], hosts: list[str]) -> None:
    upper = name.upper()
    if upper in ("PIP_TRUSTED_HOST", "UV_INSECURE_HOST", "GOINSECURE") and value.strip():
        how.append(f"{name}={value}")
        hosts += [h for v in re.split(r"[\s,]+", value) if (h := _host(v))]
    elif upper == "GIT_SSL_NO_VERIFY" or (upper in ("NODE_TLS_REJECT_UNAUTHORIZED", "PYTHONHTTPSVERIFY") and
                                         value.strip() == "0"):
        how.append(f"{name}={value}")
    elif upper in ("NPM_CONFIG_STRICT_SSL", "CONDA_SSL_VERIFY") and value.strip().lower() in _FALSE:
        how.append(f"{name}={value}")
    elif upper in ("REQUESTS_CA_BUNDLE", "SSL_CERT_FILE", "CURL_CA_BUNDLE", "PIP_CERT") and value.strip() in _NOTHING:
        how.append(f"{name}={value or chr(34) * 2}")


def _git_environment(env: list[tuple[str, str]], how: list[str]) -> None:
    """git's configuration from the environment: GIT_CONFIG_KEY_n=http.sslVerify with GIT_CONFIG_VALUE_n=false, and
    GIT_CONFIG_PARAMETERS holding `http….sslVerify=false`."""
    values = {name.upper(): value for name, value in env}
    for name, key in values.items():
        if name.startswith("GIT_CONFIG_KEY_") and _GIT_KEY.fullmatch(key.strip()):
            value = values.get("GIT_CONFIG_VALUE_" + name[len("GIT_CONFIG_KEY_"):], "")
            if value.strip().lower() in _FALSE:
                how.append(f"{name}={key} GIT_CONFIG_VALUE_{name[len('GIT_CONFIG_KEY_'):]}={value}")
    params = values.get("GIT_CONFIG_PARAMETERS", "")
    for m in re.finditer(r"(http(?:\.[^'\s=]+)?\.sslverify)\s*=\s*'?(\w+)", params, re.I):
        if m.group(2).lower() in _FALSE:
            how.append(f"GIT_CONFIG_PARAMETERS={params}")
            break


def _anywhere(args: list[str], how: list[str], hosts: list[str]) -> None:
    """Switches only certificate-skipping uses spell this way, whatever program takes them."""
    for k, a in enumerate(args):
        flag, eq, value = a.partition("=")
        if flag == "--unsafely-ignore-certificate-errors":          # deno, every site or the ones listed
            how.append(a)
            hosts += [h for v in value.split(",") if (h := _host(v))]
            continue
        if flag in ("--tls-verify", "--tlsverify"):                  # docker, podman, buildah, skopeo
            setting = value if eq else (args[k + 1] if k + 1 < len(args) else "")
            if setting.lower() in _FALSE:
                how.append(f"{flag}={setting}")
            continue
        if flag == "enableStrictSsl" or (a == "enableStrictSsl" and k + 1 < len(args)):   # yarn config set …
            setting = value if eq else args[k + 1]
            if setting.lower() in _FALSE:
                how.append(f"enableStrictSsl {setting}")
            continue
        if flag in ("--trusted-host", "--allow-insecure-host"):
            value = value if eq else (args[k + 1] if k + 1 < len(args) else "")
            how.append(f"{flag} {value}".strip())
            if h := _host(value):
                hosts.append(h)
        elif flag == "--insecure-skip-tls-verify" and (not eq or value.lower() not in _FALSE):
            how.append(a)
        elif a == "--no-verify-ssl":
            how.append(a)


_CURL_CONFIG = "a config file FERB can't read"


def _curl(args: list[str], how: list[str]) -> None:
    skip = False
    for k, a in enumerate(args):
        if skip:
            skip = False
            continue
        if a in ("--insecure", "--proxy-insecure", "--doh-insecure"):
            how.append(a)
        elif a == "--config" or a.startswith("--config="):           # its settings could skip the checks
            how.append(f"{a} ({_CURL_CONFIG})" if "=" in a else
                       f"--config {args[k + 1] if k + 1 < len(args) else ''} ({_CURL_CONFIG})")
            skip = "=" not in a
        elif a.startswith("-") and not a.startswith("--") and len(a) > 1:
            for j, ch in enumerate(a[1:]):
                if ch == "k":
                    how.append(a)
                    break
                if ch == "K":                              # -K file: the config file it reads
                    named = a[j + 2:] or (args[k + 1] if k + 1 < len(args) else "")
                    how.append(f"-K {named} ({_CURL_CONFIG})")
                    skip = j == len(a) - 2
                    break
                if ch in _CURL_VALUED:
                    skip = j == len(a) - 2                 # its value is the next word
                    break


def _git(args: list[str], how: list[str]) -> None:
    for k, a in enumerate(args):
        if a == "-c" and k + 1 < len(args):
            key, _, value = args[k + 1].partition("=")
            if _GIT_KEY.fullmatch(key) and value.strip().lower() in _FALSE:
                how.append(f"-c {args[k + 1]}")
    if args[:1] == ["config"]:
        rest = [a for a in args[1:] if not a.startswith("--")]
        if len(rest) >= 2 and _GIT_KEY.fullmatch(rest[0]) and rest[1].lower() in _FALSE:
            how.append(f"config {rest[0]} {rest[1]}")


def _pip_config(args: list[str], how: list[str], hosts: list[str]) -> None:
    """`pip config set <scope>.trusted-host <host>` and `pip config set <scope>.cert ''` (a lasting setting)."""
    words = [a for a in args if not a.startswith("-")]
    if words[:2] != ["config", "set"] or len(words) < 3:
        return
    key, _, value = words[2].partition("=")
    value = value or (words[3] if len(words) > 3 else "")
    name = key.rpartition(".")[2].lower()
    if name == "trusted-host":
        how.append(f"config set {key} {value}".strip())
        hosts += [h for v in re.split(r"[\s,]+", value) if (h := _host(v))]
    elif name == "cert" and value.strip() in _NOTHING:
        how.append(f"config set {key} {value or chr(34) * 2}")


def _npm(args: list[str], how: list[str]) -> None:
    for k, a in enumerate(args):
        if a == "--no-strict-ssl" or (a.startswith("--strict-ssl=") and a.split("=", 1)[1].lower() in _FALSE) or (
                a == "--strict-ssl" and k + 1 < len(args) and args[k + 1].lower() in _FALSE):
            how.append(a)
    rest = args[args.index("config") + 1:] if "config" in args else args     # `npm config set` and `npm set`
    if rest[:1] == ["set"]:
        key, _, value = rest[1].partition("=") if len(rest) > 1 else ("", "", "")
        value = value or (rest[2] if len(rest) > 2 else "")
        if key == "strict-ssl" and value.lower() in _FALSE:
            how.append(f"{'config ' if 'config' in args else ''}set strict-ssl {value}")
