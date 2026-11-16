//! The pairing code as the QR carries it: the short form and the old link, read back exactly; what is refused;
//! how large each one's QR is. No network. It also writes the sample codes the scanning test draws
//! (scripts/scan-test.sh).

use ferb_link::{
    errors,
    net::RelaySpec,
    pairing::{Invite, KNOWN_RELAYS},
    qr,
};
use qrcode::EcLevel;

/// A throwaway authority made with the commands of relay/setup.sh (its key was never kept).
const CA: &str = "MIIBpDCCAUmgAwIBAgIUCIJUcWguJ3bydd6aX0Q591mr5PYwCgYIKoZIzj0EAwIwHzEdMBsGA1UEAwwUZmVyYi1yZWxheSBhdXRob3JpdHkwHhcNMjYxMTEwMTEyOTQ1WhcNMzYxMTA3MTEyOTQ1WjAfMR0wGwYDVQQDDBRmZXJiLXJlbGF5IGF1dGhvcml0eTBZMBMGByqGSM49AgEGCCqGSM49AwEHA0IABL1BtQGTFd0i1NFfoTqXbR7AqCSY47kwHHPqtbgvP9XaH_jWvjb3CiRL9Qudgx913t8k-2zbr7_moymxcPsjl_ejYzBhMB0GA1UdDgQWBBQ_VpXC9SO-t-79zfsEnJI2jp6pITAfBgNVHSMEGDAWgBQ_VpXC9SO-t-79zfsEnJI2jp6pITAPBgNVHRMBAf8EBTADAQH_MA4GA1UdDwEB_wQEAwICBDAKBggqhkjOPQQDAgNJADBGAiEAl6I3ev79fziGRJvs9nOj-Tm5ez1bl0cdPb2gGn2mcnECIQC6zLXoTxGmxO3y8MLifT6jLLM6X0C0XTy9yB_2eBoI8g";
const TOKEN: &str = "q0tZ3mXw8Jr5v2NcLb7Hd1sPfYk9AeUoG4iT6xRz";

fn mac() -> iroh::EndpointId {
    iroh::SecretKey::from_bytes(&[42; 32]).public()
}
fn secret() -> [u8; 32] {
    std::array::from_fn(|i| (i * 7 + 3) as u8)
}
fn shared() -> Invite {
    Invite {
        mac: mac(),
        relays: vec![KNOWN_RELAYS[2].to_string()],
        own: None,
        secret: secret(),
    }
}
fn own(token: bool, ca: bool) -> Invite {
    let url = "https://203.0.113.10:7443".to_string();
    Invite {
        mac: mac(),
        relays: vec![url.clone()],
        own: Some(RelaySpec {
            url,
            token: token.then(|| TOKEN.to_string()),
            ca: ca.then(|| CA.to_string()),
        }),
        secret: secret(),
    }
}

