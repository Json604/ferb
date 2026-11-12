//! Embeds Python's migration files, the one copy both daemons apply (decision: ENG-4).
//! Writes `$OUT_DIR/migrations.rs`: `EMBEDDED`, each `NNNN_*.sql` of `ferb/store/migrations` as
//! (file name, exact bytes), sorted by name as `ferb/store/db.py:158` sorts them.

use std::{env, fs, path::PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let folder = manifest
        .join("../../../ferb/store/migrations")
        .canonicalize()
        .expect("ferb/store/migrations is missing");
    println!("cargo:rerun-if-changed={}", folder.display());
    let mut names: Vec<String> = fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| {
            let b = n.as_bytes();
            b.len() >= 9
                && b[..4].iter().all(u8::is_ascii_digit)
                && b[4] == b'_'
                && n.ends_with(".sql")
        })
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no migrations in {}", folder.display());
    let mut out = String::from("pub(crate) static EMBEDDED: &[(&str, &[u8])] = &[\n");
    for name in &names {
        let path = folder.join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        out += &format!(
            "    ({name:?}, include_bytes!({:?})),\n",
            path.display().to_string()
        );
    }
    out += "];\n";
    let dest = PathBuf::from(env::var("OUT_DIR").unwrap()).join("migrations.rs");
    fs::write(dest, out).unwrap();
}
