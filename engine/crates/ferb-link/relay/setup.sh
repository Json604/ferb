#!/bin/sh
# Sets up your own iroh relay for FERB on a Linux server with systemd. Run it on the server, as root.
#
#   A server of its own (ports 80 and 443 free, a DNS name pointing at it):
#       sh setup.sh relay.example.com you@example.com
#   A shared machine that already runs other things (a web server on 80/443, no DNS name needed):
#       sh setup.sh --shared [--port 7443] [--address ADDRESS]
#   Only check whether this machine can run it (changes nothing; root not needed):
#       sh setup.sh --check [the same arguments as above]
#   Only print the config file it would write (for a system without systemd; changes nothing):
#       sh setup.sh --print-config [the same arguments as above]
#   Later:
#       sh setup.sh --status | --update VERSION | --uninstall
#
# Before it changes anything it checks what it relies on (the preflight below) and stops with the reason when a
# required thing is missing.
#
# Everything it installs is one folder (/opt/ferb-relay), one service (ferb-relay) running as its own
# unprivileged user (ferb-relay), and, where ufw is active, in --shared mode, firewall rules for the relay's ports.
# It never touches another service, a web server's configuration, or existing firewall rules. --uninstall removes
# all of it.
#
# It installs the official iroh-relay release binary and writes its config file; nothing here is FERB's own code.
set -eu

VERSION="${IROH_RELAY_VERSION:-1.4.1}"        # the iroh release FERB is built on (same major version at least)
DIR=/opt/ferb-relay
UNIT=/etc/systemd/system/ferb-relay.service
SVC=ferb-relay
RELEASE_HOST=github.com                       # where the release binary is downloaded from
QUIC_PORT=7842                                # iroh-relay's address discovery port (UDP), the one FERB's side expects

# What the preflight asks for before installing. These two come from measurement of a running relay.
# From measurement (relay/README.md, "What the server needs"): the relay code used under 10 MB of memory idle and
# while carrying a 300 MB file; the binary is 10 MB. The values below leave room on top.
MEM_NEED_MB=64                                # free memory (MemAvailable) the relay needs
DISK_NEED_MB=100                              # free disk where $DIR will live (binary, certificates, download)
CLOCK_TOLERANCE_S=300                         # how far this machine's clock may be from the release host's

# The preflight reads the system's own files below this folder. Empty on a real machine; only the tests set it.
PREFLIGHT_ROOT="${FERB_RELAY_PREFLIGHT_ROOT:-}"

die() { echo "setup: $*" >&2; exit 1; }
say() { echo "setup: $*"; }
have() { command -v "$1" >/dev/null 2>&1; }

# listening tcp|udp PORT: 0 when something listens on that port here, 1 when not, 2 when it cannot be told.
listening() {
  if have ss; then
    case "$1" in tcp) l_out=$(ss -tlnH 2>/dev/null) || return 2 ;; *) l_out=$(ss -ulnH 2>/dev/null) || return 2 ;; esac
  elif have netstat; then
    case "$1" in tcp) l_out=$(netstat -tln 2>/dev/null) || return 2 ;; *) l_out=$(netstat -uln 2>/dev/null) || return 2 ;; esac
  else
    return 2
  fi
  echo "$l_out" | awk '{print $4}' | grep -Eq "[:.]$2\$"
}
ufw_on() { command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -q "^Status: active"; }
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }

cpu_arch() {   # the release's name for this CPU, or nothing when there is no release binary for it
  case "$(uname -m)" in
    x86_64|amd64) echo x86_64 ;;
    aarch64|arm64) echo aarch64 ;;
  esac
}

# The SHA-256 of the release archive, pinned for 1.4.1 (recorded 2026-11-10 from the release page over https);
# for another version pass IROH_RELAY_SHA256.
pinned_sha() {   # $1 version, $2 arch
  case "$1-$2" in
    1.4.1-x86_64) echo 75254641e3fad7fcbac6dee4d485556fbe06ef708340a9d8810469270ba626b8 ;;
    1.4.1-aarch64) echo 3fc0c2d67c1cd66250bf82f3e2f17a985b7081a3d272186bad9d63e3f2347b38 ;;
    *) echo "${IROH_RELAY_SHA256:-}" ;;
  esac
}

# the address this machine reaches the internet from (a look in the routing table; nothing is sent)
route_src() {
  ip -4 route get 1.1.1.1 2>/dev/null | awk '{for (i = 1; i < NF; i++) if ($i == "src") print $(i + 1)}' | head -1
}

is_private() {   # an IPv4 address that is not reachable from the internet as it is
  case "$1" in
    10.*|192.168.*|127.*|169.254.*|172.1[6-9].*|172.2[0-9].*|172.3[01].*) return 0 ;;
    100.6[4-9].*|100.[7-9][0-9].*|100.1[01][0-9].*|100.12[0-7].*) return 0 ;;
  esac
  return 1
}

