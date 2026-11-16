#!/bin/sh
# Runs setup.sh's preflight (--check) and --print-config against a made-up machine: stub commands on PATH and a
# fake /etc/os-release, /proc/meminfo and /run/systemd under a temporary folder. Nothing is installed and no
# network is used. It shows that the checks decide and word things as intended; it does not show that the
# install works on any system.
#
#   sh relay/tests/preflight_test.sh            (runs setup.sh with sh, and with dash when there is one)
set -u

HERE=$(cd "$(dirname "$0")" && pwd)
SETUP="$HERE/../setup.sh"
T=$(mktemp -d "${TMPDIR:-/tmp}/ferb-relay-test.XXXXXX")
trap 'rm -rf "$T"' EXIT
mkdir -p "$T/all" "$T/base" "$T/conf" "$T/root"

# Real tools the script and the stubs may use (everything else is a stub or absent).
for c in awk sed grep head tail cut tr cat sort dirname find mktemp seq rm sleep; do
  p=$(command -v "$c") || { echo "this test needs $c"; exit 2; }
  ln -s "$p" "$T/base/$c"
done

stub() { cat > "$T/all/$1"; chmod +x "$T/all/$1"; }
C="$T/conf"
stub uname <<E
#!/bin/sh
case "\$1" in -s) cat "$C/kernel" ;; -m) cat "$C/arch" ;; *) echo Linux ;; esac
E
stub id <<E
#!/bin/sh
[ "\${1:-}" = -u ] && { cat "$C/uid"; exit 0; }
exit 1
E
stub systemctl <<E
#!/bin/sh
if [ "\$1" = is-active ]; then
  case "\$*" in *firewalld*) [ "\$(cat "$C/firewalld")" = active ]; exit \$? ;; esac
  exit 3
fi
echo "systemctl \$*" >> "$C/changes"
E
stub ss <<E
#!/bin/sh
case "\$1" in -t*) f="$C/tcp" ;; *) f="$C/udp" ;; esac
while read -r p; do [ -n "\$p" ] && echo "LISTEN 0 128 0.0.0.0:\$p 0.0.0.0:*"; done < "\$f"
exit 0
E
stub netstat <<E
#!/bin/sh
echo "Active Internet connections (only servers)"
echo "Proto Recv-Q Send-Q Local Address Foreign Address State"
case "\$1" in -t*) f="$C/tcp" ;; *) f="$C/udp" ;; esac
while read -r p; do [ -n "\$p" ] && echo "tcp 0 0 0.0.0.0:\$p 0.0.0.0:* LISTEN"; done < "\$f"
exit 0
E
stub curl <<E
#!/bin/sh
case "\$*" in
  *-sSI*)
    rc=\$(cat "$C/curl_rc")
    [ "\$rc" = 0 ] || { echo "curl: (6) Could not resolve host"; exit "\$rc"; }
    printf 'HTTP/2 200 \r\nserver: test\r\n'
    [ -s "$C/http_date" ] && printf 'date: %s\r\n' "\$(cat "$C/http_date")"
    printf '\r\n' ;;
  *) echo "curl \$*" >> "$C/changes" ;;
esac
E
stub date <<E
#!/bin/sh
cat "$C/now"
E
stub df <<E
#!/bin/sh
echo "Filesystem 1024-blocks Used Available Capacity Mounted on"
echo "/dev/vda1 25000000 1000000 \$(cat "$C/df_kb") 10% /"
E
stub ip <<E
#!/bin/sh
s=\$(cat "$C/ip_src")
[ -n "\$s" ] || exit 1
echo "1.1.1.1 via 203.0.113.1 dev eth0 src \$s uid 0"
E
stub ufw <<E
#!/bin/sh
[ "\$1" = status ] && { echo "Status: \$(cat "$C/ufw")"; exit 0; }
echo "ufw \$*" >> "$C/changes"
E
stub iptables <<E
#!/bin/sh
echo "-P INPUT ACCEPT"
[ "\$(cat "$C/iptables")" = rules ] && echo "-A INPUT -p tcp --dport 22 -j ACCEPT"
exit 0
E
stub openssl <<E
#!/bin/sh
[ "\$1" = version ] && { cat "$C/openssl"; exit 0; }
echo "openssl \$*" >> "$C/changes"
E
stub getent <<E
#!/bin/sh
[ -s "$C/dns" ] || exit 2
while read -r a; do echo "\$a STREAM \$2"; done < "$C/dns"
E
# anything that would change the machine only leaves a line in conf/changes
for c in tar sha256sum install useradd userdel mkdir chown chmod cp; do
  stub "$c" <<E
