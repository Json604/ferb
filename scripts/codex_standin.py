"""Developer toolbox (RUN-412, RUN-449): FERB's real Codex adapter (codex_stream.start and events, the same flags,
feature lockdown, locked catalog, item gate and tool-catalog check) with the pinned Codex binary, against a local
stand-in for ChatGPT's backend that logs every request. Offline: Codex runs under sandbox-exec with only localhost
reachable, a temporary HOME and CODEX_HOME and a made-up ChatGPT login; nothing reads ~/.ferb or a real login.

    uv run python scripts/codex_standin.py [--codex-root DIR]

DIR holds node_modules/@openai/codex-* (default: this checkout). Runs, each a planner turn with two lookup tools:
- ferb:     FERB's provider entry (RUN-449). Must: no WebSocket upgrade, the request as an HTTP POST, the catalog
            check passed with only FERB's tools and Codex's base tools, a final answer.
- builtin:  the built-in `openai` provider, as before RUN-449 (the WebSocket refused with 426, as ChatGPT does).
            Shows the upgrade attempt the entry removes.
- unlocked: FERB's entry with the feature lockdown and the locked catalog off. Must stop with a violation.
Exit status 0 when every run did what it must."""

import argparse
import asyncio
import base64
import json
import os
import shutil
import ssl
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

SHIM_TOOLS = ["memory.read", "trail.query"]
ANSWER = '{"text":"ok"}'


def shim() -> None:
    """A minimal MCP tool server on stdio (Codex's tool bridge stand-in): it lists SHIM_TOOLS and runs none."""
    from ferb.runtimes.codex_exec import mcp_name
    for line in sys.stdin:
        msg = json.loads(line)
        result: Any = None
        if msg.get("method") == "initialize":
            result = {"protocolVersion": msg["params"]["protocolVersion"], "capabilities": {"tools": {}},
                      "serverInfo": {"name": "ferb", "version": "1"}}
        elif msg.get("method") == "tools/list":
            result = {"tools": [{"name": mcp_name(n), "description": "A FERB lookup",
                                 "inputSchema": {"type": "object", "properties": {}}} for n in SHIM_TOOLS]}
        elif "id" in msg:
            result = {}
        if "id" in msg:
            sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": result}) + "\n")
            sys.stdout.flush()


def _jwt(claims: dict[str, Any]) -> str:
    part = [base64.urlsafe_b64encode(json.dumps(d).encode()).decode().rstrip("=") for d in ({"alg": "none"}, claims)]
    return f"{part[0]}.{part[1]}.sig"


def fake_login() -> dict[str, Any]:
    """A made-up ChatGPT login with an account id, so Codex makes its workspace routing lookup as it does live."""
    now, account = int(time.time()), "00000000-0000-0000-0000-000000000000"
    auth = {"chatgpt_account_id": account, "chatgpt_plan_type": "pro", "chatgpt_user_id": "user-fake",
            "user_id": "user-fake"}
    claims = {"https://api.openai.com/auth": auth, "iat": now, "exp": now + 86400, "sub": "fake"}
    return {"auth_mode": "chatgpt", "OPENAI_API_KEY": None, "last_refresh": "2026-11-09T00:00:00.000000Z",
            "tokens": {"id_token": _jwt({**claims, "email": "fake@example.com"}), "access_token": _jwt(claims),
                       "refresh_token": "rt_fake", "account_id": account}}