# ---- the preflight: every assumption checked before anything is changed --------------------------------------
# One line per check. "problem" is a required thing that is missing: the install stops. "warning" is something
# that works but is worth knowing. "skipped" could not be checked (says why). "note" is a fact to act on.
P_BAD=0; P_WARN=0; P_SKIP=0
p_line() { printf '  %-8s %s\n' "$1" "$2"; }
p_ok() { p_line ok "$1"; }
p_bad() { P_BAD=$((P_BAD + 1)); p_line problem "$1"; }
p_warn() { P_WARN=$((P_WARN + 1)); p_line warning "$1"; }
p_skip() { P_SKIP=$((P_SKIP + 1)); p_line skipped "$1"; }
p_note() { p_line note "$1"; }

os_field() { sed -n "s/^$1=//p" "$PREFLIGHT_ROOT/etc/os-release" 2>/dev/null | head -1 | tr -d '"'; }

check_os() {
  kernel=$(uname -s 2>/dev/null || echo unknown)
  if [ "$kernel" != Linux ]; then
    p_bad "system: this is $kernel, not Linux. The relay binary this script installs is the Linux one: use a Linux server."
    return 0
  fi
  if [ ! -r "$PREFLIGHT_ROOT/etc/os-release" ]; then
    p_warn "system: Linux, but /etc/os-release is missing, so the distribution is unknown. This script has been run on Ubuntu 24.04 only; the checks below decide whether it can work here."
    return 0
  fi
  os_name=$(os_field PRETTY_NAME)
  [ -n "$os_name" ] || os_name="$(os_field ID) $(os_field VERSION_ID)"
  if [ "$(os_field ID)" = ubuntu ] && [ "$(os_field VERSION_ID)" = 24.04 ]; then
    p_ok "system: $os_name (the one system this script has been run on for real)"
  else
    p_note "system: $os_name. This script has not been run on it (only on Ubuntu 24.04); the checks below decide whether it can work here."
  fi
}

check_arch() {   # $1: the iroh-relay version to install
  arch=$(cpu_arch)
  if [ -z "$arch" ]; then
    p_bad "CPU: $(uname -m). This script knows the release binary only for x86_64 and aarch64: use a server with one of those."
  elif [ -z "$(pinned_sha "$1" "$arch")" ]; then
    p_bad "CPU: $arch is fine, but no checksum is known for iroh-relay $1 (only 1.4.1 is pinned). Pass IROH_RELAY_SHA256=<sha256 of the release archive> after checking it on the release page."
  else
    p_ok "CPU: $arch, iroh-relay $1 (the archive's checksum is known and will be checked)"
  fi
}

check_init() {
  if have systemctl && [ -d "$PREFLIGHT_ROOT/run/systemd/system" ]; then
    p_ok "init system: systemd is running"
    return 0
  fi
  if have systemctl; then
    why="systemd is installed but did not start this system (usual inside a container)"
  else
    why="there is no systemd here (no systemctl)"
  fi
  pid1=$(cat "$PREFLIGHT_ROOT/proc/1/comm" 2>/dev/null || true)
  p_bad "init system: $why${pid1:+; process 1 is $pid1}. This script installs a systemd service and nothing else, so it stops here. The same relay can run as a plain foreground process under whatever keeps services running on this system: see \"Without systemd\" in relay/README.md and 'sh setup.sh --print-config'."
}

check_root() {
  if [ "$(id -u)" = 0 ]; then
    p_ok "root: running as root"
  elif [ "$CHECK" = 1 ]; then
    p_skip "root: not root. This check runs anyway; the install itself must be run as root, and the lines marked skipped below need root too."
  else
    p_bad "root: not root. Run this as root (it adds a user, a folder under /opt and a systemd service)."
  fi
}

check_commands() {
  need="curl tar sha256sum install find mktemp awk sed"
  [ "$MODE" = update ] || need="$need seq useradd"
  [ "$MODE" != shared ] || need="$need openssl"
  missing=""
  for c in $need; do have "$c" || missing="$missing $c"; done
  if [ "$MODE" != update ] && ! have ss && ! have netstat; then missing="$missing ss"; fi
  if [ -n "$missing" ]; then
    p_bad "commands: missing:$missing. Install them and run this again (Debian/Ubuntu packages: curl tar coreutils findutils openssl iproute2 passwd; Fedora/RHEL: curl tar coreutils findutils openssl iproute shadow-utils)."
  else
    p_ok "commands: all there ($need$([ "$MODE" = update ] || echo ', ss or netstat'))"
  fi
  if [ "$MODE" = shared ] && have openssl; then
    ov=$(openssl version 2>/dev/null || true)
    case "$ov" in
      "OpenSSL 0."*|"OpenSSL 1.0."*|"OpenSSL 1.1.0"*)
        p_bad "openssl: $ov is too old to make the relay's certificate (the script uses -addext, which needs OpenSSL 1.1.1 or newer). Update openssl." ;;
      "OpenSSL "*) p_ok "openssl: $ov" ;;
      *) p_warn "openssl: '$ov' is not OpenSSL; making the certificate has only been run with OpenSSL 3. If it fails, install OpenSSL." ;;
    esac
  fi
}

