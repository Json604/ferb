# ferb-link

The phone link: one crate for both sides of the connection between FERB on the Mac and the phone apps, over iroh
(kb PHN-440). Stage 3 slice S1 (kb T-1200); `engine/JOURNAL.md` entry 102 says what was done and why.

## History

Ported on 2026-11-16 from the phone lab, `~/Desktop/ferb-phone-link` at commit `57b3f34` (`rust/`, read only), whose
`CONTRACT.md` (link version 2) both sides follow. The lab's decisions are imported into `kb/decisions/phone.toml`
(its PHN-n is PHN-n; SEC-n is PHN-(100+n), NET-n PHN-(200+n), LIM-n PHN-(300+n)); prose comments in the code still
name the lab's ids (`kb NET-22` is PHN-222), test tags use the repo's. `relay/` is the lab's own-relay setup script
and guide, unchanged.

What changed from the lab:

- **Build.** The repo's toolchain (1.100), its lint settings, explicit package fields as an excluded crate; edition
  2024 kept (the code relies on its drop order around Mutex guards). The Mac side (`bridge`, `admin`, `daemon`,
  `store`, `secrets`, `peer`, the `ferb-link` program) builds only on macOS; the rest is the phone side. iroh 1.4.1
  is patched by `engine/patches/apply.sh` (`engine/patches/README.md`). `anyhow` only with the `devrelay` feature.
- **The app's door (`ffi`).** `LinkClient` objects with explicit start and close, driven by the phone library
  (`ferb-mobile-ffi`) and named by handles in the Rust functions `client_*`; several per process; every boundary call
  catches a panic (`link_internal_error`) and is bounded (`START_DEADLINE`, `POLL_WAIT_MAX`, `CLOSE_DEADLINE`). The
  lab's three calls (`start`, `command`, `poll`) still drive one client per process. No C or JNI exports and no
  `cdylib` since T-1221 (the phone ships one library, ferb-mobile-ffi's).
- **The public relays carry text, questions and Stop only (PHN-444).** On the public relays the Mac refuses, before
  anything is read or begun: pictures (`link.get` previews, `tasks.preview`, `questions.picture`, `seen.image`),
  files both ways (`link.get`, `link.put`, `link.tree`, `link.put_group`), older chat pages (`chat.entries.page`
  with `before_pos` or `day`, `chat.history.page` with `before`), pages over `shared_page_max` (50) entries, and any
  reply or listing that does not fit one control frame. Codes: `needs_direct_or_own_relay`, `transfer_too_large`.
  A transfer whose path falls back to a public relay stops. The phone's client emits `bulk {allowed, tier}` when
  what its path carries changes, so the app can offer a refused thing again. The lab's eight shared-relay bulk
  caps (budget, daily total, picture size and pace, transfer size) no longer apply to anything; their settings are
  kept so older apps still read them.
- **No timed start-overs on the public relays (PHN-445);** the owner's relay keeps the lab's schedule.
- **Always use my relay (PHN-219, PHN-453)** is the setting `own_relay_always` (off: direct whenever it exists);
  on, both sides drop their IP transports while the owner's relay is in use. The phone's own relay state reaches
  the app as `relay_state` events (falling back to the public relay and returning, PHN-425).
- **Switches:** `segmentation_offload` (QUIC GSO, on as in the lab; iroh #4647), port mapping (off in tests).
- **Chat pages of stage 2** (`chat.entries.*`) are in the role table (reads private, mark plain); ferb-rpc's
  `Role::Phone` (T-1201) decides the engine's classes.

## Tests

`cargo test` runs on 127.0.0.1 only: local iroh relays, and `tests/common/standin.rs`, a Rust STAND-IN for the
daemon (fake answers; a port of the minimum of the lab's Python `standin/`; slice S3 builds the real side). By hand
(`--ignored`): `n0` (n0's public relays), `vm` (a real own relay), `measure_what_trying_for_direct_costs_on_a_relay`,
and the Keychain test. Left in the lab for the daemon's slices: the tests of FERB's protection lists, key places,
quarantine and the Mac-screen prompts (S3, S3b), and the check against FERB's own Python method table (S2).

## Build

```sh
engine/patches/apply.sh                      # once: the patched iroh in engine/target/patched
cargo test --manifest-path engine/crates/ferb-link/Cargo.toml -- --test-threads 3
cargo check --manifest-path engine/crates/ferb-link/Cargo.toml --target aarch64-linux-android   # with the NDK's CC and AR
cargo check --manifest-path engine/crates/ferb-link/Cargo.toml --locked --target aarch64-apple-ios   # and -ios-sim
```

iroh 1.4.1's network monitor (netwatch) and DNS resolver (n0-dns-resolver) are patched for iOS (kb PHN-478,
`engine/patches/README.md`): netwatch takes upstream's fix for libc 0.2.190 (n0-computer/net-tools#267) and netdev
0.47, and the resolver reads the nameservers through libresolv, so the iOS library imports nothing the iOS SDK marks
unavailable. `tools/ios_api_check.py` checks a built iOS library against the SDK's headers and lists the
required-reason APIs it uses (for the app's PrivacyInfo.xcprivacy):

```sh
IPHONEOS_DEPLOYMENT_TARGET=13.0 cargo build --manifest-path engine/crates/ferb-mobile-ffi/Cargo.toml --locked --target aarch64-apple-ios --lib
python3 -I engine/crates/ferb-link/tools/ios_api_check.py \
  "${CARGO_TARGET_DIR:-engine/crates/ferb-mobile-ffi/target}/aarch64-apple-ios/debug/libferb_mobile_ffi.dylib" --min-ios 13.0
```
