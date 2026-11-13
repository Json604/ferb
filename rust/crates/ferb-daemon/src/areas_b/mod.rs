//! Group B's handler areas (stage 1, part 12): `ferb/daemon/areas/{browsers, runtimes, about, sensing, egress, intro,
//! jev, notify, voice, desktop, environment, compose}.py` and the daemon modules they use (`about.py`,
//! `reports.py`, `task_preview.py`, `disturbance.py`, `persona.py`, `banners.py`, `events.py`).
//!
//! Every handler reads the daemon's shared state through [`core::Host`] and the engines it needs through its
//! area's `Deps` (the seams; tests give fakes). [`register`] puts each area's methods on the server; an area whose
//! `Deps` is None registers nothing (a test registers only what it replays).

pub mod core;

pub mod about;
pub mod banners;
pub mod browsers;
pub mod compose;
pub mod desktop;
pub mod disturbance;
pub mod egress;
pub mod environment;
pub mod events;
pub mod intro;
pub mod jev;
pub mod notify;
pub mod reports;
pub mod runtimes;
pub mod sensing;
pub mod task_preview;
pub mod voice;

use ferb_rpc::Server;

/// Each area's seams; `host` is what all of them share.
pub struct Deps {
    pub host: core::Host,
    pub about: Option<std::sync::Arc<about::Deps>>,
    pub browsers: Option<browsers::Deps>,
    pub egress: Option<egress::Deps>,
    pub intro: Option<std::sync::Arc<intro::Deps>>,
    pub jev: Option<jev::Deps>,
    pub notify: Option<notify::Deps>,
    pub runtimes: Option<runtimes::Deps>,
    pub sensing: Option<sensing::Deps>,
}

impl Deps {
    /// No area yet: set the ones to register.
    pub fn new(host: core::Host) -> Deps {
        Deps {
            host,
            about: None,
            browsers: None,
            egress: None,
            intro: None,
            jev: None,
            notify: None,
            runtimes: None,
            sensing: None,
        }
    }
}

/// Group B's methods on `server` (`areas/rpc.py` `_register_rpc`, the entries whose handlers live in these areas).
pub fn register(server: &mut Server, deps: &Deps) {
    let host = &deps.host;
    if let Some(d) = &deps.about {
        about::register(server, host, d);
    }
    if let Some(d) = &deps.browsers {
        browsers::register(server, host, d);
    }
    if let Some(d) = &deps.egress {
        egress::register(server, host, d);
    }
    if let Some(d) = &deps.intro {
        intro::register(server, host, d);
    }
    if let Some(d) = &deps.jev {
        jev::register(server, host, d);
    }
    if let Some(d) = &deps.notify {
        notify::register(server, host, d);
    }
    if let Some(d) = &deps.runtimes {
        runtimes::register(server, host, d);
    }
    if let Some(d) = &deps.sensing {
        sensing::register(server, host, d);
    }
}
