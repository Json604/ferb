"""L39 (owner, 2026-10-25): a shell command that turns off TLS certificate checks is classified from its own words
and environment (ferb/policy/tls.py); FERB asks the owner before it runs (tests/test_tls_confirm.py)."""

import pytest

from ferb.policy.tls import certificate_checks_off
from tests.support.live_cases import LIVE_PIP as LIVE


def test_the_live_pip_command_turns_checks_off_for_both_hosts() -> None:
    found = certificate_checks_off(LIVE)
    assert found is not None
    assert found.how == ("--trusted-host pypi.org", "--trusted-host files.pythonhosted.org")
    assert found.hosts == ("pypi.org", "files.pythonhosted.org")


@pytest.mark.parametrize("command, switch", [
    ("pip install x --trusted-host=pypi.org", "--trusted-host pypi.org"),
    ("uv pip install x --allow-insecure-host pypi.org", "--allow-insecure-host pypi.org"),
    ("PIP_TRUSTED_HOST=pypi.org pip install x", "PIP_TRUSTED_HOST=pypi.org"),
    ("export UV_INSECURE_HOST=pypi.org; uv sync", "UV_INSECURE_HOST=pypi.org"),
    ("pip install --cert /dev/null x", "--cert /dev/null"),
    ("python3 -m pip install --cert= x", '--cert ""'),
    ("curl -k https://example.com/x", "-k"),
    ("curl -sSLk https://example.com/x -o out", "-sSLk"),
    ("curl --insecure https://example.com", "--insecure"),
    ("wget --no-check-certificate https://example.com/f", "--no-check-certificate"),
    ("git -c http.sslVerify=false clone https://example.com/r.git", "-c http.sslVerify=false"),
    ("git config --global http.sslverify false", "config http.sslverify false"),
    ("GIT_SSL_NO_VERIFY=1 git clone https://example.com/r.git", "GIT_SSL_NO_VERIFY=1"),
    ("NODE_TLS_REJECT_UNAUTHORIZED=0 node app.js", "NODE_TLS_REJECT_UNAUTHORIZED=0"),
    ("npm config set strict-ssl false", "config set strict-ssl false"),
    ("yarn config set strict-ssl false", "config set strict-ssl false"),
    ("npm install --strict-ssl=false", "--strict-ssl=false"),
    ("PYTHONHTTPSVERIFY=0 python3 fetch.py", "PYTHONHTTPSVERIFY=0"),
    ("REQUESTS_CA_BUNDLE= python3 fetch.py", 'REQUESTS_CA_BUNDLE=""'),
    ("env SSL_CERT_FILE=/dev/null python3 fetch.py", "SSL_CERT_FILE=/dev/null"),
    ("python3 -c 'import ssl, urllib.request as u; u.urlopen(\"https://x\", context=ssl._create_unverified_context())'",
     "python3 code: _create_unverified_context"),
    ("python3 -c \"import requests; requests.get('https://x', verify=False)\"", "python3 code: verify=False"),
    ("python3 - <<'EOF'\nimport requests\nrequests.get('https://x', verify=False)\nEOF", "python3 code: verify=False"),
    ("kubectl --insecure-skip-tls-verify get pods", "--insecure-skip-tls-verify"),
    ("aws s3 ls --no-verify-ssl", "--no-verify-ssl"),
    # review-1 risk 4 (I3): each slipped before
    ('sh -c "pip install --trusted-host files.example x"', "--trusted-host files.example"),
    ("bash -c 'curl -k https://example.com'", "-k"),
    ("bash -lc 'curl -sk https://example.com'", "-sk"),
    ("nice -n 10 curl -k https://example.com", "-k"),
    ("/usr/bin/env NODE_TLS_REJECT_UNAUTHORIZED=0 node app.js", "NODE_TLS_REJECT_UNAUTHORIZED=0"),
    ("sudo -u me curl -k https://example.com", "-k"),
    ("echo `curl -k https://example.com`", "-k"),
    ("pip config set global.trusted-host pypi.org", "config set global.trusted-host pypi.org"),
    ("python3 -m pip config set global.cert ''", 'config set global.cert ""'),
    ("npm set strict-ssl false", "set strict-ssl false"),
    ("pnpm set strict-ssl false", "set strict-ssl false"),
    ("eval 'curl -k https://example.com'", "-k"),
    ("env -S 'curl -k https://example.com'", "-k"),
    ("time -o t.txt curl -k https://example.com", "-k"),
])
def test_each_switch_is_named_as_written(command: str, switch: str) -> None:
    found = certificate_checks_off(command)
    assert found is not None and switch in found.how


@pytest.mark.parametrize("command", [
    "pip install PyMuPDF",
    "python3 -m pip install --quiet --target /tmp/x PyMuPDF",
    "curl -sSL https://example.com -o out",
    "curl -o k https://example.com",               # k is -o's value, not a switch
    "sort -k 2 notes.txt",                         # another program's -k
    "git -c http.sslVerify=true clone https://example.com/r.git",
    "NODE_TLS_REJECT_UNAUTHORIZED=1 node app.js",
    "REQUESTS_CA_BUNDLE=/etc/ssl/cert.pem python3 fetch.py",
    "curl --cert client.pem https://example.com",  # a client certificate, not a check turned off
    "echo 'curl -k is unsafe' > notes.txt",
    "cat <<'EOF' > notes.md\nNever use verify=False; it's unsafe\nEOF",
])
def test_ordinary_commands_turn_nothing_off(command: str) -> None:
    assert certificate_checks_off(command) is None


def test_an_applescript_do_shell_script_is_read_too() -> None:
    from ferb.policy.tls import applescript_checks_off
    found = applescript_checks_off('do shell script "curl -k https://example.com/a -o \\"/tmp/a b\\""')
    assert found is not None and found.how == ("-k",) and found.hosts == ("example.com",)
    assert applescript_checks_off('do shell script "curl -sSL https://example.com"') is None
    assert applescript_checks_off('tell application "Finder" to get name of every window') is None


def test_ordinary_wrapped_commands_still_turn_nothing_off() -> None:
    for command in ("nice -n 10 pip install x", "sudo -u me curl https://example.com", "sh -c 'sort -k 2 f'",
                    "bash -c 'echo curl -k is unsafe'", "env -u HOME curl https://example.com"):
        assert certificate_checks_off(command) is None, command


def test_a_url_the_switch_applies_to_names_its_host() -> None:
    found = certificate_checks_off("curl -k https://files.example.com/a.zip -o a.zip")
    assert found is not None and found.hosts == ("files.example.com",)