check_memory() {
  kb=$(awk '$1 == "MemAvailable:" {print $2}' "$PREFLIGHT_ROOT/proc/meminfo" 2>/dev/null || true)
  if [ -z "$kb" ]; then
    p_warn "memory: MemAvailable could not be read from /proc/meminfo, so free memory was not checked (needed: $MEM_NEED_MB MB)."
  elif [ $((kb / 1024)) -lt "$MEM_NEED_MB" ]; then
    p_bad "memory: $((kb / 1024)) MB free, $MEM_NEED_MB MB needed. Stop something else on this machine or use one with more memory."
  else
    p_ok "memory: $((kb / 1024)) MB free ($MEM_NEED_MB MB needed)"
  fi
}

check_disk() {
  d=$DIR
  while [ ! -d "$d" ]; do d=$(dirname "$d"); done
  kb=$(df -Pk "$d" 2>/dev/null | awk 'NR == 2 {print $4}')
  case "$kb" in
    ''|*[!0-9]*) p_warn "disk: free space under $d could not be read (df), so it was not checked (needed: $DISK_NEED_MB MB)." ;;
    *)
      if [ $((kb / 1024)) -lt "$DISK_NEED_MB" ]; then
        p_bad "disk: $((kb / 1024)) MB free under $d, $DISK_NEED_MB MB needed. Free some space."
      else
        p_ok "disk: $((kb / 1024)) MB free under $d ($DISK_NEED_MB MB needed)"
      fi ;;
  esac
}

check_tcp() {   # $1 port, $2 what it is for, $3 what to do when taken
  rc=0; listening tcp "$1" || rc=$?
  case "$rc" in
    0) p_bad "TCP port $1 ($2): already in use on this machine. $3" ;;
    1) p_ok "TCP port $1 ($2): free" ;;
    *) p_skip "TCP port $1 ($2): cannot tell whether it is free (neither ss nor netstat works here)" ;;
  esac
}

check_ports() {
  if [ "$MODE" = shared ]; then
    if [ "$(cat "$DIR/port" 2>/dev/null)" = "$PORT" ]; then
      p_ok "TCP port $PORT: the relay is already installed on it; left as it is"
    else
      check_tcp "$PORT" "the relay" "Choose another with --port."
      check_tcp "$PLAIN" "the relay's plain-HTTP side, on 127.0.0.1 only" "It is always the relay's port plus one: choose another --port."
    fi
    if [ -f "$DIR/port" ]; then
      p_ok "UDP port $QUIC_PORT (address discovery): the relay is already installed; left as it is"
      return 0
    fi
    rc=0; listening udp "$QUIC_PORT" || rc=$?
    case "$rc" in
      0) p_warn "UDP port $QUIC_PORT (address discovery, optional): in use on this machine, so the install leaves address discovery off. The relay itself works without it; through this relay a direct connection will then only be found on the same Wi-Fi." ;;
      1) p_ok "UDP port $QUIC_PORT (address discovery, optional): free; the install turns address discovery on" ;;
      *) p_skip "UDP port $QUIC_PORT (address discovery, optional): cannot tell whether it is free" ;;
    esac
    return 0
  fi
  if [ -f "$DIR/mode" ]; then
    p_ok "ports 80, 443 and UDP $QUIC_PORT: the relay is already installed; left as they are"
    return 0
  fi
  for p in 80 443; do
    check_tcp "$p" "needed for the Let's Encrypt certificate and the relay" "Another web server? Use: sh setup.sh --shared"
  done
  # In this mode the config turns address discovery on; iroh-relay 1.4.1 does not start when it cannot bind the
  # port (read in its src/server.rs, not run).
  rc=0; listening udp "$QUIC_PORT" || rc=$?
  case "$rc" in
    0) p_bad "UDP port $QUIC_PORT (address discovery): in use on this machine. In this mode the relay is set up with address discovery on and would not start. Free the port, or use: sh setup.sh --shared (it leaves discovery off when the port is taken)." ;;
    1) p_ok "UDP port $QUIC_PORT (address discovery): free" ;;
    *) p_skip "UDP port $QUIC_PORT (address discovery): cannot tell whether it is free" ;;
  esac
}

