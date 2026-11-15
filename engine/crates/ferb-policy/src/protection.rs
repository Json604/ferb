//! policy/protection.py: FERB's protection lists (S2, SQ3, SQ5, D27, D31): where FERB's shell and file tools never
//! read or write, what code always protects (FERB's own state, FERB.app, FERB's service definitions), and which
//! rules the escape hatch may lift. Paths and ids only (rule 5).
//!
//! A path is checked as typed, as the OS reaches it now and as the kernel names the file it reaches (a name-only
//! open and `F_GETPATH`, through the [`Files`] seam), against each entry as written, as resolved when FERB started
//! and in its firmlinked spelling. Entries are never opened; an entry in another app's container is not even looked
//! at.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::ppath::{self, FileKind, Files, ResolveError};
use crate::{norm, py};

const PLAN_NEVER_READ: &[&str] = &[
    "~/Library/Keychains",
    "/Library/Keychains",
    "~/Library/Messages",
    "~/Library/Mail",
    "~/Library/Cookies",
    "~/Library/Group Containers/group.com.apple.usernoted",
    "~/Library/Application Support/1Password",
    "~/Library/Group Containers/2BUA8C4S2C.com.1password",
    "~/Library/Application Support/Bitwarden",
    "~/.ssh",
    "~/.aws",
    "~/.config/gh",
    "~/.netrc",
    "~/.npmrc",
    "~/.docker",
];
/// GATE-224: every browser's data folder, from the one browser list (ferb/act/mac/browsers.py `data_folders`,
/// recorded by the fixtures).
pub const BROWSER_STORES: &[&str] = &[
    "~/Library/Safari",
    "~/Library/Containers/com.apple.Safari",
    "~/Library/SafariTechnologyPreview",
    "~/Library/Containers/com.apple.SafariTechnologyPreview",
    "~/Library/Application Support/BraveSoftware",
    "~/Library/Application Support/Arc",
    "~/Library/Application Support/Dia",
    "~/Library/Application Support/com.operasoftware.OperaGX",
    "~/Library/Application Support/Firefox",
    "~/Library/Application Support/Orion",
    "~/Library/Containers/com.duckduckgo.macos.browser",
    "~/Library/Application Support/librewolf",
    "~/Library/Application Support/Comet",
    "~/Library/Application Support/Yandex",
    "~/Library/Application Support/Floorp",
    "~/Library/Application Support/MullvadBrowser",
    "~/Library/Application Support/TorBrowser-Data",
    "~/Library/Application Support/zen",
    "~/Library/Application Support/Waterfox",
    "~/Library/Application Support/Thorium",
    "~/Library/Application Support/Google/Chrome",
    "~/Library/Application Support/Google/Chrome Beta",
    "~/Library/Application Support/Google/Chrome Dev",
    "~/Library/Application Support/Google/Chrome Canary",
    "~/Library/Application Support/Chromium",
    "~/Library/Application Support/Microsoft Edge",
    "~/Library/Application Support/Microsoft Edge Beta",
    "~/Library/Application Support/Microsoft Edge Dev",
    "~/Library/Application Support/Microsoft Edge Canary",
    "~/Library/Application Support/Vivaldi",
    "~/Library/Application Support/com.operasoftware.Opera",
];
pub const EXTRA_NEVER_READ: &[&str] = &[
    "~/.gnupg",
    "~/.kube",
    "~/.config/gcloud",
    "~/.azure",
    "~/.git-credentials",
    "~/.pypirc",
    "~/.cargo/credentials.toml",
];
const OFFERED_BROWSERS: &[&str] = &[
    "~/Library/Safari",
    "~/Library/Application Support/Google/Chrome",
    "~/Library/Application Support/BraveSoftware",
    "~/Library/Application Support/Chromium",
    "~/Library/Application Support/Microsoft Edge",
    "~/Library/Application Support/Arc",
    "~/Library/Application Support/Vivaldi",
    "~/Library/Application Support/Firefox",
];
pub const NEW_DEFAULTS: &str = "config.defaults_added";
pub const DEFAULT_NEVER_WRITE: &[&str] = &[
    "~/.ssh",
    "~/Library/Keychains",
    "~/Library/LaunchAgents",
    "/Library/LaunchAgents",
    "/Library/LaunchDaemons",
    "~/.zshrc",
    "~/.zprofile",
    "~/.zshenv",
    "~/.zlogin",
    "~/.bash_profile",
    "~/.bashrc",
    "~/.profile",
    "/System",
    "/Library",
    "/Applications",
];
pub const DEFAULT_TERMINAL_APPS: &[&str] = &[
    "com.apple.Terminal",
    "com.googlecode.iterm2",
    "dev.warp.Warp-Stable",
    "dev.warp.Warp-Preview",
    "com.mitchellh.ghostty",
    "org.alacritty",
    "net.kovidgoyal.kitty",
    "com.github.wez.wezterm",
    "co.zeit.hyper",
    "org.tabby",
    "com.termius-dmg.mac",
    "com.termius.mac",
    "com.raphaelamorim.rio",
    "org.contourterminal.contour",
];
pub const FERB_STATE: &str = "ferb_state";
pub const NETWORK: &str = "network";
pub const ADMIN: &str = "admin";
pub const TRUSTD: &str = "trustd";
pub const FERB_APP: &str = "ferb_app";
pub const FERB_SERVICE: &str = "ferb_service";
pub const SERVICE_LABELS: &[&str] = &["com.ferb.daemon", "com.ferb.FERB.login"];
pub const BUNDLE_SERVICE: &str = "Contents/Library/LaunchAgents/com.ferb.daemon.plist";
pub const NEVER_LIFTED: &[&str] = &[FERB_STATE, NETWORK, FERB_APP, FERB_SERVICE];
pub const WORKSPACE: &str = "workspace";
pub const UPLOADS: &str = "uploads";
pub const READ: &str = "read";
pub const WRITE: &str = "write";
pub const REMOVE: &str = "remove";
const REMOVES: &[&str] = &[REMOVE, "trash", "delete_permanent"];
const DATA_VOLUME: &str = "/System/Volumes/Data";
const VOLFS: &str = "/.vol";
const APP_DATA: &[&str] = &["Library/Containers", "Library/Group Containers"];

