//! The answer models against pydantic's own answers (rust/fixtures/orchestrator/models.json, made by
//! make_models.py): every case's dump, `model_dump_json()` and `str(ValidationError)` byte for byte.
//!
//! decision: ORCH-630, ORCH-582, ORCH-545, ORCH-604

use ferb_context::pyvalue::{dumps, loads, Dumps, List, Py};
use ferb_orchestrator::validate::{dump_json_value, validate_by_name, ValidationError};

const URL: &str = "\n    For further information visit https://errors.pydantic.dev/2.13/v/";

/// make_models.py `unpack`: each item's link line put back after its `[type=…]` line.
fn unpack(packed: &str) -> String {
    let mut out = String::new();
    for (i, line) in packed.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(line);
        if let Some(rest) = line.strip_prefix("  ").filter(|_| line.ends_with(']')) {
            if let Some(at) = rest.rfind(" [type=") {
                let kind = &rest[at + 7..];
                if let Some(end) = kind.find(',') {
                    out.push_str(URL);
                    out.push_str(&kind[..end]);
                }
            }
        }
    }
    out
}

fn path_of(op: &[Py]) -> Vec<Py> {
    op[1].as_list().expect("a path").to_vec()
}

fn at<'a>(root: &'a mut Py, path: &[Py]) -> &'a mut Py {
    let mut node = root;
    for p in path {
        node = match (node, p) {
            (Py::Dict(d), Py::Str(k)) => d.0.get_mut(k).expect("key"),
            (Py::List(l), Py::Int(i)) => &mut l.0[i.parse::<usize>().expect("index")],
            _ => panic!("bad path"),
        };
    }
    node
}

/// make_models.py `apply`.
fn apply(mut root: Py, op: &[Py]) -> Py {
    let kind = op[0].as_str().expect("op kind");
    let path = path_of(op);
    if path.is_empty() {
        return match kind {
            "set" => op[2].clone(),
            "wrap" => Py::List(List(vec![root])),
            "dup" => {
                if let Py::List(l) = &mut root {
                    let i: usize = match &op[2] {
                        Py::Int(i) => i.parse().unwrap(),
                        _ => panic!(),
                    };
                    let copy = l.0[i].clone();
                    l.0.push(copy);
                }
                root
            }
            _ => panic!("op {kind} on the root"),
        };
    }
    let (last, parent_path) = path.split_last().expect("non-empty");
    if kind == "dup" {
        let target = at(&mut root, &path);
        if let Py::List(l) = target {
            let i: usize = match &op[2] {
                Py::Int(i) => i.parse().unwrap(),
                _ => panic!(),
            };
            let copy = l.0[i].clone();
            l.0.push(copy);
        }
        return root;
    }
    let parent = at(&mut root, parent_path);
    match (parent, last) {
        (Py::Dict(d), Py::Str(k)) => match kind {
            "del" => {
                d.0.shift_remove(k);
            }
            "set" => {
                d.0.insert(k.clone(), op[2].clone());
            }
            "wrap" => {
                let old = d.0.get(k).expect("key").clone();
                d.0.insert(k.clone(), Py::List(List(vec![old])));
            }
            "suffix" => {
                if let Some(Py::Str(s)) = d.0.get_mut(k) {
                    s.push_str(op[2].as_str().unwrap());
                }
            }
            _ => panic!("{kind}"),
        },
        (Py::List(l), Py::Int(i)) => {
            let i: usize = i.parse().unwrap();
            match kind {
                "del" => {
                    l.0.remove(i);
                }
                "set" => l.0[i] = op[2].clone(),
                "wrap" => {
                    let old = l.0[i].clone();
                    l.0[i] = Py::List(List(vec![old]));
                }
                "suffix" => {
                    if let Py::Str(s) = &mut l.0[i] {
                        s.push_str(op[2].as_str().unwrap());
                    }
                }
                _ => panic!("{kind}"),
            }
        }
        _ => panic!("bad path"),
    }
    root
}