// decision: PHN-135
#[test]
fn the_short_code_reads_back_exactly() {
    for invite in [
        shared(),
        Invite {
            relays: KNOWN_RELAYS.iter().map(|s| s.to_string()).collect(),
            ..shared()
        },
        Invite {
            relays: vec![
                "http://localhost:3341".into(),
                "https://relay.example.com/".into(),
                "https://relay.example.com".into(),
            ],
            ..shared()
        },
        Invite {
            relays: vec![],
            ..shared()
        },
        own(false, false),
        own(true, false),
        Invite {
            own: Some(RelaySpec::new(
                "https://relay.example.com",
                Some("not base64 !".into()),
            )),
            ..shared()
        },
        Invite {
            own: Some(RelaySpec::new("http://localhost:3340", Some("test".into()))),
            relays: vec!["http://localhost:3340".into(), KNOWN_RELAYS[0].into()],
            ..shared()
        },
    ] {
        let code = invite.to_code().unwrap();
        assert!(
            code.starts_with("FERB-")
                && code
                    .bytes()
                    .all(|c| b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ+-./:".contains(&c)),
            "{code}"
        );
        assert_eq!(Invite::parse(&code).unwrap(), invite, "{code}");
        // the link older builds show still reads, and says the same
        assert_eq!(Invite::parse(&invite.to_url()).unwrap(), invite);
    }
    // the whole one-time secret is in it: every bit of it changes the code
    let other = Invite {
        secret: {
            let mut s = secret();
            s[31] ^= 1;
            s
        },
        ..shared()
    };
    assert_ne!(other.to_code().unwrap(), shared().to_code().unwrap());
    assert_eq!(
        Invite::parse(&other.to_code().unwrap()).unwrap().secret,
        other.secret
    );
}

// decision: PHN-135
#[test]
fn an_own_relays_authority_travels_as_its_name_and_key() {
    let invite = own(true, true);
    let read = Invite::parse(&invite.to_code().unwrap()).unwrap();
    let (theirs, mine) = (read.own.clone().unwrap(), invite.own.clone().unwrap());
    assert_eq!(
        (
            theirs.url.as_str(),
            &theirs.token,
            &read.relays,
            read.mac,
            read.secret
        ),
        (
            mine.url.as_str(),
            &mine.token,
            &invite.relays,
            invite.mac,
            invite.secret
        )
    );
    assert!(
        theirs.ca.as_deref().unwrap().starts_with("key:"),
        "the certificate itself is not in the code"
    );
    assert_eq!(
        theirs.authority().unwrap(),
        mine.authority().unwrap(),
        "the same name and key the certificate holds"
    );
    assert!(theirs.roots().unwrap().is_some());
    // a P-256 key (what the setup script makes) is carried as its 33-byte point
    // and the name the setup script gives it is not spelled out: 35 bytes in all
    let with_key = invite.to_code().unwrap().len();
    let without = own(true, false).to_code().unwrap().len();
    assert!(
        (52..=53).contains(&(with_key - without)),
        "{}",
        with_key - without
    );
    // an authority with another name, and a key that is not P-256, travel whole
    let other = RelaySpec {
        ca: Some(RelaySpec::authority_text(
            b"\x31\x0b\x30\x09\x06\x03\x55\x04\x03\x0c\x02me",
            &[7u8; 300],
        )),
        ..mine.clone()
    };
    let invite2 = Invite {
        own: Some(other.clone()),
        ..invite.clone()
    };
    assert_eq!(Invite::parse(&invite2.to_code().unwrap()).unwrap(), invite2);
    // a damaged certificate does not fit the short form; the Mac then shows the link
    let mut bad = invite.clone();
    bad.own.as_mut().unwrap().ca = Some("AAAA".into());
    assert!(bad.to_code().is_err());
}

// decision: PHN-135
#[test]
fn the_old_link_and_a_pasted_code_still_read() {
    // a link exactly as a build before the short code printed it
    let id = mac().to_string();
    let old = format!(
        "ferblink://pair?v=2&id={id}&s=AwoRGB8mLTQ7QklQV15lbHN6gYiPlp2kq7K5wMfO1dw&r=https%3A%2F%2Feuc1-1.relay.n0.iroh.link.%2F"
    );
    assert_eq!(Invite::parse(&old).unwrap(), shared());
    assert_eq!(shared().to_url(), old, "the link has not changed");
    // typed or pasted: lower case, spaces and line breaks inside are not part of it
    let code = shared().to_code().unwrap();
    let wrapped = format!(
        "  {}\n   {} \r\n{}  ",
        code[..30].to_lowercase(),
        &code[30..70],
        &code[70..]
    );
    assert_eq!(Invite::parse(&wrapped).unwrap(), shared());
    let wrapped_link = format!("{}\n  {}", &old[..80], &old[80..]);
    assert_eq!(Invite::parse(&wrapped_link).unwrap(), shared());
}

// decision: PHN-135
#[test]
fn a_damaged_or_foreign_code_is_refused() {
    let code = shared().to_code().unwrap();
    for (bad, why) in [
        (code[..code.len() - 3].to_string(), "cut off"),
        (format!("{code}000"), "longer"),
        (code.replace('-', "_"), "wrong character"),
        ("FERB-".to_string(), "empty"),
        ("https://example.com/".to_string(), "another link"),
    ] {
        assert_eq!(
            Invite::parse(&bad).unwrap_err().code,
            errors::BAD_REQUEST,
            "{why}"
        );
    }
    // another link version, and a shared relay this build has no name for: update, not "not a code"
    let v3 = format!("FERB-{}{}", "30", &code[7..]);
    assert_ne!(v3, code);
    assert_eq!(
        Invite::parse(&v3).unwrap_err().code,
        errors::VERSION_MISMATCH
    );
    let mut far = shared();
    far.relays = vec![];
    let none = far.to_code().unwrap();
    assert_eq!(Invite::parse(&none).unwrap().relays.len(), 0);
    // every default shared relay has a number
    for url in ferb_link::net::Shared::N0.urls() {
        assert!(
            KNOWN_RELAYS.contains(&url.to_string().as_str()),
            "{url} needs a number in KNOWN_RELAYS (add at the end)"
        );
    }
}

fn modules(text: &str, level: EcLevel) -> usize {
    qr::rows_at(text, level, true).len()
}

// decision: PHN-135
#[test]
fn how_large_each_code_is() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("qr-samples");
    std::fs::create_dir_all(&dir).unwrap();
    let mut table = String::new();
    for (name, old, new) in [
        ("shared", shared().to_url(), shared().to_code().unwrap()),
        (
            "own",
            own(true, false).to_url(),
            own(true, false).to_code().unwrap(),
        ),
        (
            "own-ca",
            own(true, true).to_url(),
            own(true, true).to_code().unwrap(),
        ),
    ] {
        for (kind, text) in [("old", &old), ("new", &new)] {
            for (l, level) in [("L", EcLevel::L), ("M", EcLevel::M), ("Q", EcLevel::Q)] {
                let rows = qr::rows_at(text, level, true);
                table.push_str(&format!(
                    "MEASURE {name} {kind} {l}: {} characters, version {}, {} modules a side\n",
                    text.len(),
                    (rows.len() - 17) / 4,
                    rows.len()
                ));
                std::fs::write(
                    dir.join(format!("{name}-{kind}-{l}.txt")),
                    format!("{text}\n{}\n", rows.join("\n")),
                )
                .unwrap();
            }
        }
    }
    print!("{table}");
    assert_eq!(
        qr::rows(&shared().to_code().unwrap()).len(),
        modules(&shared().to_code().unwrap(), qr::LEVEL)
    );
    // the bounds this change is for: the usual code is version 5 or smaller, and an own relay with its own
    // authority no larger than the old code was without any own relay
    assert!(
        modules(&shared().to_code().unwrap(), qr::LEVEL) <= 37,
        "{table}"
    );
    assert!(
        modules(&own(true, true).to_code().unwrap(), EcLevel::M)
            <= modules(&shared().to_url(), EcLevel::M),
        "{table}"
    );
    // the terminal drawing: a four-module quiet zone, black on white set by the code itself
    let text = qr::text(&shared().to_code().unwrap(), true);
    let side = modules(&shared().to_code().unwrap(), qr::LEVEL) + 8;
    assert_eq!(text.lines().count(), side.div_ceil(2));
    assert!(text.lines().all(|l| l.starts_with("\x1b[38;5;16;48;5;231m")
        && l.ends_with("\x1b[0m")
        && l.chars().filter(|c| "█▀▄ ".contains(*c)).count() == side));
}