/// `DEFAULT_NEVER_READ`.
pub fn default_never_read() -> Vec<&'static str> {
    PLAN_NEVER_READ
        .iter()
        .chain(BROWSER_STORES)
        .chain(EXTRA_NEVER_READ)
        .copied()
        .collect()
}

/// `NEVER_READ_OFFERED`: the never-read list as config.toml got it before 2026-11-01.
pub fn never_read_offered() -> Vec<&'static str> {
    PLAN_NEVER_READ
        .iter()
        .chain(EXTRA_NEVER_READ)
        .chain(OFFERED_BROWSERS)
        .copied()
        .collect()
}

/// `BadProtection`: [protection] or FERB's workspace isn't as FERB can enforce it; FERB doesn't start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadProtection(pub String);

impl std::fmt::Display for BadProtection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BadProtection {}

/// protection.py:110 `Entry`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub path: String,
    pub shown: String,
    pub typed: String,
    pub kernel: Option<String>,
    pub app_data: bool,
}

/// An owner-approved TCP port and the resolved executable its listener must run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalPort {
    pub port: u16,
    pub executable: String,
}

struct Ctx<'a> {
    files: &'a dyn Files,
}

impl Ctx<'_> {
    fn absolute(&self, p: &str) -> String {
        if ppath::is_absolute(p) {
            ppath::clean(p)
        } else {
            ppath::div(&self.files.cwd().unwrap_or_else(|_| "/".into()), p)
        }
    }

    fn typed(&self, p: &str) -> String {
        ppath::clean(&ppath::normpath(&self.absolute(p)))
    }

    /// `files.real_path(str(_absolute(path)), "/")`.
    fn real_path(&self, p: &str) -> Result<String, ResolveError> {
        let raw = self.absolute(p);
        if ppath::exists(self.files, &raw).map_err(ResolveError::Os)? {
            ppath::resolve(self.files, &raw)
        } else {
            Ok(ppath::div(
                &ppath::resolve(self.files, &ppath::parent(&raw))?,
                &ppath::name(&raw),
            ))
        }
    }

    fn real(&self, p: &str) -> String {
        self.real_path(p).unwrap_or_else(|_| self.typed(p))
    }

    fn opened_name(&self, p: &str, folder: bool, seen: &ppath::Stat) -> Option<String> {
        let (st, name) = self.files.opened_name(p, folder).ok()?;
        (st.dev == seen.dev && st.ino == seen.ino).then_some(name)
    }

    fn kernel(&self, path: &str) -> Option<String> {
        let mut tail: Vec<String> = Vec::new();
        let mut p = ppath::clean(&ppath::realpath(self.files, &self.absolute(path)).ok()?);
        loop {
            let st = match self.files.lstat(&p) {
                Ok(st) => Some(st),
                Err(e) if is_missing(&e) => None,
                Err(_) => return None,
            };
            let named = match st {
                Some(s) if s.kind == FileKind::Dir => self.opened_name(&p, true, &s),
                Some(s) if s.kind == FileKind::File => self.opened_name(&p, false, &s),
                _ => None,
            };
            if let Some(named) = named {
                let mut out = named;
                for t in tail.iter().rev() {
                    out = ppath::div(&out, t);
                }
                return Some(ppath::clean(&ppath::normpath(&out)));
            }
            if st.is_some_and(|s| s.kind == FileKind::Dir) || p == ppath::parent(&p) {
                return None;
            }
            tail.push(ppath::name(&p));
            p = ppath::parent(&p);
        }
    }

    fn firmlinked(&self, p: &str) -> Option<String> {
        let typed = self.typed(p);
        if ppath::is_relative_to(&typed, DATA_VOLUME) && typed != DATA_VOLUME {
            Some(ppath::div("/", &ppath::relative_to(&typed, DATA_VOLUME)))
        } else {
            None
        }
    }

    fn forms(&self, p: &str) -> Vec<String> {
        let mut out = vec![self.typed(p), self.real(p)];
        if let Some(k) = self.kernel(p) {
            out.push(k);
        }
        if let Some(f) = self.firmlinked(&self.typed(p)) {
            out.push(f);
        }
        dedup(out)
    }

    fn unmapped(&self, p: &str) -> bool {
        if !ppath::is_relative_to(&self.typed(p), VOLFS) {
            return false;
        }
        self.kernel(p)
            .is_none_or(|k| ppath::is_relative_to(&k, VOLFS))
    }

    fn folder_or_missing(&self, p: &str) -> bool {
        match (ppath::is_dir(self.files, p), ppath::exists(self.files, p)) {
            (Ok(d), Ok(e)) => d || !e,
            _ => true,
        }
    }
}