#[test]
fn models_match_pydantic() {
    // FERB_MODELS_FIXTURE: a wider corpus make_models.py wrote elsewhere (FERB_MODELS_OUT), for checking
    let path = std::env::var("FERB_MODELS_FIXTURE").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/orchestrator/models.json"
        )
        .into()
    });
    let text = std::fs::read_to_string(path).expect("models.json");
    let fixture = loads(&text).expect("fixture JSON");
    let seeds = fixture
        .get("seeds")
        .and_then(Py::as_list)
        .expect("seeds")
        .to_vec();
    let cases = fixture.get("cases").and_then(Py::as_list).expect("cases");
    let (mut differ, mut checked) = (Vec::new(), 0);
    for (n, case) in cases.iter().enumerate() {
        let model = case.get("model").and_then(Py::as_str).unwrap();
        let mode = case.get("mode").and_then(Py::as_str).unwrap();
        let (value, input_text) = match case.get("input").and_then(Py::as_str) {
            Some(t) => (loads(t).unwrap_or(Py::None), t.to_string()),
            None => {
                let seed = match case.get("seed") {
                    Some(Py::Int(i)) => i.parse::<usize>().unwrap(),
                    _ => panic!(),
                };
                let mut value = seeds[seed].clone();
                for op in case.get("ops").and_then(Py::as_list).unwrap() {
                    value = apply(value, op.as_list().unwrap());
                }
                let ascii = matches!(case.get("ascii"), Some(Py::Bool(true)));
                let text = dumps(
                    &value,
                    Dumps {
                        ascii,
                        compact: false,
                        sort_keys: false,
                    },
                );
                (value, text)
            }
        };
        let got = if mode == "json" {
            validate_by_name(model, &Py::None, Some(&input_text))
        } else {
            validate_by_name(model, &value, None)
        };
        checked += 1;
        let problem = compare(model, case, &got);
        if let Some(p) = problem {
            differ.push(format!(
                "case {n} ({model}, {mode}): {p}\n  input: {input_text}"
            ));
        }
    }
    for d in differ.iter().take(40) {
        eprintln!(
            "{}\n",
            ferb_context::pyvalue::str_repr(d)
                .chars()
                .take(1200)
                .collect::<String>()
        );
    }
    assert!(
        differ.is_empty(),
        "{} of {checked} cases differ",
        differ.len()
    );
    assert!(checked > 5000, "{checked} cases");
}

fn compare(model: &str, case: &Py, got: &Result<Py, ValidationError>) -> Option<String> {
    if let Some(ok) = case.get("ok") {
        let dump = match got {
            Ok(d) => d,
            Err(e) => return Some(format!("expected success, got {:?} / {e}", e.raised())),
        };
        let text = dumps(dump, Dumps::UTF8);
        if let Some(Py::Str(want)) = ok.get("dump") {
            if &text != want {
                return Some(format!("dump\n  want {want}\n  got  {text}"));
            }
        } else if !same_hash(&text, ok.get("dump_sha"), ok.get("dump_len")) {
            return Some(format!("dump (hash)\n  got  {text}"));
        }
        let json = dump_json_value(dump);
        if !same_hash(&json, ok.get("json_sha"), ok.get("json_len")) {
            return Some(format!("model_dump_json (hash)\n  got  {json}"));
        }
        if model == "TaskSpec" {
            let mut noplan = dump.clone();
            if let Py::Dict(d) = &mut noplan {
                d.0.shift_remove("plan");
            }
            let want = ok.get("json_noplan").and_then(Py::as_str).unwrap();
            let got = dump_json_value(&noplan);
            if got != want {
                return Some(format!("json_noplan\n  want {want}\n  got  {got}"));
            }
        }
        return None;
    }
    if let Some(Py::Str(packed)) = case.get("error") {
        let want = unpack(packed);
        return match got {
            Ok(d) => Some(format!("expected an error, got {}", dumps(d, Dumps::UTF8))),
            Err(e) if e.raised().is_some() => {
                Some(format!("expected an error, raised {:?}", e.raised()))
            }
            Err(e) => {
                let text = e.to_string();
                (text != want).then(|| {
                    let (w, g): (Vec<&str>, Vec<&str>) =
                        (want.lines().collect(), text.lines().collect());
                    let at = w
                        .iter()
                        .zip(g.iter())
                        .position(|(a, b)| a != b)
                        .unwrap_or(w.len().min(g.len()));
                    format!(
                        "error line {at}\n  want {}\n  got  {}",
                        w.get(at).unwrap_or(&"<none>"),
                        g.get(at).unwrap_or(&"<none>")
                    )
                })
            }
        };
    }
    let raised = case.get("raised").and_then(Py::as_list).expect("raised");
    let (exc, msg) = (raised[0].as_str().unwrap(), raised[1].as_str().unwrap());
    match got {
        Err(e) => match e.raised() {
            Some(r) if r.exception == exc && r.message == msg => None,
            other => Some(format!("expected {exc}: {msg}, got {other:?} / {e}")),
        },
        Ok(d) => Some(format!(
            "expected {exc}: {msg}, got {}",
            dumps(d, Dumps::UTF8)
        )),
    }
}

fn same_hash(text: &str, sha: Option<&Py>, len: Option<&Py>) -> bool {
    let want_len: usize = match len {
        Some(Py::Int(i)) => i.parse().unwrap(),
        _ => return false,
    };
    text.chars().count() == want_len
        && sha.and_then(Py::as_str) == Some(&sha256_hex(text.as_bytes())[..24])
}

/// SHA-256 (FIPS 180-4), for the fixture's hashed texts.
fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for (word, bytes) in w.iter_mut().zip(chunk.chunks(4)) {
            *word = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut v = h;
        for (k, word) in K.iter().zip(w.iter()) {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(*k)
                .wrapping_add(*word);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [
                t1.wrapping_add(t2),
                v[0],
                v[1],
                v[2],
                v[3].wrapping_add(t1),
                v[4],
                v[5],
                v[6],
            ];
        }
        for (a, b) in h.iter_mut().zip(v.iter()) {
            *a = a.wrapping_add(*b);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

#[test]
fn sha256_known() {
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