// decision: PHN-136
#[test]
fn the_mask_is_chosen_so_nothing_else_looks_like_a_corner_square() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("qr-many");
    std::fs::create_dir_all(&dir).unwrap();
    let count = |rows: &[String]| {
        qr::lookalikes(
            rows.len(),
            &rows
                .iter()
                .flat_map(|r| r.bytes().map(|b| b == b'1'))
                .collect::<Vec<_>>(),
        )
    };
    for (name, make) in [
        ("shared", shared as fn() -> Invite),
        ("own-ca", || own(true, true)),
    ] {
        let (mut usual_bad, mut careful_bad, n) = (0, 0, 200);
        for i in 0..n {
            // a different Mac and secret each time, as real codes are
            let mut invite = make();
            invite.secret = *blake3::hash(&[i as u8, 1]).as_bytes();
            invite.mac =
                iroh::SecretKey::from_bytes(blake3::hash(&[i as u8, 2]).as_bytes()).public();
            let code = invite.to_code().unwrap();
            let (usual, careful) = (qr::rows_at(&code, qr::LEVEL, false), qr::rows(&code));
            usual_bad += (count(&usual) > 0) as u32;
            careful_bad += (count(&careful) > 0) as u32;
            assert!(count(&careful) <= count(&usual));
            std::fs::write(
                dir.join(format!("{name}-usual-{i:03}.txt")),
                format!("{code}\n{}\n", usual.join("\n")),
            )
            .unwrap();
            std::fs::write(
                dir.join(format!("{name}-careful-{i:03}.txt")),
                format!("{code}\n{}\n", careful.join("\n")),
            )
            .unwrap();
            // (the link as it was drawn before, for the scan test's comparison)
            let url = invite.to_url();
            std::fs::write(
                dir.join(format!("{name}-before-{i:03}.txt")),
                format!(
                    "{url}\n{}\n",
                    qr::rows_at(&url, qr::LEVEL, false).join("\n")
                ),
            )
            .unwrap();
        }
        println!(
            "MEASURE {name}: codes with a corner-square look-alike, of {n}: {usual_bad} with the standard's mask, {careful_bad} with the chosen one"
        );
        assert!(careful_bad * 20 <= n, "{name}: {careful_bad} of {n}");
    }
}