class StandIn:
    """ChatGPT's backend for one run: routing, settings, the Responses API over HTTP+SSE; a WebSocket upgrade is
    logged and refused (426)."""

    def __init__(self) -> None:
        self.log: list[dict[str, Any]] = []
        self.port = 0

    def sse(self) -> bytes:
        item = {"type": "message", "id": "m1", "role": "assistant", "status": "completed", "phase": "final_answer",
                "content": [{"type": "output_text", "text": ANSWER, "annotations": []}]}
        events = [{"type": "response.created", "response": {"id": "r1"}},
                  {"type": "response.output_item.added", "output_index": 0, "item": {**item, "content": [],
                                                                                    "status": "in_progress"}},
                  {"type": "response.output_text.delta", "item_id": "m1", "output_index": 0, "content_index": 0,
                   "delta": ANSWER},
                  {"type": "response.output_item.done", "output_index": 0, "item": item},
                  {"type": "response.completed", "response": {"id": "r1", "usage": {
                      "input_tokens": 100, "input_tokens_details": {"cached_tokens": 0}, "output_tokens": 5,
                      "output_tokens_details": {"reasoning_tokens": 0}, "total_tokens": 105}}}]
        return "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in events).encode()

    async def handle(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        try:
            while True:
                try:
                    head = (await reader.readuntil(b"\r\n\r\n")).decode(errors="replace").split("\r\n")
                except (asyncio.IncompleteReadError, ConnectionError):
                    return
                method, path, _ = head[0].split(" ", 2)
                headers = {k.strip().lower(): v.strip() for k, v in (h.split(":", 1) for h in head[1:] if ":" in h)}
                self.log.append({"method": method, "path": path, "websocket": headers.get("upgrade") == "websocket",
                                 "version": headers.get("version"), "encoding": headers.get("content-encoding"),
                                 "header_names": sorted(headers)})       # names only: values may hold the login
                if "content-length" in headers:
                    await reader.readexactly(int(headers["content-length"]))
                elif headers.get("transfer-encoding", "").lower() == "chunked":
                    while (size := int((await reader.readline()).split(b";")[0].strip(), 16)):
                        await reader.readexactly(size + 2)
                    await reader.readline()
                status, kind, body = "404 Not Found", "application/json", b"{}"
                if headers.get("upgrade", "").lower() == "websocket":
                    status = "426 Upgrade Required"
                elif method == "POST" and path.rstrip("/").endswith("/responses"):
                    status, kind, body = "200 OK", "text/event-stream", self.sse()
                elif path.endswith("/wham/accounts/check"):
                    status, body = "200 OK", json.dumps({"accounts": [{
                        "id": "00000000-0000-0000-0000-000000000000", "structure": "personal", "plan_type": "pro",
                        "can_access_with_session": True, "is_deactivated": False, "account_routing_override": "us",
                        "workspace_backend_origin": f"https://127.0.0.1:{self.port}"}]}).encode()
                elif path.endswith("/wham/settings/user"):
                    status = "200 OK"
                writer.write(f"HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {len(body)}\r\n\r\n"
                             .encode() + body)
                await writer.drain()
        finally:
            writer.close()


def certificate(folder: Path) -> ssl.SSLContext:
    """A self-signed certificate for 127.0.0.1 (Codex sends routed requests only to an HTTPS origin), trusted by Codex
    through CODEX_CA_CERTIFICATE; the stand-in's TLS context."""
    import datetime
    import ipaddress

    from cryptography import x509
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import ec
    from cryptography.x509.oid import NameOID
    key = ec.generate_private_key(ec.SECP256R1())
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "127.0.0.1")])
    now = datetime.datetime.now(datetime.timezone.utc)
    cert = (x509.CertificateBuilder().subject_name(name).issuer_name(name).public_key(key.public_key())
            .serial_number(x509.random_serial_number()).not_valid_before(now - datetime.timedelta(minutes=5))
            .not_valid_after(now + datetime.timedelta(days=1))
            .add_extension(x509.SubjectAlternativeName([x509.IPAddress(ipaddress.ip_address("127.0.0.1"))]), False)
            .add_extension(x509.BasicConstraints(ca=False, path_length=None), True)
            .sign(key, hashes.SHA256()))
    (folder / "cert.pem").write_bytes(cert.public_bytes(serialization.Encoding.PEM))
    (folder / "key.pem").write_bytes(key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
                                                       serialization.NoEncryption()))
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(folder / "cert.pem", folder / "key.pem")
    context.set_alpn_protocols(["http/1.1"])
    return context