#!/bin/sh
echo "$c \$*" >> "$C/changes"
E
done

# A healthy machine: Ubuntu 24.04, x86_64, systemd, root, everything free. Each case changes what it is about.
fresh() {
  rm -rf "$T/bin" "$T/root"
  mkdir -p "$T/bin" "$T/root/etc" "$T/root/proc/1" "$T/root/run/systemd/system"
  for f in "$T"/all/*; do ln -s "$f" "$T/bin/$(basename "$f")"; done
  rm -f "$T/bin/netstat" "$T/bin/iptables"
  printf 'PRETTY_NAME="Ubuntu 24.04.1 LTS"\nID=ubuntu\nVERSION_ID="24.04"\n' > "$T/root/etc/os-release"
  printf 'MemTotal: 980000 kB\nMemAvailable: 700000 kB\n' > "$T/root/proc/meminfo"
  echo systemd > "$T/root/proc/1/comm"
  echo Linux > "$C/kernel"; echo x86_64 > "$C/arch"; echo 0 > "$C/uid"
  : > "$C/tcp"; printf '68\n' > "$C/udp"
  echo 0 > "$C/curl_rc"; echo "Tue, 10 Nov 2026 12:00:00 GMT" > "$C/http_date"; echo 1794312030 > "$C/now"
  echo 20000000 > "$C/df_kb"; echo 203.0.113.7 > "$C/ip_src"
  echo inactive > "$C/ufw"; echo inactive > "$C/firewalld"; echo none > "$C/iptables"
  echo "OpenSSL 3.0.13 30 Jan 2024 (Library: OpenSSL 3.0.13 30 Jan 2024)" > "$C/openssl"
  echo 203.0.113.7 > "$C/dns"
  : > "$C/changes"
}
without() { for c in "$@"; do rm -f "$T/bin/$c"; done; }
with() { for c in "$@"; do ln -s "$T/all/$c" "$T/bin/$c"; done; }

PASS=0; FAIL=0; CASE=""
run() {   # runs setup.sh with the given arguments on the made-up machine
  OUT=$(env -i PATH="$T/bin:$T/base" FERB_RELAY_PREFLIGHT_ROOT="$T/root" ${EXTRA_ENV:-} "$SH" "$SETUP" "$@" 2>&1)
  RC=$?
}
t() { CASE="$1"; }
good() { PASS=$((PASS + 1)); }
wrong() { FAIL=$((FAIL + 1)); echo "FAIL [$SH] $CASE: $1"; echo "$OUT" | sed 's/^/    | /'; }
has() { if echo "$OUT" | grep -Eq -e "$1"; then good; else wrong "expected a line matching: $1"; fi; }
hasnt() { if echo "$OUT" | grep -Eq -e "$1"; then wrong "did not expect: $1"; else good; fi; }
rc() { if [ "$RC" = "$1" ]; then good; else wrong "expected exit $1, got $RC"; fi; }
unchanged() { if [ -s "$C/changes" ]; then wrong "something was changed: $(cat "$C/changes")"; else good; fi; }

suite() {
  t "a healthy Ubuntu 24.04 machine, --shared"
  fresh; run --check --shared
  rc 0; unchanged
  has "ok +system: Ubuntu 24.04.1 LTS .*run on for real"
  has "ok +CPU: x86_64, iroh-relay 1.4.1"
  has "ok +init system: systemd is running"
  has "ok +root: running as root"
  has "ok +commands: all there"
  has "ok +openssl: OpenSSL 3.0.13"
  has "ok +memory: 683 MB free \(64 MB needed\)"
  has "ok +disk: 19531 MB free"
  has "ok +TCP port 7443 \(the relay\): free"
  has "ok +TCP port 7444 .*127.0.0.1 only.*: free"
  has "ok +UDP port 7842 .*optional.*: free"
  has "ok +firewall: no firewall rules found"
  has "note +provider firewall: .*cannot be seen from here.*TCP 7443 and UDP 7842"
  has "ok +address: 203.0.113.7 .*public"
  has "ok +outbound HTTPS: github.com answers"
  has "ok +clock: within 30 s"
  has "no problems \(0 warning\(s\), 0 skipped\)"
  has "only a check; nothing was installed"
  hasnt "^ +(problem|warning|skipped) "

  t "--check alone means --shared and says so"
  fresh; run --check
  rc 0; has "no mode given: checking for --shared"

  t "another distribution, aarch64"
  fresh; printf 'PRETTY_NAME="Debian GNU/Linux 12 (bookworm)"\nID=debian\nVERSION_ID="12"\n' > "$T/root/etc/os-release"; echo aarch64 > "$C/arch"
  run --check --shared
  rc 0; has "note +system: Debian GNU/Linux 12 \(bookworm\)\. This script has not been run on it"; has "ok +CPU: aarch64"

  t "no /etc/os-release"
  fresh; rm "$T/root/etc/os-release"; run --check --shared
  rc 0; has "warning +system: Linux, but /etc/os-release is missing"

  t "not Linux"
  fresh; echo Darwin > "$C/kernel"; run --check --shared
  rc 1; has "problem +system: this is Darwin, not Linux"

  t "unsupported CPU"
  fresh; echo riscv64 > "$C/arch"; run --check --shared
  rc 1; has "problem +CPU: riscv64\. .*only for x86_64 and aarch64"; has "1 problem\(s\).*Nothing was changed"

  t "a version with no pinned checksum"
  fresh; EXTRA_ENV="IROH_RELAY_VERSION=1.5.0"; run --check --shared; EXTRA_ENV=""
  rc 1; has "problem +CPU: x86_64 is fine, but no checksum is known for iroh-relay 1.5.0"
  fresh; EXTRA_ENV="IROH_RELAY_VERSION=1.5.0 IROH_RELAY_SHA256=abc"; run --check --shared; EXTRA_ENV=""
  rc 0; has "ok +CPU: x86_64, iroh-relay 1.5.0"

  t "no systemd (OpenRC)"
  fresh; without systemctl; rm -rf "$T/root/run/systemd"; echo init > "$T/root/proc/1/comm"; run --check --shared
  rc 1; has "problem +init system: there is no systemd here \(no systemctl\); process 1 is init"
  has "Without systemd.*relay/README.md.*--print-config"

  t "systemd installed but not running (a container)"
  fresh; rm -rf "$T/root/run/systemd"; echo bash > "$T/root/proc/1/comm"; run --check --shared
  rc 1; has "problem +init system: systemd is installed but did not start this system.*process 1 is bash"

  t "not root: --check still runs and says what it skipped"
  fresh; echo 1000 > "$C/uid"; run --check --shared
  rc 0; has "skipped +root: not root"; has "skipped +firewall: .*needs root"; has "no problems \(0 warning\(s\), 2 skipped\)"

  t "not root: the install stops"
  fresh; echo 1000 > "$C/uid"; run --shared
  rc 1; unchanged; has "problem +root: not root\. Run this as root"

  t "missing commands"
  fresh; without sha256sum openssl ss; run --check --shared
  rc 1; has "problem +commands: missing: sha256sum openssl ss\."
  fresh; without ss; with netstat; run --check --shared
  rc 0; has "ok +commands: all there"; has "ok +TCP port 7443 \(the relay\): free"
  fresh; without ss; with netstat; echo 7443 > "$C/tcp"; run --check --shared
  rc 1; has "problem +TCP port 7443 .*already in use"

  t "old openssl"
  fresh; echo "OpenSSL 1.0.2k-fips  26 Jan 2017" > "$C/openssl"; run --check --shared
  rc 1; has "problem +openssl: OpenSSL 1.0.2k.*too old"

  t "low memory"
  fresh; printf 'MemTotal: 480000 kB\nMemAvailable: 30000 kB\n' > "$T/root/proc/meminfo"; run --check --shared
  rc 1; has "problem +memory: 29 MB free, 64 MB needed"

  t "low disk"
  fresh; echo 50000 > "$C/df_kb"; run --check --shared
  rc 1; has "problem +disk: 48 MB free under .*100 MB needed"

  t "the relay's port is taken"
  fresh; printf '22\n7443\n' > "$C/tcp"; run --check --shared
  rc 1; has "problem +TCP port 7443 \(the relay\): already in use.*--port"; has "ok +TCP port 7444"
  fresh; printf '22\n7443\n' > "$C/tcp"; run --check --shared --port 8443
  rc 0; has "ok +TCP port 8443 \(the relay\): free"; has "ok +TCP port 8444"
  fresh; printf '7444\n' > "$C/tcp"; run --check --shared
  rc 1; has "problem +TCP port 7444 .*port plus one"
  fresh; run --check --shared --port http
  rc 1; has "--port needs a number"

  t "the UDP discovery port is taken: a warning in --shared, a stop for a server of its own"
  fresh; printf '7842\n' > "$C/udp"; run --check --shared
  rc 0; has "warning +UDP port 7842 .*leaves address discovery off"
  fresh; printf '7842\n' > "$C/udp"; run --check relay.example.com you@example.com
  rc 1; has "problem +UDP port 7842 .*would not start"

  t "a server of its own"
  fresh; run --check relay.example.com you@example.com
  rc 0; has "ok +TCP port 80 "; has "ok +TCP port 443 "; has "ok +name: relay.example.com resolves to 203.0.113.7"
  has "provider firewall: .*TCP 80, TCP 443 and UDP 7842"; hasnt "openssl"
  fresh; printf '80\n443\n' > "$C/tcp"; run --check relay.example.com
  rc 1; has "problem +TCP port 80 .*--shared"; has "problem +TCP port 443 "
  fresh; : > "$C/dns"; run --check relay.example.com
  rc 0; has "warning +name: relay.example.com does not resolve"
  fresh; echo 198.51.100.9 > "$C/dns"; run --check relay.example.com
  rc 0; has "warning +name: relay.example.com resolves to 198.51.100.9, and this machine's address is 203.0.113.7"

  t "firewalls"
  fresh; echo active > "$C/ufw"; run --check --shared
  rc 0; has "ok +firewall: ufw is active; the install adds allow rules for TCP 7443 and UDP 7842"
  fresh; echo active > "$C/ufw"; run --check relay.example.com
  rc 0; has "warning +firewall: ufw is active\. In this mode the script adds no ufw rules.*ufw allow 80/tcp"
  fresh; echo active > "$C/firewalld"; run --check --shared
  rc 0; has "warning +firewall: firewalld is active.*--add-port=7443/tcp --add-port=7842/udp"
  fresh; with iptables; echo rules > "$C/iptables"; run --check --shared
  rc 0; has "warning +firewall: iptables rules are present"
  fresh; with iptables; run --check --shared
  rc 0; has "ok +firewall: no firewall rules found"

  t "addresses"
  fresh; echo 10.0.0.5 > "$C/ip_src"; run --check --shared
  rc 1; has "problem +address: this machine's own address is 10.0.0.5, a private one.*--address PUBLIC_ADDRESS"
  fresh; echo 100.64.3.2 > "$C/ip_src"; run --check --shared
  rc 1; has "problem +address: .*100.64.3.2, a private one"
  fresh; echo 10.0.0.5 > "$C/ip_src"; run --check --shared --address 203.0.113.7
  rc 0; has "ok +address: 203.0.113.7 \(given with --address\)"
  fresh; echo 10.0.0.5 > "$C/ip_src"; run --check --shared --address 10.0.0.5
  rc 0; has "warning +address: 10.0.0.5 .*private address"
  fresh; run --check --shared --address relay.example.com
  rc 0; has "ok +address: relay.example.com \(a name"
  fresh; without ip; run --check --shared
  rc 1; has "problem +address: this machine's IPv4 address could not be found.*--address"
  fresh; echo 172.20.1.1 > "$C/ip_src"; run --check relay.example.com
  rc 0; has "warning +address: .*172.20.1.1, a private one: it is behind NAT"

  t "clock"
  fresh; echo 1794312600 > "$C/now"; run --check --shared
  rc 1; has "problem +clock: this machine's clock is 600 s \(about 10 min\) ahead"; has "timedatectl set-ntp true"
  fresh; echo 1794311400 > "$C/now"; run --check --shared
  rc 1; has "problem +clock: .*600 s \(about 10 min\) behind"
  fresh; echo 1794312300 > "$C/now"; run --check --shared
  rc 0; has "ok +clock: within 300 s"
  fresh; echo "Tue, 29 Feb 2028 23:59:59 GMT" > "$C/http_date"; echo 1835481599 > "$C/now"; run --check --shared
  rc 0; has "ok +clock: within 0 s"
  fresh; echo "Fri, 01 Jan 2027 00:00:00 GMT" > "$C/http_date"; echo 1798761601 > "$C/now"; run --check --shared
  rc 0; has "ok +clock: within 1 s"
  fresh; : > "$C/http_date"; run --check --shared
  rc 0; has "warning +clock: could not be compared"

  t "no way out to the release host"
  fresh; echo 6 > "$C/curl_rc"; run --check --shared
  rc 1; has "problem +outbound HTTPS: https://github.com does not answer from here \(curl: \(6\) Could not resolve host\)"
  has "skipped +clock: not compared"

  t "a failed required check stops the install before anything is changed"
  fresh; echo riscv64 > "$C/arch"; run --shared
  rc 1; unchanged; has "Nothing was changed"; hasnt "downloading"
  fresh; without systemctl; rm -rf "$T/root/run/systemd"; run relay.example.com you@example.com
  rc 1; unchanged; has "problem +init system"; hasnt "downloading"
  fresh; printf '7443\n' > "$C/tcp"; run --shared
  rc 1; unchanged

  t "--update"
  fresh; run --check --update
  rc 0; has "checking this machine for an update to iroh-relay 1.4.1"; hasnt "TCP port|memory|firewall|address"
  fresh; run --check --update 1.4.2
  rc 1; has "no checksum is known for iroh-relay 1.4.2"
  fresh; run --update
  rc 1; unchanged; has "nothing to update: run the setup first"

  t "--status and --uninstall keep their guards"
  fresh; echo 1000 > "$C/uid"; run --status
  rc 1; has "setup: run this as root"
  fresh; without systemctl; run --uninstall
  rc 1; unchanged; has "setup: this script needs systemd"
  fresh; run --check --status
  rc 1; has "go with an install"

  t "usage"
  fresh; run
  rc 1; has "usage: sh setup.sh relay.example.com you@example.com"
  fresh; run relay.example.com
  rc 1; has "usage: "
  fresh; run https://relay.example.com you@example.com
  rc 1; has "give only the host name"
  fresh; run --shared --bogus
  rc 1; has "unknown option --bogus"

  t "--print-config needs no root and no systemd"
  fresh; echo 1000 > "$C/uid"; without systemctl; rm -rf "$T/root/run/systemd"; run --print-config --shared --port 8443
  rc 0; unchanged
  has '^http_bind_addr = "127.0.0.1:8444"$'; has '^access.shared_token = \["<token>"\]$'
  has '^enable_quic_addr_discovery = true$'; has '^https_bind_addr = "\[::\]:8443"$'; has '^quic_bind_addr = "\[::\]:7842"$'
  has '^cert_mode = "Manual"$'; has '^manual_cert_path = "/opt/ferb-relay/certs/default.crt"$'
  has "^# +/opt/ferb-relay/iroh-relay --config-path /opt/ferb-relay/config.toml$"
  fresh; run --print-config relay.example.com you@example.com
  rc 0; unchanged
  has '^http_bind_addr = "\[::\]:80"$'; has '^hostname = "relay.example.com"$'; has '^cert_mode = "LetsEncrypt"$'
  has '^cert_dir = "/opt/ferb-relay/certs"$'; has '^contact = "you@example.com"$'
}

[ ! -e /opt/ferb-relay ] || { echo "this machine has /opt/ferb-relay: the test is for a machine without a relay"; exit 2; }
for SH in sh dash; do
  SH=$(command -v "$SH") || continue
  before=$((PASS + FAIL))
  suite
  echo "$SH: $((PASS + FAIL - before)) assertions run"
done
echo "passed $PASS, failed $FAIL"
[ "$FAIL" = 0 ]