check_firewall() {
  if [ "$MODE" = shared ]; then wanted="TCP $PORT and UDP $QUIC_PORT"; else wanted="TCP 80, TCP 443 and UDP $QUIC_PORT"; fi
  if have systemctl && systemctl is-active --quiet firewalld 2>/dev/null; then
    if [ "$MODE" = shared ]; then fw_ports="--add-port=$PORT/tcp --add-port=$QUIC_PORT/udp"; else fw_ports="--add-port=80/tcp --add-port=443/tcp --add-port=$QUIC_PORT/udp"; fi
    p_warn "firewall: firewalld is active. This script does not change firewalld: allow $wanted yourself (firewall-cmd --permanent $fw_ports; firewall-cmd --reload)."
  elif [ "$(id -u)" != 0 ]; then
    p_skip "firewall: reading this machine's firewall rules (ufw, nftables, iptables) needs root"
  elif ufw_on; then
    if [ "$MODE" = shared ]; then
      p_ok "firewall: ufw is active; the install adds allow rules for $wanted and no other (--uninstall removes them)"
    else
      p_warn "firewall: ufw is active. In this mode the script adds no ufw rules: allow $wanted yourself (ufw allow 80/tcp; ufw allow 443/tcp; ufw allow $QUIC_PORT/udp)."
    fi
  elif have nft && nft list ruleset 2>/dev/null | grep -q .; then
    p_warn "firewall: nftables rules are present (not through ufw or firewalld). This script does not read or change them: make sure they accept $wanted."
  elif have iptables && iptables -S 2>/dev/null | grep -qv '^-P'; then
    p_warn "firewall: iptables rules are present (not through ufw or firewalld). This script does not read or change them: make sure they accept $wanted."
  else
    p_ok "firewall: no firewall rules found on this machine (looked for firewalld, ufw, nftables, iptables)"
  fi
  p_note "provider firewall: a firewall outside this machine (a cloud provider's security group or network firewall) cannot be seen from here. It must allow $wanted from anywhere."
}

check_address() {
  src=$(route_src)
  if [ "$MODE" = shared ]; then
    if [ -n "$ADDRESS" ]; then
      case "$ADDRESS" in
        *[!0-9.]*) p_ok "address: $ADDRESS (a name, given with --address). Whether it leads to this machine from outside cannot be checked from here; FERB's Test on the Mac does that." ;;
        *)
          if is_private "$ADDRESS"; then
            p_warn "address: $ADDRESS (given with --address) is a private address: the phone reaches this relay only from inside that network, not from mobile data."
          else
            p_ok "address: $ADDRESS (given with --address). Whether it leads to this machine from outside cannot be checked from here; FERB's Test on the Mac does that."
          fi ;;
      esac
    elif [ -z "$src" ]; then
      p_bad "address: this machine's IPv4 address could not be found (no 'ip' command, or no IPv4 route to the internet). Pass --address with the name or address the phone will use to reach this machine."
    elif is_private "$src"; then
      p_bad "address: this machine's own address is $src, a private one: it is behind NAT (many cloud providers map a public address onto it). The relay link must carry the public address: pass --address PUBLIC_ADDRESS, and make sure the ports are forwarded to this machine. (For a relay used only inside this network, pass --address $src.)"
    else
      p_ok "address: $src is this machine's own address and is a public one (no NAT seen from here). Whether it is reachable from outside cannot be checked from here; FERB's Test on the Mac does that."
    fi
    return 0
  fi
  if [ -z "$src" ]; then
    p_warn "address: this machine's IPv4 address could not be found (no 'ip' command, or no IPv4 route), so it could not be compared with $HOST."
  elif is_private "$src"; then
    p_warn "address: this machine's own address is $src, a private one: it is behind NAT. Ports 80, 443 and UDP $QUIC_PORT must be forwarded to it from the public address $HOST points at."
  else
    p_ok "address: $src is this machine's own address and is a public one (no NAT seen from here)"
  fi
  if ! have getent; then
    p_skip "name: $HOST could not be looked up (no getent command). Let's Encrypt needs it to point at this server."
    return 0
  fi
  named=$(getent ahostsv4 "$HOST" 2>/dev/null | awk '{print $1}' | sort -u | tr '\n' ' ')
  [ -n "$named" ] || named=$(getent hosts "$HOST" 2>/dev/null | awk '{print $1}' | sort -u | tr '\n' ' ')
  named=${named% }
  if [ -z "$named" ]; then
    p_warn "name: $HOST does not resolve from this machine. Let's Encrypt gives no certificate until the DNS record points at this server."
  elif [ -n "$src" ] && ! is_private "$src" && ! echo " $named " | grep -Fq " $src "; then
    p_warn "name: $HOST resolves to $named, and this machine's address is $src. Let's Encrypt gives no certificate unless the name leads here: fix the DNS record (ignore this if another address is mapped onto this machine)."
  else
    p_ok "name: $HOST resolves to $named"
  fi
}

