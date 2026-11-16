# Patches to published crates

Each patch here changes a crate exactly as published on crates.io, by a small reviewable diff; nothing else of the
crate is copied into the repository.

## iroh-1.4.1.patch (ferb-link; kb PHN-222)

iroh 1.4.1 keeps one address-discovery (QAD) connection to a relay per address family, but counted it as that
family's report only after its observed address *changed*. So every periodic network report (each 20 to 26 s) opened
new discovery connections to every relay in the map, a full TLS handshake each: about 100 kB a minute each way on
an idle phone. The patch makes the kept connection count from its first observation on: one value, in two places of
`src/net_report.rs`. Measured on the owner's phone in the lab: 111 kB a minute in / 87 out before, about 10 each way
after (kb PHN-222's evidence).

Upstream: n0-computer/iroh #4523 ("Initial QAD report is not cached, allowing redundant periodic probes") is this
fault; it was closed on 2026-10-06 as intended behaviour, with no fix merged. #4589 (open: an option to pause the
periodic net report while no connection is open) carries comments that seed the observer with the first report, the
same two lines as this patch, on 1.2.0 to 1.4.1. The code is unchanged in 1.4.1. The patch goes when an iroh release
no longer needs it.

## netwatch-0.19.3.patch (ferb-link; kb PHN-478)

iroh's network monitor. libc 0.2.190 (rust-lang/libc#5158) declares the routing-socket constants (`RTM_*`, `RTA_*`,
`RTAX_*`, `RTF_*`) for macOS only, as the iOS SDK has no `net/route.h`, so netwatch 0.19.3 does not build for iOS.
The patch is upstream's fix, n0-computer/net-tools#267 (issue #261, merged 2026-11-06, not released), backported:
its three source files apply to 0.19.3 unchanged, and the netdev requirement goes from 0.46.1 to 0.47.0 as in that
PR. The constants are defined in `src/interfaces/bsd/macos.rs` with the values of `net/route.h`; on macOS they equal
libc's (the PR's `constants_match_libc` test, run on 0.19.3 with this patch: passes), so the macOS code is the same.
netdev 0.47.0 (shellrow/netdev#195) uses only APIs the iOS SDK offers: no SystemConfiguration on iOS, DNS servers
from libresolv, no `in6_ifreq`. It also changes macOS and Android (route parsing, local address choice, 64-bit
netlink counters; netwatch's own 17 tests pass on macOS with it). The patch goes when iroh takes a netwatch with #267.

## n0-dns-resolver-0.1.0.patch (ferb-link; kb PHN-478)

iroh's DNS resolver reads the system's nameservers on every Apple platform from the SystemConfiguration dynamic
store (`SCDynamicStoreCreateWithOptions`, `SCDynamicStoreCopyValue`, `kSCDynamicStoreUseSessionKeys`), which the iOS
SDK marks `API_UNAVAILABLE(ios, tvos, watchos)`. The patch keeps that reader for macOS only and adds
`src/system_config/apple_mobile.rs` for Apple's mobile platforms: `res_ninit` and `res_getservers` from libresolv
(public in the iOS SDK; struct sizes checked against the iOS 27.2 SDK), as netdev 0.47's iOS reader does, with an
IPv6 resolver's scope kept; search domains are not read there. `system-configuration` becomes a macOS-only
dependency, `libc` a mobile-only one. No upstream issue or fix was found (n0-computer/n0-dns-resolver, 2026-11-16);
0.1.0 is the only release. The macOS code is unchanged (the same module, compiled under `target_os = "macos"`).

## How they are applied

Reproduced from the phone lab's `scripts/iroh-patch.sh` (ferb-phone-link@57b3f34), which made the iroh copy alone:

1. `engine/patches/apply.sh` (one table line per patched crate) takes `iroh-1.4.1.crate` from cargo's download cache, or downloads it from
   crates.io (the only network it uses), and refuses it unless its SHA-256 is the published
   `e3689cab508af9bcef7816c1201b403595ec6aa20e7d15e1f9683bb4be4a5b17` (netwatch 0.19.3: `39da9cad…5e96`,
   n0-dns-resolver 0.1.0: `535c99c9…48f7`; the full values are in the script's table).
2. It unpacks it into `engine/target/patched/iroh-1.4.1-ferb` (ignored by git, like all build output) and applies the
   patch with `patch -p1`. It does nothing when that copy is already there for this crate and this patch (both
   hashes are written into the copy).
3. `engine/crates/ferb-link/Cargo.toml` has `[patch.crates-io] iroh = { path = "../../target/patched/iroh-1.4.1-ferb" }`, and
   the same for the other two.

Run `engine/patches/apply.sh` once before any cargo command on ferb-link. When ferb-link joins the workspace, the `[patch]` table
moves to `engine/Cargo.toml` (cargo reads it only at a workspace root) and `engine/gate.sh` runs the script before
cargo.