fn is_missing(e: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        matches!(
            e.raw_os_error(),
            Some(libc::ENOENT) | Some(libc::ENOTDIR) | Some(libc::EACCES) | Some(libc::EPERM)
        ) || e.kind() == std::io::ErrorKind::NotFound
    }
    #[cfg(not(unix))]
    {
        e.kind() == std::io::ErrorKind::NotFound
    }
}

fn dedup(items: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for i in items {
        if !out.contains(&i) {
            out.push(i);
        }
    }
    out
}

/// `_key`: a path as a Mac volume compares it (case and Unicode normalization ignored).
fn key(p: &str) -> Vec<String> {
    ppath::parts(p)
        .iter()
        .map(|part| py::casefold(&norm::nfc(part)))
        .collect()
}

fn under(p: &str, root: &str) -> bool {
    let (a, b) = (key(p), key(root));
    a.len() >= b.len() && a[..b.len()] == b[..]
}

fn entry_path(home: &str, entry: &str) -> String {
    if entry == "~" || entry.starts_with("~/") {
        return ppath::div(home, entry.get(2..).unwrap_or(""));
    }
    if ppath::is_absolute(entry) {
        ppath::clean(entry)
    } else {
        ppath::div(home, entry)
    }
}

fn strings(value: &Value, key: &str, what: &str) -> Result<Vec<String>, BadProtection> {
    let bad = || {
        BadProtection(format!(
            "protection.{key} in config.toml must be a list of {what}, each in quotes"
        ))
    };
    let items = value.as_array().ok_or_else(bad)?;
    items
        .iter()
        .map(|v| match v {
            Value::String(s) if !py::strip(s).is_empty() => Ok(s.clone()),
            _ => Err(bad()),
        })
        .collect()
}