# The Date header of an HTTP answer on stdin, as seconds since 1970 (nothing when there is none).
http_date_epoch() {
  awk '
    BEGIN { split("Jan Feb Mar Apr May Jun Jul Aug Sep Oct Nov Dec", mn, " "); for (i = 1; i <= 12; i++) mon[mn[i]] = i }
    tolower($1) == "date:" && !done {
      gsub(/\r/, ""); d = $3 + 0; m = mon[$4]; y = $5 + 0; split($6, t, ":")
      if (!m || y < 1970) next
      if (m <= 2) { y -= 1; m += 12 }
      days = 365 * y + int(y / 4) - int(y / 100) + int(y / 400) + int((153 * (m - 3) + 2) / 5) + d - 719469
      printf "%.0f\n", days * 86400 + t[1] * 3600 + t[2] * 60 + t[3]; done = 1
    }'
}

check_network() {
  if ! have curl; then
    p_skip "outbound HTTPS and clock: not checked (curl is missing, see above)"
    return 0
  fi
  if ! hdr=$(curl -sSI --max-time 15 "https://$RELEASE_HOST/" 2>&1); then
    p_bad "outbound HTTPS: https://$RELEASE_HOST does not answer from here ($(echo "$hdr" | tail -1)). The relay binary is downloaded from there. Check DNS, an outbound firewall or a proxy; if the message is about a certificate, check the clock (date -u) and the ca-certificates package."
    p_skip "clock: not compared (it needs the answer above)"
    return 0
  fi
  p_ok "outbound HTTPS: $RELEASE_HOST answers (the download itself also follows a redirect to its file host; a failed download stops the install before anything is installed)"
  there=$(echo "$hdr" | http_date_epoch)
  here=$(date -u +%s 2>/dev/null || true)
  case "$there-$here" in
    -*|*-|*[!0-9-]*) p_warn "clock: could not be compared with $RELEASE_HOST's (no readable Date in its answer). Compare by hand: date -u"; return 0 ;;
  esac
  off=$((here - there))
  if [ "$off" -lt 0 ]; then off=$((0 - off)); side=behind; else side=ahead; fi
  if [ "$off" -le "$CLOCK_TOLERANCE_S" ]; then
    p_ok "clock: within $off s of $RELEASE_HOST's (up to $CLOCK_TOLERANCE_S s is accepted)"
  else
    p_bad "clock: this machine's clock is $off s (about $((off / 60)) min) $side $RELEASE_HOST's. Certificates are checked against the clock: one made or requested with a wrong clock is refused by the Mac and the phone. Turn on time sync (timedatectl set-ntp true, or install chrony), then run this again."
  fi
}

preflight() {
  case "$MODE" in
    shared) say "checking this machine for --shared on port $PORT (nothing is changed by these checks)" ;;
    dedicated) say "checking this machine for a relay of its own at $HOST (nothing is changed by these checks)" ;;
    update) say "checking this machine for an update to iroh-relay $NEWVER (nothing is changed by these checks)" ;;
  esac
  check_os
  if [ "$MODE" = update ]; then check_arch "$NEWVER"; else check_arch "$VERSION"; fi
  check_init
  check_root
  check_commands
  check_disk
  if [ "$MODE" != update ]; then      # an update replaces a relay that is already running: same memory, same ports
    check_memory
    check_ports
    check_firewall
    check_address
  fi
  check_network
  if [ "$P_BAD" -gt 0 ]; then
    say "$P_BAD problem(s), $P_WARN warning(s), $P_SKIP skipped. Nothing was changed. Fix the lines marked 'problem' and run this again."
    exit 1
  fi
  say "no problems ($P_WARN warning(s), $P_SKIP skipped)."
}

install_binary() {
  arch=$(cpu_arch)
  [ -n "$arch" ] || die "unsupported CPU $(uname -m)"
  url="https://$RELEASE_HOST/n0-computer/iroh/releases/download/v$1/iroh-relay-v$1-$arch-unknown-linux-musl.tar.gz"
  tmp=$(mktemp -d)
  say "downloading $url"
  curl -fsSL "$url" -o "$tmp/relay.tar.gz" || die "download failed (is $1 a released version?)"
  # The archive must be the one this script was written for.
  want=$(pinned_sha "$1" "$arch")
  [ -n "$want" ] || die "no checksum known for iroh-relay $1 on $arch: pass IROH_RELAY_SHA256=<sha256 of the release archive>"
  got=$(sha256sum "$tmp/relay.tar.gz" | cut -d" " -f1)
  [ "$got" = "$want" ] || die "the downloaded archive is not the expected one (sha256 $got); nothing was installed"
  tar -xzf "$tmp/relay.tar.gz" -C "$tmp"
  found=$(find "$tmp" -type f -name iroh-relay | head -1)
  [ -n "$found" ] || die "the archive has no iroh-relay binary"
  mkdir -p "$DIR"
  install -m 0755 "$found" "$DIR/iroh-relay"
  rm -rf "$tmp"
  "$DIR/iroh-relay" --version
  echo "$1" > "$DIR/VERSION"
}

