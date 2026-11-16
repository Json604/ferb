> Ported with ferb-link from the phone lab (ferb-phone-link@57b3f34 `relay/`), unchanged. Paths below are the lab's:
> `rust/src/…` is `engine/crates/ferb-link/src/…` here; the lab's `scripts/mac.sh` prompt is the `ferb-link serve`
> program (S4 makes it FERB.app's helper, S5 puts the setting in Settings → Phone). `sh tests/preflight_test.sh`
> runs the script's preflight checks offline.

# Your own relay

A relay is a small server that passes packets between the phone and the Mac when they cannot reach each other
directly. FERB works without one of your own, through n0's free shared relays, with caps on pictures and files.
With your own relay those caps are gone wherever the phone is. The phone is mostly used away from home, so this is
a main feature of the phone link, and it is still optional: nothing below is needed for the link to work.

`setup.sh` installs the official `iroh-relay` 1.4.1 release binary as a systemd service and writes its config
file. Nothing in it is FERB's own code.

## Is my own relay needed?

No. It is needed only for full-size pictures and files while the phone is away and no direct connection is found.

| | Without your own relay | With your own relay |
|---|---|---|
| Messages, questions, answers, Stop | always work | always work |
| Phone and Mac on the same Wi-Fi, or a direct connection found | everything at full size | everything at full size |
| Phone away, no direct connection (the link goes through a relay) | small pictures and files, within the caps of `CONTRACT.md` §5 and §7, through the free shared relay | no size caps |
| Who can use the relay | anyone (it is n0's public service; it also rate-limits) | only devices that hold your token |
| What you maintain | nothing | one small Linux server |

## The owner's questions

> what guarantee is that our own relay setup is adaptable to any VM, its the same on every VM, who decided the
> memory, the cpu, the connection and all, what if something we have mentioned not available there, is it
> needed??? who ensures safety of that VM??

- **"what guarantee is that our own relay setup is adaptable to any VM"**: none. The script has been run on
  exactly one machine (see [Tested on](#tested-on)). What stands in for a guarantee is the preflight: before it
  changes anything, the script checks each thing it relies on and stops with the reason and what to do. Run
  `sh setup.sh --check` on a machine to see the answer for that machine without installing anything.
- **"its the same on every VM"**: the relay is the same: one release binary, checked against a pinned SHA-256,
  and the same config file. The machines are not the same; what differs (init system, firewall, NAT, free ports,
  clock) is what the preflight looks at.
- **"who decided the memory, the cpu, the connection and all"**: the service's limits (`MemoryMax=256M`,
  `CPUWeight=50`, `Nice=5` in the systemd unit) were chosen by the script's author so that the relay stays a small,
  low-priority guest on a machine that may do other work. The amounts the preflight asks for (`MEM_NEED_MB`,
  `DISK_NEED_MB` at the top of `setup.sh`) come from the measurement under "What the server needs", with room on
  top. No connection limit is set: iroh-relay's rate limits are off unless a `limits` section is written
  (`iroh-relay-1.4.1/src/main.rs`, the `limits` field: "Disabled if not present"), and the script writes none.
  Who may connect is decided by the access token.
- **"what if something we have mentioned not available there"**: the preflight says which thing, and the install
  stops before changing anything. A system without systemd is refused; see [Without systemd](#without-systemd).
  The UDP discovery port is optional; see [What is optional](#what-is-optional).
- **"is it needed???"**: no; see the table above.
- **"who ensures safety of that VM??"**: you do. See [Who keeps the server safe](#who-keeps-the-server-safe).

## What the server needs

- **A Linux server with systemd, x86_64 or aarch64, that the phone can reach**: a public address, or a public
  address your provider maps onto it.
- **Memory: 64 MB free is asked for.** Measured 2026-11-10: the same relay code (iroh-relay 1.4.1, built into the
  lab's test relay, on this Mac, without TLS) used 9 MB idle, 9 MB with one phone and one Mac connected, and 9.4 MB
  at its highest while it carried a 300 MB file. The official Linux binary with TLS on a server was not measured;
  64 MB leaves room for that difference. The service is capped at 256 MB by its systemd unit.
- **CPU: one core is plenty.** In the same measurement the relay took 4 to 7 % of one core of this Mac while the
  file crossed it, and nothing when idle. Not measured on a small server's CPU.
- **Disk: 100 MB free is asked for.** The binary is 10 MB unpacked, the downloaded archive 4 MB; certificates and
  the config are a few kB.
- **Bandwidth: this is the real limit.** The relay forwards every byte both ways, so a transfer of N bytes costs
  about N in and N out on the server (a little more for framing). Chat and questions are a few kB. A live
  preview is the heavy case: a 200 kB picture five times a second is 1 MB/s in and 1 MB/s out. What you can move
  in a month is your server's data allowance divided by two; check that allowance with your provider.
- **Ports, a server of its own**: TCP 80 and 443 free and reachable (80 is for the Let's Encrypt certificate),
  a DNS name pointing at the server, UDP 7842 free.
- **Ports, `--shared`**: one TCP port of your choice (7443 by default) reachable from outside, and the next one
  (7444) free on 127.0.0.1 only. No DNS name. UDP 7842 is optional.
- **Outbound HTTPS to github.com** (the release binary) and, for a server of its own, to Let's Encrypt.
- **A clock that is right**, within 5 minutes: certificates are checked against it.
- **Commands**: `curl tar sha256sum install find mktemp awk sed seq useradd`, `ss` or `netstat`, and `openssl`
  (1.1.1 or newer) for `--shared`.

The release archive the script downloads is the `-unknown-linux-musl` build (see the URL in `install_binary`).
Rust builds for that target are statically linked by default, so the binary should not depend on the system's C
library and should run on glibc and musl systems alike. Seen running on Ubuntu 24.04 (glibc) only; the binary
was not inspected here and was not run on a musl system.

## What is optional

**The UDP discovery port (7842).** The relay itself works over its TCP/TLS port alone. UDP 7842 is iroh-relay's
"QUIC address discovery": a device asks the relay which public address its packets come from, which it needs to
try a direct connection with the other device.

- Without it, through this relay, a direct connection may not be found, so sessions stay on the relay more often
  (more of your server's bandwidth, and more delay). On the same Wi-Fi a direct connection is still found.
- Source: `iroh-1.4.1/src/net_report/probes.rs` (module comment: the QUIC address discovery probes are how an
  endpoint learns "our public IP addresses and ports"; the HTTPS probes are a fallback that measures only
  latency); the port's default, `iroh-relay-1.4.1/src/defaults.rs` (`DEFAULT_RELAY_QUIC_PORT = 7842`); it is off
  unless the config turns it on (`iroh-relay-1.4.1/src/main.rs`, `enable_quic_addr_discovery`, "Defaults to
  `false`"). Not verified here: whether a home router's port mapping (`iroh-1.4.1/src/portmapper.rs`) can supply
  the address instead, and how often a direct connection is actually found with and without the port.
- What the script does: both modes write `enable_quic_addr_discovery = true`. `--shared` also writes
  `quic_bind_addr = "[::]:7842"`; when UDP 7842 is already taken on the machine at first install, it leaves both
  lines out and says so. A server of its own relies on the default port; there the relay does not start when
  the port is taken (`iroh-relay-1.4.1/src/server.rs`, `QuicServer::spawn` failing fails the whole start; read,
  not run), so the preflight stops.
- FERB's Test on the Mac warns when discovery does not answer (`rust/src/relaytest.rs`).

**ufw rules.** In `--shared` mode, where `ufw` is active, the script adds allow rules for its two ports. It does
not do this for a server of its own, and it never changes firewalld, nftables or iptables: the preflight tells
you what to allow.

## Check a machine first

```sh
sh setup.sh --check --shared [--port 7443] [--address ADDRESS]
sh setup.sh --check relay.example.com
```
It changes nothing and does not need root (the lines that need root are marked `skipped`). The same checks run
at the start of every install and `--update`. Each prints one line:

| Line | Meaning |
|---|---|
| `ok` | fine |
| `problem` | a required thing is missing; the line says what to do. The install stops before changing anything. |
| `warning` | it will work, with a limit the line names |
| `skipped` | could not be checked, with the reason |
| `note` | a fact you must act on yourself |

The checks, in order:

1. **system**: Linux, and which distribution and version (`/etc/os-release`). Only Ubuntu 24.04 is reported as
   run for real; any other is a note, not a stop.
2. **CPU**: x86_64 or aarch64, and a pinned checksum for the version (only 1.4.1 is pinned; another version
   needs `IROH_RELAY_SHA256`).
3. **init system**: systemd present and running. Otherwise a stop that points to [Without systemd](#without-systemd).
4. **root**: required for the install; `--check` runs without it.
5. **commands**: the list above; for `--shared`, also that openssl is 1.1.1 or newer.
6. **disk**: free space where `/opt/ferb-relay` will live, against `DISK_NEED_MB`.
7. **memory**: `MemAvailable` against `MEM_NEED_MB`.
8. **TCP ports**: the relay's port and the next one (`--shared`), or 80 and 443, are free on this machine.
9. **UDP 7842**: free or not. Taken is a warning for `--shared` (discovery stays off) and a stop for a server
   of its own.
10. **firewall**: firewalld, ufw, nftables or iptables rules, or none, and what the script will or will not do
    about it. Always followed by a note: a firewall outside the machine (a cloud provider's) cannot be seen from
    inside and must allow the ports.
11. **address**: the address this machine sends from (read from its routing table; nothing is sent, and no
    "what is my address" service is asked). A private address means the machine is behind NAT: `--shared`
    without `--address` stops and asks for the public one. For a server of its own, also whether the name
    resolves and to this machine.
12. **outbound HTTPS**: github.com answers.
13. **clock**: this machine's time against the `Date` header of that answer; more than 5 minutes off is a stop.

For `--update` only checks 1 to 6, 12 and 13 run. What the checks cannot tell: whether the machine is reachable
from outside. FERB's Test on the Mac tells that.

## Set it up (about two minutes)

**A server of its own** (ports 80 and 443 free, a DNS name pointing at it):
```sh
scp relay/setup.sh root@relay.example.com:
ssh root@relay.example.com sh setup.sh relay.example.com you@example.com
```
**A machine that already runs other things** (a web server on 80/443; no DNS name needed):
```sh
scp relay/setup.sh root@your-server:
ssh root@your-server sh setup.sh --shared
```
Shared mode touches nothing that is there: the relay listens on TCP 7443 with a certificate from its own authority
(made on the spot; FERB pins it, so only this relay is accepted at that address) and on UDP 7842 for address
discovery. Where `ufw` is active it adds those two rules and no other. If your provider has a firewall outside
the machine, allow TCP 7443 and UDP 7842 there.

Either way everything is in `/opt/ferb-relay`, runs as the service `ferb-relay` under its own user, and ends by
printing one line to paste on the Mac:

```
ferbrelay://HOST[:7443]?tok=<token>[&ca=<certificate authority>]
```

## Tested on

| System | Mode | How tested |
|---|---|---|
| Ubuntu 24.04, x86_64 or aarch64 not recorded here, 1 vCPU, 1 GB RAM | `--shared` | run for real (one machine, before the preflight was added) |
| Ubuntu 24.04 | `--shared`, with the preflight | preflight logic run with stubbed commands on macOS; the changed script has not been run on a server |
| Ubuntu 24.04 | a server of its own (Let's Encrypt) | not tested: this mode has never been run |
| Debian 12 (as a made-up `/etc/os-release`) | `--shared` | preflight logic run with stubbed commands on macOS |
| Any other systemd distribution (Debian, Fedora, RHEL family, Arch, openSUSE) | both | not tested |
| A system without systemd (Alpine with OpenRC, a container) | refusal | preflight logic run with stubbed commands on macOS |
| A system without systemd | the foreground process of [Without systemd](#without-systemd) | not tested |
| firewalld, nftables or iptables machines | both | preflight logic run with stubbed commands on macOS (the wording of the answer only) |
| A machine behind NAT, a busy port, low memory, low disk, a wrong clock, an unsupported CPU, a missing command | both | preflight logic run with stubbed commands on macOS |

"Stubbed" means `relay/tests/preflight_test.sh`: the script's checks run under `sh` and `dash` on macOS against
made-up answers from `uname`, `ss`, `curl` and the rest. It shows that the checks decide and word things as
written above. It does not show that any real system answers the way the stubs do, and it runs no install.
`shellcheck` was not available where this was written.

## Without systemd

The script refuses a system where systemd is not running (Alpine with OpenRC, most containers) and installs
nothing. There is no OpenRC or other service file here, because none has been tested. What works in principle is
the same official binary as a plain foreground process under whatever keeps services running on that system.
**None of the steps below has been run.**

1. Print the config file the script would have written (works anywhere `sh` does; no root):
   ```sh
   sh setup.sh --print-config --shared [--port 7443] > config.toml
   sh setup.sh --print-config relay.example.com you@example.com > config.toml
   ```
   It contains `<token>` and the paths under `/opt/ferb-relay`; change both to yours.
2. The binary: download the archive from the URL in `install_binary` in `setup.sh`, and compare its
   `sha256sum` with the value in `pinned_sha` before unpacking it.
3. A token: `head -c 32 /dev/urandom | openssl base64 -A | tr '+/' '-_' | tr -d '=' | cut -c1-40`.
4. For `--shared`, the certificate: the six `openssl` commands in the `--shared` part of `setup.sh` (under "The
   relay's own certificate authority"), with `SAN=IP:<your address>` or `SAN=DNS:<your name>`. Keep `ca.pem`,
   `default.crt` and `default.key`; delete `ca.key`.
5. Run it as a user of its own, never as root, under your supervisor:
   ```sh
   /opt/ferb-relay/iroh-relay --config-path /opt/ferb-relay/config.toml
   ```
   Ports below 1024 (a server of its own) need the right to bind them.
6. The line for FERB: `ferbrelay://ADDRESS:PORT?tok=TOKEN&ca=CA`, where CA is
   `openssl x509 -in ca.pem -outform DER | openssl base64 -A | tr '+/' '-_' | tr -d '='`
   (for a server of its own: `ferbrelay://NAME?tok=TOKEN`).

## Turn it on in FERB
On the Mac (in this lab: the `scripts/mac.sh` prompt): `relay ferbrelay://…` (the whole line).
FERB tests the relay first and saves it only if the test passes. If it fails it says which step failed:

| Code | Meaning | Fix |
|---|---|---|
| `own_relay_bad_url` | not a relay link or an `https://` address | paste the whole line the setup printed |
| `own_relay_dns` | the name does not resolve | fix the DNS record |
| `own_relay_unreachable` | nothing answers on the port | start the service, open the firewall |
| `own_relay_tls` | the certificate is not accepted | Let's Encrypt needs 80/443 open; a `--shared` relay needs the whole link, with its `ca` |
| `own_relay_not_a_relay` | something else answers there | the address must be the relay itself |
| `own_relay_access_denied` | the token is wrong or missing | paste the line again |
| `own_relay_version` | the relay is a different major iroh version | `sh setup.sh --update <version>` |

A passing test may still warn that address discovery (UDP 7842) does not answer: everything works through the relay
then, but a direct connection is only found on the same Wi-Fi.

Paired phones learn the relay from the Mac (over the link, or from the pairing code): nothing is typed on the
phone.

## When the relay is down
After 30 s without it, FERB and the phone fall back to the shared relays, say so (state `own_fallback`, with the
reason) and apply the shared-relay caps. FERB tests your relay once a minute (`own_relay_probe_s = 60`) and moves
back when it answers.
`own_relay_fallback = false` in the limits keeps FERB off the shared relays entirely; then the phone shows
`own_relay_unreachable` until the relay is back.

## Who keeps the server safe

**You do.** The server is yours to maintain: its operating system's updates, who may log in over ssh and how
(keys, no password login), its firewall, and whatever else runs on it. The script does none of that, does not
update the relay by itself, and FERB does not watch the server. What the script does is keep the relay small
and boxed in, and FERB is built so that the relay is not trusted with anything.

### What the setup does to limit harm

Each line is what `setup.sh` writes; none of it was observed on a server by the person writing this.

- **Its own unprivileged user**, `ferb-relay`, a system account with no login shell (`useradd --system
  --shell /usr/sbin/nologin`, in `make_user_and_token`). The service runs as it (`User=`, `Group=` in the unit).
- **A boxed-in service** (`write_unit`): `NoNewPrivileges=true`, `ProtectSystem=strict` (the system's files are
  read-only to it), `ProtectHome=true`, `PrivateTmp=true`, and one writable folder, `/opt/ferb-relay/certs`. For
  a server of its own it gets one capability, binding ports below 1024 (`CAP_NET_BIND_SERVICE`), and no other.
- **A memory cap and low priority**: `MemoryMax=256M`, `CPUWeight=50`, `Nice=5`. Not verified here that the cap
  takes effect on a given system (`systemctl show ferb-relay -p MemoryMax -p MemoryCurrent` shows it).
- **An access token**: 40 characters from `/dev/urandom`, in `/opt/ferb-relay/token` and in the config file,
  both readable only by root and the service's group (`lock_down`, mode 0640). Only a device that presents it
  may relay (`access.shared_token`; `iroh-relay-1.4.1/src/main.rs`, `SharedTokenAccess`: any other connection is
  denied).
- **A pinned checksum**: the downloaded archive must match the SHA-256 written in the script for 1.4.1
  (`pinned_sha`), or nothing is installed. Another version needs its checksum passed in `IROH_RELAY_SHA256`.
- **Ports it opens.** `--shared`: the TCP port you chose on all addresses, the next port on 127.0.0.1 only, and
  UDP 7842. A server of its own: TCP 80 and 443, UDP 7842. The metrics port is off (`enable_metrics = false`).
- **`--shared`: the certificate authority's key is deleted** after it signs the relay's one certificate.
- **What stays open to anyone**, token or not: `/healthz` (it tells the relay's version;
  `iroh-relay-1.4.1/src/server.rs`, `healthz_handler`), the probe paths `/ping` and `/generate_204`, and address
  discovery on UDP 7842. The config comment says the access setting covers relaying only
  (`iroh-relay-1.4.1/src/main.rs`, the `access` field). I inferred from that comment that the discovery port
  takes no token; not verified here.

### If someone takes over the server

**They cannot:**

- **Read or alter what FERB and the phone send.** The phone and the Mac speak QUIC with TLS 1.3 to each other,
  end to end. The side that connects checks that the other's key is the link id it dialled
  (`iroh-1.4.1/src/tls/verifier.rs`, `verify_server_cert`: the presented public key must equal the expected
  endpoint id). The side that accepts requires the caller to prove a key in the handshake
  (`verify_client_cert` in the same file accepts any key whose handshake signature is valid), and that key is the
  caller's link id, which FERB then compares with its paired devices. The relay holds neither key. What it
  forwards is the QUIC packets as bytes: the sender wraps each outgoing QUIC transmit as it is
  (`iroh-1.4.1/src/socket/transports/relay.rs`, `datagrams_from_transmit`), the frame carries them as an
  uninterpreted byte field (`iroh-relay-1.4.1/src/protos/relay.rs`, `Datagrams.contents`), and the server hands
  them to the destination without looking inside (`iroh-relay-1.4.1/src/server/client.rs`,
  `handle_frame_send_packet`; `src/server/clients.rs`, `send_packet`). An altered packet fails QUIC's own
  integrity check at the receiver: that is how QUIC works; not verified here in noq's source.
- **Connect to FERB or pose as the phone.** A device proves its link id to the relay and to the other side with
  its secret key (`iroh-relay-1.4.1/src/protos/handshake.rs`, module comment: the client signs the relay's
  challenge or TLS keying material). The server has no such key. FERB accepts only paired link ids past the
  first frame (`SECURITY.md`, tested by the `pairing` and `link` suites; not rerun for this file).
- **Get anything FERB stores**: no file, message, secret or key of FERB's is on the server. The only FERB-related
  secrets there are the relay's own token and its certificate key.

**They can:**

- See which link ids talk to each other, when, how much, and from which IP addresses.
- Drop or slow the link.
- Read the token, and so let other devices use the relay, and spend the server's bandwidth.
- Use the server itself for anything else. If other things run on it (`--shared`), those are exposed too. That is
  the server's own safety, and this setup neither helps nor hurts it beyond the items above.

**What happens then.** When the relay stops answering, the link falls back to the shared relays after 30 s with
their caps, and both sides say why (`own_fallback`; default `own_relay_fallback_after_s = 30` in
`rust/src/limits.rs`; the `relay` suite asserts the state). A relay that only slows the link down is not
detected as such: not verified here. To cut a taken-over relay off, turn "Use my own relay" off in FERB, then
rebuild the server; a new install makes a new token and, for `--shared`, a new authority.

### Keeping it current

- Status: `sh setup.sh --status`. Logs: `journalctl -u ferb-relay -f`.
- Update: `IROH_RELAY_SHA256=<sha256 of the release archive> sh setup.sh --update 1.4.2`. The checksum is needed
  for every version except 1.4.1, which is pinned. The address, the token and the certificate stay.
- The Mac reports a relay it cannot use: FERB's Test fails with `own_relay_version` when the relay's `/healthz`
  names another major version, and with `own_relay_unreachable` when nothing answers (`rust/src/relaytest.rs`).
  While in use, a relay that stops answering shows as `own_fallback`. Not verified here: whether a version
  mismatch is also reported outside the Test.
- Change the token: delete `/opt/ferb-relay/token`, run the setup again, paste the new line in FERB (phones follow).
- Remove everything it installed: `sh setup.sh --uninstall` (the service, its user, the folder, its firewall rules).

## What the config means (iroh-relay 1.4.1, `src/main.rs`)
A server of its own (`sh setup.sh --print-config NAME EMAIL` prints it; `--print-config --shared` the other):
```toml
http_bind_addr = "[::]:80"                 # plain HTTP: the certificate challenge and captive-portal check
enable_metrics = false                     # no metrics port (it would listen on 9090)
access.shared_token = ["<token>"]          # only devices presenting this token may relay
enable_quic_addr_discovery = true          # address discovery on UDP 7842

[tls]
https_bind_addr = "[::]:443"
hostname = "relay.example.com"
cert_mode = "LetsEncrypt"
cert_dir = "/opt/ferb-relay/certs"
contact = "you@example.com"
```

n0 also publishes a Docker image (`n0computer/iroh-relay`); this guide does not use it because its entry point
could not be checked here.

## Try it on this Mac first
`dev-relay --port 3340 --token test` (built by `scripts/build.sh`) is the same relay code over plain http on
127.0.0.1, with a token. `ferb-link serve` accepts `relay http://localhost:3340 test`.

The preflight's tests need no server: `sh relay/tests/preflight_test.sh`.
