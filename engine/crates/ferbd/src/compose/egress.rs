//! The shell's only way off the Mac (S2, D28): FERB's egress proxy (`ferb-shell` `EgressProxy`), whose port the
//! sandbox profile names and whose per-process credentials the shell manager hands each command. Every destination
//! is the gateway's to allow (`_allow_destination`, `destination_allowed`; L2: any remote host for a process FERB
//! tracks, an owner's No final); every TLS hello's server name goes through the same check (S4). A command's first
//! connection to each host and its final network record are journaled (`shell.destination`, `shell.network`).
//!
//! The proxy is built with the shell, before the gateway exists; the gateway, the shell manager and the journal are
//! filled in here as the start makes them. Until the gateway is set every destination is refused.

use std::sync::{Arc, OnceLock, Weak};

use ferb_gateway::gateway::Gateway;
use ferb_shell::manager::ShellManager;
use ferb_shell::proxy::{EgressProxy, ProxyConfig};
use serde_json::{json, Map, Value};

use crate::basics::Journal;

/// The cells the proxy's callbacks read.
#[derive(Default)]
pub struct Egress {
    pub proxy: OnceLock<Arc<EgressProxy>>,
    pub gateway: OnceLock<Weak<Gateway>>,
    pub shells: OnceLock<Weak<ShellManager>>,
    pub journal: OnceLock<Arc<Journal>>,
    /// `sandbox.available()` at start: (contained, why).
    pub containment: OnceLock<(bool, String)>,
}

/// What a refusal says when the gateway can't answer (`_allow_destination`'s except).
pub const CANT_CHECK: &str = "FERB could not check this destination";
pub const CANT_CHECK_LOCAL: &str = "FERB could not check this local destination";

impl Egress {
    fn gateway(&self) -> Option<Arc<Gateway>> {
        self.gateway.get().and_then(Weak::upgrade)
    }

    /// `_allow_destination` / `_allow_local_destination`: the gateway's answer; anything that stops it refuses.
    pub fn allow(&self, proc: &str, host: &str, port: u16, local: bool) -> (bool, String) {
        let refused = || {
            (
                false,
                if local { CANT_CHECK_LOCAL } else { CANT_CHECK }.to_string(),
            )
        };
        match self.gateway() {
            Some(g) => g
                .destination_allowed(proc, host, port, local)
                .unwrap_or_else(|_| refused()),
            None => refused(),
        }
    }

    /// The task a running shell process belongs to (the gateway's `proc_task`).
    pub fn owner_of(&self, proc: &str) -> Option<String> {
        self.shells.get()?.upgrade()?.live_owner(proc)
    }

    fn note(&self, kind: &str, data: Value) {
        if let Some(j) = self.journal.get() {
            j.note(kind, data);
        }
    }

    /// The proxy over these cells (`EgressProxy(self._allow_destination, ...)`), serving on 127.0.0.1.
    pub fn build(
        self: &Arc<Self>,
        redactor: &Arc<ferb_sanitize::redact::Redactor>,
        protection: Arc<dyn ferb_shell::protection::Protection>,
    ) -> std::io::Result<Arc<EgressProxy>> {
        let (a, l, d) = (self.clone(), self.clone(), self.clone());
        let excluded = redactor.clone();
        let mut cfg = ProxyConfig::new(Arc::new(move |proc: &str, host: &str, port: u16| {
            a.allow(proc, host, port, false)
        }));
        cfg.redactor = Some(redactor.clone() as Arc<dyn ferb_shell::proxy::Secrets>);
        cfg.excluded = Arc::new(move |h: &str| excluded.excluded_host(h));
        cfg.inspect_sni = Arc::new(|_, _, _| true);
        cfg.local_access = Some(Arc::new(ferb_shell::local::LocalAccess::new(
            protection,
            Arc::new(ferb_shell::local::MacFacts),
        )));
        cfg.allow_local = Some(Arc::new(move |proc: &str, host: &str, port: u16| {
            l.allow(proc, host, port, true)
        }));
        cfg.on_destination = Some(Arc::new(move |proc: &str, host: &str, port: u16| {
            d.note(
                "shell.destination",
                json!({"task_id": d.owner_of(proc), "process": proc, "host": host, "port": port}),
            )
        }));
        let proxy = Arc::new(EgressProxy::new(cfg)?);
        let _ = self.proxy.set(proxy.clone());
        Ok(proxy)
    }

    /// `_shell_network`: a finished shell process's final network record.
    pub fn on_network(self: &Arc<Self>) -> ferb_shell::manager::OnNetwork {
        let me = self.clone();
        Arc::new(
            move |info: &ferb_shell::manager::ProcInfo, facts: Map<String, Value>| {
                let mut data = Map::new();
                data.insert("task_id".into(), json!(info.owner_task));
                data.insert("process".into(), json!(info.id));
                for (k, v) in facts {
                    data.insert(k, v);
                }
                me.note("shell.network", Value::Object(data));
            },
        )
    }

    /// `doctor.check`'s live proxy facts (`proxy.probe()`: expect 407; a dead serving thread is restarted).
    pub fn probe(&self) -> Map<String, Value> {
        self.proxy.get().map(|p| p.probe()).unwrap_or_default()
    }

    pub fn running(&self) -> bool {
        self.proxy.get().is_some()
    }

    pub fn close(&self) {
        if let Some(p) = self.proxy.get() {
            p.close();
        }
    }
}