status() {
  [ -d "$DIR" ] || { say "not installed"; return 1; }
  say "installed in $DIR, mode $(cat "$DIR/mode" 2>/dev/null), iroh-relay $(cat "$DIR/VERSION" 2>/dev/null), service $(systemctl is-active $SVC 2>/dev/null || true)"
  [ -f "$DIR/port" ] && say "port $(cat "$DIR/port")/tcp$( [ -f "$DIR/ufw-rule" ] && echo ' (ufw rule added by this script)')"
  return 0
}

uninstall() {
  systemctl disable --now $SVC >/dev/null 2>&1 || true
  rm -f "$UNIT"
  systemctl daemon-reload
  systemctl reset-failed $SVC >/dev/null 2>&1 || true
  if [ -f "$DIR/ufw-rule" ] && ufw_on; then
    while read -r rule; do [ -n "$rule" ] && ufw delete allow "$rule" >/dev/null 2>&1 || true; done < "$DIR/ufw-rule"
  fi
  rm -rf "$DIR"
  if id $SVC >/dev/null 2>&1; then userdel $SVC >/dev/null 2>&1 || true; fi
  say "removed: the service, its user, $DIR and the firewall rule this script added. Nothing else was changed."
}

write_unit() {   # $1: extra capability lines or empty
  cat > "$UNIT" <<UNIT
[Unit]
Description=FERB's iroh relay
After=network-online.target
Wants=network-online.target

[Service]
User=$SVC
Group=$SVC
ExecStart=$DIR/iroh-relay --config-path $DIR/config.toml
WorkingDirectory=$DIR
Restart=always
RestartSec=3
$1
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=$DIR/certs
# a guest on a machine that may do other work: bounded, and low priority
MemoryMax=256M
CPUWeight=50
Nice=5

[Install]
WantedBy=multi-user.target
UNIT
  systemctl daemon-reload
  systemctl enable $SVC >/dev/null 2>&1
  systemctl restart $SVC
}

make_user_and_token() {
  id $SVC >/dev/null 2>&1 || useradd --system --home-dir "$DIR" --no-create-home --shell /usr/sbin/nologin $SVC
  mkdir -p "$DIR/certs"
  # The access token: only devices that present it may use this relay. Kept across reruns.
  if [ ! -f "$DIR/token" ]; then
    ( umask 077; head -c 32 /dev/urandom | b64url | cut -c1-40 | tr -d '\n' > "$DIR/token" )
  fi
  TOKEN=$(cat "$DIR/token")
}

lock_down() {
  chown -R root:$SVC "$DIR"
  chmod 0750 "$DIR"
  chmod 0640 "$DIR/config.toml" "$DIR/token"
  chown -R $SVC:$SVC "$DIR/certs"
  chmod 0700 "$DIR/certs"
}

# The config file of each mode, on stdout. $1: the access token.
config_shared() {
  echo "http_bind_addr = \"127.0.0.1:$PLAIN\""
  echo 'enable_metrics = false'
  echo "access.shared_token = [\"$1\"]"
  # Address discovery (UDP): how a device that uses only this relay learns its public address, which a direct
  # connection needs. FERB's side expects it on port 7842.
  if [ -n "$QUIC" ]; then echo 'enable_quic_addr_discovery = true'; fi
  echo
  echo '[tls]'
  echo "https_bind_addr = \"[::]:$PORT\""
  if [ -n "$QUIC" ]; then echo "quic_bind_addr = \"[::]:$QUIC\""; fi
  echo 'cert_mode = "Manual"'
  echo "manual_cert_path = \"$DIR/certs/default.crt\""
  echo "manual_key_path = \"$DIR/certs/default.key\""
}

config_dedicated() {
  echo 'http_bind_addr = "[::]:80"'
  echo 'enable_metrics = false'
  echo "access.shared_token = [\"$1\"]"
  echo 'enable_quic_addr_discovery = true'      # UDP 7842: lets devices find a direct connection
  echo
  echo '[tls]'
  echo 'https_bind_addr = "[::]:443"'
  echo "hostname = \"$HOST\""
  echo 'cert_mode = "LetsEncrypt"'
  echo "cert_dir = \"$DIR/certs\""
  echo "contact = \"$CONTACT\""      # iroh-relay requires a contact for Let's Encrypt
}