fn local_ports(value: &Value, ctx: &Ctx<'_>) -> Result<Vec<LocalPort>, BadProtection> {
    let error = || {
        BadProtection(
            "protection.local_ports in config.toml must be a list of {port = <TCP port>, executable = <absolute \
             executable path>} entries, with one executable per port"
                .to_string(),
        )
    };
    let items = value.as_array().ok_or_else(error)?;
    let mut ports: Vec<LocalPort> = Vec::new();
    for item in items {
        let map = item.as_object().ok_or_else(error)?;
        if map.len() != 2 || !map.contains_key("port") || !map.contains_key("executable") {
            return Err(error());
        }
        let port = match &map["port"] {
            Value::Number(n) => n
                .as_i64()
                .filter(|p| (1..=65535).contains(p))
                .ok_or_else(error)?,
            _ => return Err(error()),
        };
        let executable = match &map["executable"] {
            Value::String(s) if !s.contains('\0') && ppath::is_absolute(s) => s.clone(),
            _ => return Err(error()),
        };
        let pin = LocalPort {
            port: port as u16,
            executable: ppath::clean(
                &ppath::realpath(ctx.files, &executable).unwrap_or(executable),
            ),
        };
        match ports.iter_mut().find(|p| p.port == pin.port) {
            Some(existing) if *existing != pin => return Err(error()),
            Some(_) => {}
            None => ports.push(pin),
        }
    }
    Ok(ports)
}

/// `workspace_problem`: why FERB must not start with this workspace, else None.
pub fn workspace_problem(ferb_home: &str, files: &dyn Files) -> Option<String> {
    let ctx = Ctx { files };
    let ws = ppath::div(ferb_home, WORKSPACE);
    if ppath::is_symlink(files, &ws).unwrap_or(false) {
        let target = files.readlink(&ws).unwrap_or_default();
        return Some(format!(
            "FERB's workspace ({ws}) is a link to {target}, not a folder. A link there could open FERB's own files to \
             its tools, so FERB won't start until you move the link away (FERB then makes a fresh workspace)."
        ));
    }
    if ppath::exists(files, &ws).unwrap_or(false)
        && (!ppath::is_dir(files, &ws).unwrap_or(false)
            || ctx.real(&ws) != ppath::div(&ctx.real(ferb_home), WORKSPACE))
    {
        return Some(format!(
            "FERB's workspace ({ws}) is not a plain folder in FERB's home, so FERB won't start until you move it away \
             (FERB then makes a fresh workspace)."
        ));
    }
    None
}

/// protection.py:350 `Protection`.
pub struct Protection<'a> {
    files: &'a dyn Files,
    pub home: String,
    app_data_roots: Vec<String>,
    pub ferb_home: String,
    pub workspace: String,
    pub uploads: String,
    state_roots: Vec<(String, String, String)>,
    pub reads: Vec<Entry>,
    pub writes: Vec<Entry>,
    pub terminal_apps: Vec<String>,
    pub service_files: Vec<String>,
    service_roots: Vec<String>,
    pub local_ports: Vec<LocalPort>,
    pub app_bundle: Option<String>,
    app_roots: Vec<String>,
}

/// The [protection] values as config.toml gives them (JSON values of the TOML).
pub struct Lists<'v> {
    pub never_read: &'v Value,
    pub never_write: &'v Value,
    pub terminal_apps: &'v Value,
    pub local_ports: &'v Value,
}

/// `load`: the daemon's lists after checking the workspace. `app_bundle`: FERB.app the daemon runs from, else the
/// installed one (`paths.app_bundle() or paths.installed_app()`).
pub fn load<'a>(
    lists: &Lists<'_>,
    home: &str,
    ferb_home: &str,
    app_bundle: Option<&str>,
    files: &'a dyn Files,
) -> Result<Protection<'a>, BadProtection> {
    if let Some(problem) = workspace_problem(ferb_home, files) {
        return Err(BadProtection(problem));
    }
    Protection::new(home, ferb_home, lists, app_bundle, files)
}

