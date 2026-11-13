//! `ferb`: FERB.app's `Contents/Resources/bin/ferb`, the program FERB.app runs for its internal commands and the
//! `ferb` link's developer toolbox (`ferbd::run::cli`). The daemon, the tool bridge and the grant probe are not here:
//! they run as FERB.app's agent, `ferbd` (`ferbd::host`).

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(ferbd::run::cli(&args));
}