print_config() {
  echo "# The config file setup.sh writes to $DIR/config.toml for iroh-relay $VERSION (mode: $MODE)."
  echo "# Put your own access token in place of <token>. The relay runs in the foreground with:"
  echo "#     $DIR/iroh-relay --config-path $DIR/config.toml"
  echo "# How to get the binary, the token and (for --shared) the certificate by hand: relay/README.md, \"Without systemd\"."
  if [ "$MODE" = shared ]; then config_shared "<token>"; else config_dedicated "<token>"; fi
}

need_root_systemd_curl() {
  [ "$(id -u)" = 0 ] || die "run this as root"
  command -v systemctl >/dev/null || die "this script needs systemd"
  command -v curl >/dev/null || die "install curl first"
}

# ---- what was asked ---------------------------------------------------------------------------------------------
CHECK=0; PRINT=0
case "${1:-}" in
  --check) CHECK=1; shift ;;
  --print-config) PRINT=1; shift ;;
esac
USAGE="usage: sh setup.sh relay.example.com you@example.com   (or: sh setup.sh --shared)"
MODE=""; PORT=7443; ADDRESS=""; QUIC=$QUIC_PORT; HOST=""; CONTACT=""; NEWVER=$VERSION
case "${1:-}" in
  --status|--uninstall)
    [ "$CHECK$PRINT" = 00 ] || die "--check and --print-config go with an install (or --check with --update), not with $1"
    need_root_systemd_curl
    if [ "$1" = --status ]; then status; exit $?; fi
    uninstall; exit 0 ;;
  --update)
    [ "$PRINT" = 0 ] || die "--print-config goes with an install, not with --update"
    MODE=update; NEWVER="${2:-$VERSION}" ;;
  --shared)
    MODE=shared; shift
    while [ $# -gt 0 ]; do
      case "$1" in
        --port) PORT="${2:-}"; shift 2 ;;
        --address) ADDRESS="${2:-}"; shift 2 ;;
        *) die "unknown option $1" ;;
      esac
    done
    case "$PORT" in ''|*[!0-9]*) die "--port needs a number, like 7443" ;; esac ;;
  "")
    [ "$CHECK" = 1 ] || die "$USAGE"
    MODE=shared
    say "no mode given: checking for --shared. For a server of its own: sh setup.sh --check relay.example.com" ;;
  -*) die "unknown option $1. $USAGE" ;;
  *)
    MODE=dedicated; HOST="$1"; CONTACT="${2:-}"
    if [ -z "$CONTACT" ] && [ "$CHECK" = 0 ]; then die "$USAGE"; fi
    case "$HOST" in *://*|*/*) die "give only the host name, like relay.example.com" ;; esac ;;
esac
PLAIN=$((PORT + 1))

if [ "$PRINT" = 1 ]; then print_config; exit 0; fi

if [ "$MODE" = update ] && [ "$CHECK" = 0 ]; then
  [ -f "$DIR/config.toml" ] || die "nothing to update: run the setup first"
fi
preflight
if [ "$CHECK" = 1 ]; then
  say "this was only a check; nothing was installed."
  exit 0
fi

if [ "$MODE" = update ]; then
  install_binary "$NEWVER"
  systemctl restart $SVC
  sleep 2
  systemctl is-active --quiet $SVC || die "the relay did not start; see: journalctl -u $SVC -n 50"
  say "updated. The address, the token and the certificate are unchanged."
  exit 0
fi