async def one_run(name: str, codex_root: Path) -> dict[str, Any]:
    from ferb.core.types import Identity, ModelRequest, Task
    from ferb.runtimes import codex_exec as cx
    from ferb.runtimes import codex_stream as stream
    standin = StandIn()
    saved = stream._command, stream.PROVIDER_ID, stream.CHATGPT_CODEX_BASE_URL, cx.DISABLED
    with tempfile.TemporaryDirectory(prefix="ferb-codex-standin-") as folder:
        home = Path(folder)
        server = await asyncio.start_server(standin.handle, "127.0.0.1", 0, ssl=certificate(home))
        standin.port = server.sockets[0].getsockname()[1]
        base = f"https://127.0.0.1:{standin.port}"
        os.environ["CODEX_CA_CERTIFICATE"] = str(home / "cert.pem")
        (home / "codex-home").mkdir(mode=0o700)
        (home / "codex-home" / "auth.json").write_text(json.dumps(fake_login()))
        (home / "node").symlink_to(codex_root)                     # codex_binary's installed location
        binary = cx.codex_binary(home)
        cx.bundled_catalog(home)
        catalog = cx.write_locked_catalog(home)
        slugs = [m["slug"] for m in json.loads(catalog.read_text())["models"]] if catalog else []
        model = cx.default_model() if not slugs or cx.default_model() in slugs else slugs[0]
        stream.CHATGPT_CODEX_BASE_URL = f"{base}/backend-api/codex"
        if name == "builtin":
            stream.PROVIDER_ID = "openai"
        if name == "unlocked":
            cx.DISABLED = []

        def command(runtime: Any, locked: Path | None, token_file: Path | None = None) -> list[str]:
            cmd = saved[0](runtime, None if name == "unlocked" else locked, token_file)
            if name == "builtin":           # the built-in entry can't be redefined: point it at the stand-in instead
                at = cmd.index(f"model_providers.openai={stream._toml(stream.provider_entry())}")
                del cmd[at - 1:at + 1]
                cmd += ["-c", f'openai_base_url="{base}/backend-api/codex"']
            profile = ('(version 1)(allow default)(deny network-outbound)'
                       '(allow network-outbound (remote ip "localhost:*"))(allow network-outbound (remote unix-socket))')
            return ["/usr/bin/sandbox-exec", "-p", profile, *cmd, "-c", f'chatgpt_base_url="{base}/backend-api/"']
        stream._command = command
        try:
            rt = cx.CodexRuntime(binary, "supervisor", home, model=model, timeout_s=60, tool_names=SHIM_TOOLS,
                                 shim_cmd=[sys.executable, "-I", str(Path(__file__).resolve()), "--shim"],
                                 output_schema={"type": "object", "properties": {"text": {"type": "string"}},
                                                "required": ["text"], "additionalProperties": False},
                                 cache_role="supervisor", cache_scope="standin", cache_refusal=lambda task: "")
            task = Task("t", None, "reply", "d", Identity.OWNER, set(), "mixed", "r", [], None)
            request = ModelRequest("FERB instructions", ("stable context", "the owner's message"), cacheable=1)
            started = time.monotonic()
            run = await stream.start(rt, task, request, run_token="standin")
            got = [(ev.kind, ev.data) async for ev in stream.events(rt, run)]
            seconds = round(time.monotonic() - started, 2)
        finally:
            stream._command, stream.PROVIDER_ID, stream.CHATGPT_CODEX_BASE_URL, cx.DISABLED = saved
            server.close()
    final = next((data for kind, data in got if kind == "final"), None)
    violation = next((data for kind, data in got if kind == "violation"), None)
    return {"run": name, "seconds": seconds, "requests": standin.log,
            "websocket_attempts": sum(r["websocket"] for r in standin.log),
            "posts": [r for r in standin.log if r["method"] == "POST"],
            "final": final, "violation": violation, "allowed": sorted(rt.allowed),
            "errors": [data for kind, data in got if kind == "error"]}


def verdict(result: dict[str, Any]) -> list[str]:
    """What the run failed to do, from its own record; [] when it did what it must."""
    from ferb.runtimes.codex_exec import CODEX_VERSION
    problems = []
    if result["run"] == "ferb":
        if result["websocket_attempts"]:
            problems.append("Codex tried the WebSocket")
        if [(p["path"], p["version"]) for p in result["posts"]] != [("/backend-api/codex/responses", CODEX_VERSION)]:
            problems.append(f"expected one POST with the version header: {result['posts']}")
        final = result["final"] or {}
        if final.get("text") != ANSWER or result["violation"]:
            problems.append(f"no clean final answer: {final} {result['violation']} {result['errors']}")
        elif not {"mcp__ferb.memory_read", "mcp__ferb.trail_query"} <= set(final.get("tools_seen", [])) \
                or not set(final["tools_seen"]) <= set(result["allowed"]):
            problems.append(f"the request's tools: {final.get('tools_seen')}")
    elif result["run"] == "builtin":
        if not result["websocket_attempts"]:
            problems.append("the built-in provider made no WebSocket attempt (has Codex changed?)")
    elif not result["violation"] or result["final"]:
        problems.append(f"the unlocked run didn't stop: {result['final']} {result['errors']}")
    return problems


async def main() -> int:
    from ferb.device.install import repo_root
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--codex-root", type=Path, default=repo_root())
    args = parser.parse_args()
    if not shutil.which("sandbox-exec"):
        print("sandbox-exec is needed")
        return 2
    failed, results = False, {}
    for name in ("builtin", "ferb", "unlocked"):
        result = results[name] = await one_run(name, args.codex_root.resolve())
        problems = verdict(result)
        if name == "ferb":     # the same request as the built-in provider's: header names, compression, tools
            same = [(p["header_names"], p["encoding"]) for p in result["posts"]] == \
                [(p["header_names"], p["encoding"]) for p in results["builtin"]["posts"]] and \
                (result["final"] or {}).get("tools_seen") == (results["builtin"]["final"] or {}).get("tools_seen")
            problems += [] if same else [f"the POST differs from the built-in provider's: {result['posts']} vs "
                                         f"{results['builtin']['posts']}"]
        failed = failed or bool(problems)
        print(json.dumps({"run": name, "ok": not problems, "problems": problems, "seconds": result["seconds"],
                          "requests": [f"{r['method']} {r['path']}" + (" (websocket)" if r["websocket"] else "")
                                       for r in result["requests"]],
                          "posts": [{k: p[k] for k in ("path", "version", "encoding", "header_names")}
                                    for p in result["posts"]],
                          "tools_seen": (result["final"] or {}).get("tools_seen"),
                          "violation": result["violation"]}, indent=1))
    return 1 if failed else 0


if __name__ == "__main__":
    if sys.argv[1:] == ["--shim"]:
        shim()
    else:
        sys.exit(asyncio.run(main()))