impl<'a> Protection<'a> {
    pub fn new(
        home: &str,
        ferb_home: &str,
        lists: &Lists<'_>,
        app_bundle: Option<&str>,
        files: &'a dyn Files,
    ) -> Result<Protection<'a>, BadProtection> {
        let ctx = Ctx { files };
        let home = ppath::clean(home);
        let typed_roots: Vec<String> = APP_DATA
            .iter()
            .map(|f| ctx.typed(&ppath::div(&home, f)))
            .collect();
        let mut roots = typed_roots.clone();
        roots.extend(typed_roots.iter().map(|t| ctx.real(t)));
        let app_data_roots = dedup(roots);
        let real_home = ctx.real(ferb_home);
        let workspace = ppath::div(&real_home, WORKSPACE);
        let uploads = ppath::div(&workspace, UPLOADS);
        let typed = ctx.typed(ferb_home);
        let mut homes = vec![real_home.clone(), typed];
        if let Some(k) = ctx.kernel(ferb_home) {
            homes.push(k);
        }
        let mut state_roots: Vec<(String, String, String)> = Vec::new();
        for h in homes {
            let ws = ppath::div(&h, WORKSPACE);
            let triple = (h.clone(), ws.clone(), ppath::div(&ws, UPLOADS));
            if !state_roots.contains(&triple) {
                state_roots.push(triple);
            }
        }
        let mut p = Protection {
            files,
            home: home.clone(),
            app_data_roots,
            ferb_home: real_home,
            workspace,
            uploads,
            state_roots,
            reads: Vec::new(),
            writes: Vec::new(),
            terminal_apps: Vec::new(),
            service_files: Vec::new(),
            service_roots: Vec::new(),
            local_ports: Vec::new(),
            app_bundle: app_bundle.map(ppath::clean),
            app_roots: Vec::new(),
        };
        let reads = strings(lists.never_read, "never_read", "paths")?;
        p.reads = p.entries("read", &reads);
        let writes = strings(lists.never_write, "never_write", "paths")?;
        p.writes = p.entries("write", &writes);
        let mut apps: Vec<String> = Vec::new();
        for a in strings(lists.terminal_apps, "terminal_apps", "app bundle ids")? {
            let low = py::lower(&a);
            if !apps.contains(&low) {
                apps.push(low);
            }
        }
        p.terminal_apps = apps;
        let mut service_files: Vec<String> = SERVICE_LABELS
            .iter()
            .map(|l| {
                ppath::div(
                    &ppath::div(&ppath::div(&home, "Library"), "LaunchAgents"),
                    &format!("{l}.plist"),
                )
            })
            .collect();
        if let Some(b) = &p.app_bundle {
            service_files.push(ppath::div(b, BUNDLE_SERVICE));
        }
        let mut service_roots = Vec::new();
        for s in &service_files {
            let real = ctx.real(s);
            service_roots.push(ctx.typed(s));
            service_roots.push(real.clone());
            if let Some(f) = ctx.firmlinked(&real) {
                service_roots.push(f);
            }
        }
        p.service_files = service_files;
        p.service_roots = dedup(service_roots);
        p.local_ports = local_ports(lists.local_ports, &ctx)?;
        p.app_roots = match &p.app_bundle {
            Some(b) => ctx.forms(b),
            None => Vec::new(),
        };
        Ok(p)
    }

    fn ctx(&self) -> Ctx<'a> {
        Ctx { files: self.files }
    }

    fn shown(&self, p: &str) -> String {
        let ctx = self.ctx();
        let (p, h) = (ctx.typed(p), ctx.typed(&self.home));
        if p == h {
            return "~".to_string();
        }
        if ppath::is_relative_to(&p, &h) {
            format!("~/{}", ppath::relative_to(&p, &h))
        } else {
            p
        }
    }

    fn app_data_split(&self, p: &str) -> Option<(String, String)> {
        let typed = self.ctx().typed(p);
        let tparts = ppath::parts(&typed);
        for root in &self.app_data_roots {
            let rparts = ppath::parts(root);
            if under(&typed, root) && tparts.len() > rparts.len() {
                return Some((root.clone(), ppath::from_parts(&tparts[rparts.len()..])));
            }
        }
        None
    }

    fn entries(&self, prefix: &str, items: &[String]) -> Vec<Entry> {
        let ctx = self.ctx();
        let mut out: Vec<Entry> = Vec::new();
        for item in items {
            let raw = entry_path(&self.home, item);
            let shown = self.shown(&raw);
            let real = self.entry_real(&raw);
            let id = format!("{prefix}:{shown}");
            let entry = Entry {
                id: id.clone(),
                kernel: ctx.firmlinked(&real).or_else(|| ctx.firmlinked(&raw)),
                path: real,
                shown,
                typed: ctx.typed(&raw),
                app_data: self.app_data(&raw),
            };
            if !out.iter().any(|e| e.id == id) {
                out.push(entry);
            }
        }
        out
    }

    /// Whether `path` is another app's container or inside one: never looked at.
    pub fn app_data(&self, p: &str) -> bool {
        self.app_data_split(p).is_some()
    }

    fn entry_real(&self, p: &str) -> String {
        match self.app_data_split(p) {
            Some((root, rest)) => ppath::div(&self.ctx().real(&root), &rest),
            None => self.ctx().real(p),
        }
    }

    /// `forms_now`: a listed path as written and as written resolved now (for the shell profile).
    pub fn forms_now(&self, p: &str) -> Vec<String> {
        let now = if self.app_data(p) {
            self.entry_real(p)
        } else {
            ppath::clean(&ppath::realpath(self.files, p).unwrap_or_else(|_| p.to_string()))
        };
        dedup(vec![ppath::clean(p), now])
    }

    fn forms(&self, p: &str) -> Vec<String> {
        let full = if ppath::is_absolute(p) {
            p.to_string()
        } else {
            ppath::div(&self.workspace, p)
        };
        self.ctx().forms(&full)
    }

    fn state(&self, forms: &[String], removes: bool) -> bool {
        let ctx = self.ctx();
        if forms.iter().any(|p| ctx.unmapped(p)) {
            return true;
        }
        forms.iter().any(|p| {
            self.state_roots.iter().any(|(home, ws, up)| {
                (under(p, home) && !under(p, ws))
                    || under(p, up)
                    || (removes && (under(home, p) || under(up, p)))
            })
        })
    }

    fn app(&self, forms: &[String], removes: bool) -> bool {
        forms.iter().any(|p| {
            self.app_roots
                .iter()
                .any(|app| under(p, app) || (removes && under(app, p)))
        })
    }

    fn roots(e: &Entry) -> impl Iterator<Item = &String> {
        [Some(&e.path), Some(&e.typed), e.kernel.as_ref()]
            .into_iter()
            .flatten()
    }

    fn hit(entries: &[Entry], forms: &[String], removes: bool) -> Option<String> {
        entries
            .iter()
            .find(|e| {
                forms.iter().any(|p| {
                    Self::roots(e).any(|root| under(p, root) || (removes && under(root, p)))
                })
            })
            .map(|e| e.id.clone())
    }

    fn hits(entries: &[Entry], forms: &[String], removes: bool) -> Vec<String> {
        entries
            .iter()
            .filter(|e| {
                forms.iter().any(|p| {
                    Self::roots(e).any(|root| under(p, root) || (removes && under(root, p)))
                })
            })
            .map(|e| e.id.clone())
            .collect()
    }

    pub fn ferb_state(&self, p: &str) -> bool {
        self.state(&self.forms(p), false)
    }

    pub fn read_denied(&self, p: &str) -> Option<String> {
        let forms = self.forms(p);
        if self.state(&forms, false) {
            Some(FERB_STATE.to_string())
        } else {
            Self::hit(&self.reads, &forms, false)
        }
    }

    pub fn write_denied(&self, p: &str) -> Option<String> {
        let forms = self.forms(p);
        if let Some(r) = self.read_denied(p) {
            return Some(r);
        }
        if self.app(&forms, false) {
            Some(FERB_APP.to_string())
        } else {
            Self::hit(&self.writes, &forms, false)
        }
    }

    fn service(&self, forms: &[String], removes: bool) -> bool {
        forms.iter().any(|p| {
            self.service_roots
                .iter()
                .any(|s| under(p, s) || (removes && under(s, p)))
        })
    }

    /// `fs_rules`: every rule that closes `path` to FERB's own tools that name files (`access`: read, write,
    /// remove, metadata; trash and permanent delete count as remove).
    pub fn fs_rules(&self, p: &str, access: &str) -> Vec<String> {
        let forms = self.forms(p);
        let removes = REMOVES.contains(&access);
        if self.state(&forms, removes) {
            return vec![FERB_STATE.to_string()];
        }
        if access == READ || access == "metadata" {
            return if access == "metadata" {
                Vec::new()
            } else {
                Self::hits(&self.reads, &forms, false)
            };
        }
        if self.service(&forms, false) {
            return vec![FERB_SERVICE.to_string()];
        }
        if self.app(&forms, removes) {
            return vec![FERB_APP.to_string()];
        }
        if self.service(&forms, removes) {
            return vec![FERB_SERVICE.to_string()];
        }
        let mut out = Self::hits(&self.reads, &forms, removes);
        out.extend(Self::hits(&self.writes, &forms, removes));
        dedup(out)
    }

    /// `fs_denied`: the first rule `fs_rules` names, or None.
    pub fn fs_denied(&self, p: &str, access: &str) -> Option<String> {
        self.fs_rules(p, access).into_iter().next()
    }

    pub fn liftable(&self, rule_id: &str) -> bool {
        !NEVER_LIFTED.contains(&rule_id)
            && (rule_id == ADMIN
                || rule_id == TRUSTD
                || self
                    .reads
                    .iter()
                    .chain(&self.writes)
                    .any(|e| e.id == rule_id))
    }

    pub fn entry(&self, rule_id: &str) -> Option<&Entry> {
        self.reads
            .iter()
            .chain(&self.writes)
            .find(|e| e.id == rule_id)
    }

    fn entry_pairs(&self, entries: &[Entry]) -> Vec<(String, bool)> {
        let ctx = self.ctx();
        entries
            .iter()
            .flat_map(|e| {
                dedup(vec![e.path.clone(), e.typed.clone()])
                    .into_iter()
                    .map(|p| {
                        let dir = e.app_data || ctx.folder_or_missing(&p);
                        (p, dir)
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// (path, is_dir) pairs of the never-read list, resolved and as written, for the shell profile.
    pub fn read_entries(&self) -> Vec<(String, bool)> {
        self.entry_pairs(&self.reads)
    }

    /// (path, is_dir) pairs of the never-write list, resolved and as written, for the shell profile.
    pub fn write_entries(&self) -> Vec<(String, bool)> {
        self.entry_pairs(&self.writes)
    }

    /// FERB.app, resolved and as typed.
    pub fn app_entries(&self) -> Vec<String> {
        self.app_roots.clone()
    }

    /// Every folder above a listed entry, above FERB's upload staging and above FERB.app (the root left out).
    pub fn guarded_parents(&self) -> Vec<String> {
        let mut roots: Vec<String> = self
            .reads
            .iter()
            .chain(&self.writes)
            .flat_map(|e| [e.path.clone(), e.typed.clone()])
            .collect();
        roots.extend(self.state_roots.iter().map(|(_, _, up)| up.clone()));
        roots.extend(self.app_roots.iter().cloned());
        roots.extend(self.service_roots.iter().cloned());
        let mut set: BTreeMap<Vec<String>, String> = BTreeMap::new();
        for r in roots {
            for parent in ppath::parents(&r) {
                if parent != ppath::parent(&parent) {
                    set.insert(ppath::sort_key(&parent), parent);
                }
            }
        }
        set.into_values().collect()
    }

    /// `rules`: every rule, for shell.rules and doctor.
    pub fn rules(&self) -> Vec<Value> {
        let mut out = vec![
            json!({"id": FERB_STATE, "kind": "read and write", "path": "FERB's own files (~/.ferb except its workspace, and the workspace's uploads)", "liftable": false}),
            json!({"id": NETWORK, "kind": "network", "path": "no direct connection off this Mac, and on it only to servers started inside FERB's shell, plus owner-approved executable-pinned local ports through FERB's proxy; the proxy records each host and is the way out (shell.rules says whether it is running)", "liftable": false}),
        ];
        if let Some(b) = &self.app_bundle {
            out.push(json!({"id": FERB_APP, "kind": "write", "path": b, "liftable": false}));
        }
        let shown: Vec<String> = self.service_files.iter().map(|f| self.shown(f)).collect();
        out.push(json!({"id": FERB_SERVICE, "kind": "write", "path": format!("FERB's own background-service definitions ({})", shown.join(", ")), "liftable": false}));
        out.push(json!({"id": ADMIN, "kind": "admin", "path": "a command that needs your Mac password (sudo)", "liftable": true}));
        out.push(json!({"id": TRUSTD, "kind": "service", "path": "macOS certificate trust evaluation (trustd): enabling it lets the certificate service fetch certificate URLs outside FERB's proxy; Go TLS may need it", "liftable": true}));
        out.extend(
            self.reads
                .iter()
                .map(|e| json!({"id": e.id, "kind": "read", "path": e.shown, "liftable": true})),
        );
        out.extend(
            self.writes
                .iter()
                .map(|e| json!({"id": e.id, "kind": "write", "path": e.shown, "liftable": true})),
        );
        out
    }
}