if [ "$MODE" = shared ]; then
  # ---- a shared machine: a high port, the relay's own certificate, nothing else on the machine touched --------
  if [ ! -f "$DIR/port" ] && listening udp "$QUIC"; then
    say "UDP $QUIC is in use here, so address discovery stays off: through this relay direct connections will only happen on the same Wi-Fi"
    QUIC=""
  fi
  if [ -z "$ADDRESS" ]; then
    ADDRESS=$(route_src)
    [ -n "$ADDRESS" ] || die "could not find this machine's address; pass --address"
  fi
  case "$ADDRESS" in
    *[!0-9.]*) SAN="DNS:$ADDRESS" ;;
    *) SAN="IP:$ADDRESS" ;;
  esac
  if [ ! -x "$DIR/iroh-relay" ] || [ "$(cat "$DIR/VERSION" 2>/dev/null)" != "$VERSION" ]; then install_binary "$VERSION"; fi
  make_user_and_token
  # The relay's own certificate authority and certificate (kept across reruns for the same address). FERB pins
  # the authority: only this relay is accepted at this address, and no public certificate or DNS name is needed.
  if [ ! -f "$DIR/certs/ca.pem" ] || [ "$(cat "$DIR/address" 2>/dev/null)" != "$ADDRESS" ]; then
    tmp=$(mktemp -d)
    openssl ecparam -name prime256v1 -genkey -noout -out "$tmp/ca.key" 2>/dev/null
    openssl req -x509 -new -key "$tmp/ca.key" -sha256 -days 3650 -subj "/CN=ferb-relay authority" \
      -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign" -out "$tmp/ca.pem" 2>/dev/null
    openssl ecparam -name prime256v1 -genkey -noout 2>/dev/null | openssl pkcs8 -topk8 -nocrypt -out "$tmp/default.key" 2>/dev/null
    openssl req -new -key "$tmp/default.key" -subj "/CN=ferb-relay" -out "$tmp/req.pem" 2>/dev/null
    printf 'subjectAltName=%s\nbasicConstraints=CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n' "$SAN" > "$tmp/ext"
    openssl x509 -req -in "$tmp/req.pem" -CA "$tmp/ca.pem" -CAkey "$tmp/ca.key" -CAcreateserial -sha256 -days 3650 \
      -extfile "$tmp/ext" -out "$tmp/default.crt" 2>/dev/null
    cp "$tmp/ca.pem" "$tmp/default.crt" "$tmp/default.key" "$DIR/certs/"
    rm -rf "$tmp"       # the authority's key is not kept: it signed this one certificate and is gone
  fi
  echo shared > "$DIR/mode"; echo "$PORT" > "$DIR/port"; echo "$ADDRESS" > "$DIR/address"
  config_shared "$TOKEN" > "$DIR/config.toml"
  lock_down
  if ufw_on; then
    : > "$DIR/ufw-rule.new"
    for rule in "$PORT/tcp" ${QUIC:+"$QUIC/udp"}; do
      if ! ufw status | grep -Eq "^$rule +ALLOW"; then
        ufw allow "$rule" comment "$SVC" >/dev/null
        echo "$rule" >> "$DIR/ufw-rule.new"
        say "ufw: allowed $rule (added by this script; --uninstall removes it)"
      fi
    done
    cat "$DIR/ufw-rule.new" >> "$DIR/ufw-rule"; rm -f "$DIR/ufw-rule.new"
  fi
  write_unit ""
  ok=0
  for _ in $(seq 1 10); do
    if curl --noproxy '*' -fsS --max-time 5 --cacert "$DIR/certs/ca.pem" "https://$ADDRESS:$PORT/healthz" >/dev/null 2>&1; then ok=1; break; fi
    sleep 1
  done
  if [ "$ok" = 1 ]; then
    say "the relay is up: $(curl --noproxy '*' -fsS --cacert "$DIR/certs/ca.pem" "https://$ADDRESS:$PORT/healthz")"
  else
    say "the relay does not answer on https://$ADDRESS:$PORT/healthz from here. See: journalctl -u $SVC -n 50"
  fi
  CA=$(openssl x509 -in "$DIR/certs/ca.pem" -outform DER | b64url)
  cat <<DONE

In FERB on the Mac: Settings -> Phone -> Use my own relay. Paste this one line (keep it private):

ferbrelay://$ADDRESS:$PORT?tok=$TOKEN&ca=$CA

Press Test, then Save. Paired phones pick it up from the Mac by themselves.
If Test says the relay cannot be reached, a firewall outside this machine (your provider's) must allow TCP $PORT;
if it warns about address discovery, UDP ${QUIC:-7842} as well.
DONE
  exit 0
fi

# ---- a server of its own: ports 80 and 443, a certificate from Let's Encrypt --------------------------------------
if [ ! -x "$DIR/iroh-relay" ] || [ "$(cat "$DIR/VERSION" 2>/dev/null)" != "$VERSION" ]; then install_binary "$VERSION"; fi
make_user_and_token
echo dedicated > "$DIR/mode"; echo 443 > "$DIR/port"; echo "$HOST" > "$DIR/address"
config_dedicated "$TOKEN" > "$DIR/config.toml"
lock_down
write_unit "AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE"

say "waiting for the certificate (Let's Encrypt needs ports 80 and 443 reachable from the internet)..."
ok=0
for _ in $(seq 1 30); do
  if curl --noproxy '*' -fsS --max-time 5 "https://$HOST/healthz" >/dev/null 2>&1; then ok=1; break; fi
  sleep 3
done
if [ "$ok" = 1 ]; then
  say "the relay is up: $(curl --noproxy '*' -fsS "https://$HOST/healthz")"
else
  say "the relay did not answer on https://$HOST/healthz yet."
  say "check: the DNS name points here; ports 80 and 443 are open; journalctl -u $SVC -n 50"
fi
cat <<DONE

In FERB on the Mac: Settings -> Phone -> Use my own relay. Paste this one line (keep it private):

ferbrelay://$HOST?tok=$TOKEN

Press Test, then Save. Paired phones pick it up from the Mac by themselves.
DONE
