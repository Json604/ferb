//! The phone app's door into `client`: JSON text in and out, as plain C functions and as JNI functions for Android.
//!
//! **Clients.** `LinkClient` is one phone link with its own runtime, events and transfers; nothing about it is
//! global, so a process may hold several (the iPhone app of stage 4, the UniFFI layer of slice S8, tests) and close
//! each. The C and JNI functions name a client by a handle:
//!
//!   client_start(config)          -> {"ok":true,"handle":N,"id":"<endpoint id>"} | {"error":{..}}
//!   client_command(handle, json)  -> {"ok":true} at once; the outcome arrives as an event carrying the command's "id"
//!   client_poll(handle, wait_ms)  -> the next event as JSON text, or "" when none arrived in time
//!   client_close(handle)          -> {"ok":true}; the handle is gone (a later call says `link_closed`, never crashes)
//!
//! **The lab's three calls** (`start`, `command`, `poll`; Kotlin `local.ferb.link.Native.start/command/poll`) still
//! work, on one client the process keeps for them, so the lab's Android app drives this build unchanged.
//!
//! Every call is bounded (start `START_DEADLINE`, poll at most `POLL_WAIT_MAX`, close `CLOSE_DEADLINE`; a
//! command only hands work to the client's runtime) and catches a panic at the boundary: the caller gets
//! `{"error":{"code":"link_internal_error",..}}` (a command's work that panics later: an `error` event with its id),
//! never an abort of the app's process. (kb PHN-446: the defects of the lab's library: no close, no catch_unwind,
//! one global client.)
//!
//! config: {"state": <saved state or null>, "name", "app", "unlock_pub", "device_pub", "strong_pub",
//!          "segmentation_offload": bool (default true; upstream iroh #4647),
//!          "dev": {"shared_relays": [..], "force_relay": bool}}
//! commands: pair {qr} · call {id, method, params, proof?} · get {id, params, save_to?, append?}
//!           put {id, name, path, resume?, group?, rel?} (an `accepted` event carries the upload's id)
//!           cancel {target} · active {on} · network_changed {kind?} · retry · forget · phase
//!           proof {id, sig?} (the answer to a `prove` event: the device key's signature over its `payload`)
//!           payload {approval_id, challenge, method, params} · sealed_payload {..., text} · call_payload {challenge, method, params}

use crate::{
    client::{Phone, PhoneConfig},
    errors::{self, LinkError},
    net::{RelaySpec, Shared},
};
use base64::Engine;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// How long `start` may take (the runtime and the endpoint's sockets; no network wait is in it). A new timer of
/// this port, listed for the owner (kb PHN-303).
pub const START_DEADLINE: Duration = Duration::from_secs(20);
/// The longest one `poll` waits, whatever the app asks (the lab had no bound).
pub const POLL_WAIT_MAX: Duration = Duration::from_secs(60);
/// How long `close` waits for the connection to say goodbye and the endpoint to close.
pub const CLOSE_DEADLINE: Duration = Duration::from_secs(5);

/// The code of a panic caught at the app's door (CONTRACT §10: new codes may be added within a version).
pub const LINK_INTERNAL_ERROR: &str = "link_internal_error";

fn err(e: LinkError) -> String {
    json!({"error": e}).to_string()
}
fn internal(what: &str) -> LinkError {
    LinkError::new(
        LINK_INTERNAL_ERROR,
        format!("The phone link failed inside ({what})."),
    )
    .allow("Try again; if it keeps failing, close and open the app")
}
fn closed() -> LinkError {
    LinkError::new(errors::LINK_CLOSED, "This link was closed.").allow("Start it again")
}
/// Runs one boundary call; a panic inside becomes an error reply.
fn guarded(what: &str, f: impl FnOnce() -> String) -> String {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| err(internal(what)))
}

/// One phone link, owned by whoever started it, closed by `close` (or when dropped).
pub struct LinkClient {
    rt: Option<tokio::runtime::Runtime>,
    phone: Phone,
    events: tokio::sync::Mutex<UnboundedReceiver<Value>>,
    local: UnboundedSender<Value>,
    local_rx: tokio::sync::Mutex<UnboundedReceiver<Value>>,
    /// Work `cancel` can stop, by the command's id: transfers, and calls (a stream such as `chat.entries.watch`
    /// included); a call's entry goes when it ends.
    running: Arc<Mutex<HashMap<String, tokio::task::AbortHandle>>>,
    closed: AtomicBool,
    closing: tokio::sync::Notify,
}

impl LinkClient {
    /// Starts a link from the app's config. Bounded by `START_DEADLINE`.
    pub fn start(config: &str) -> Result<Self, LinkError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("ferb-link")
            .enable_all()
            .build()
            .map_err(|e| {
                LinkError::new(
                    LINK_INTERNAL_ERROR,
                    format!("The phone link could not start its threads ({e})."),
                )
            })?;
        let cfg: Value = serde_json::from_str(config).unwrap_or(Value::Null);
        let dev = &cfg["dev"];
        let shared = match dev["shared_relays"].as_array() {
            Some(a) if !a.is_empty() => Shared::Custom(
                a.iter()
                    .filter_map(|u| u.as_str())
                    .map(|u| RelaySpec::new(u, None))
                    .collect(),
            ),
            _ => Shared::N0,
        };
        let pc = PhoneConfig {
            state: serde_json::from_value(cfg["state"].clone()).ok(),
            name: cfg["name"].as_str().unwrap_or("Phone").to_string(),
            app: cfg["app"].as_str().unwrap_or("app").to_string(),
            unlock_pub: cfg["unlock_pub"].as_str().map(str::to_string),
            device_pub: cfg["device_pub"].as_str().map(str::to_string),
            strong_pub: cfg["strong_pub"].as_str().map(str::to_string),
            device_sign: None,
            shared,
            force_relay: dev["force_relay"].as_bool() == Some(true),
            segmentation_offload: cfg["segmentation_offload"].as_bool() != Some(false),
            portmapper: dev["portmapper"].as_bool() != Some(false),
            loopback_only: dev["loopback"].as_bool() == Some(true),
            ..Default::default()
        };
        let started =
            rt.block_on(async { tokio::time::timeout(START_DEADLINE, Phone::start(pc)).await });
        let (phone, events) = match started {
            Ok(Ok(x)) => x,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(LinkError::new(
                    errors::RELAY_UNREACHABLE,
                    format!(
                        "The phone link did not start within {} s.",
                        START_DEADLINE.as_secs()
                    ),
                )
                .allow("Try again"));
            }
        };
        let (local, local_rx) = tokio::sync::mpsc::unbounded_channel();
        Ok(Self {
            rt: Some(rt),
            phone,
            events: tokio::sync::Mutex::new(events),
            local,
            local_rx: tokio::sync::Mutex::new(local_rx),
            running: Default::default(),
            closed: AtomicBool::new(false),
            closing: tokio::sync::Notify::new(),
        })
    }

    fn rt(&self) -> &tokio::runtime::Runtime {
        self.rt
            .as_ref()
            .expect("the runtime lives as long as the client")
    }
    pub fn id(&self) -> String {
        self.phone.id()
    }
    /// The link client itself (tests; the UniFFI layer of slice S8 may take it over).
    pub fn phone(&self) -> &Phone {
        &self.phone
    }
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Runs a command's work on the runtime; when it panics, the app still hears an `error` event for its id.
    fn spawn_reporting(
        &self,
        id: Value,
        work: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> tokio::task::AbortHandle {
        let task = self.rt().spawn(work);
        let abort = task.abort_handle();
        let out = self.local.clone();
        self.rt().spawn(async move {
            if let Err(e) = task.await
                && e.is_panic()
            {
                let _ = out.send(
                    json!({"type": "error", "id": id, "error": internal("a command's work")}),
                );
            }
        });
        abort
    }

    /// One command (JSON text). Never waits on the network: work goes to the client's runtime.
    pub fn command(&self, text: &str) -> String {
        if self.is_closed() {
            return err(closed());
        }
        let phone = self.phone.clone();
        let Ok(c) = serde_json::from_str::<Value>(text) else {
            return err(LinkError::new(
                errors::BAD_REQUEST,
                "the command is not JSON",
            ));
        };
        let id = c["id"].clone();
        let out = self.local.clone();
        let done = move |r: Result<Value, LinkError>| {
            let _ = out.send(match r {
                Ok(v) => json!({"type": "result", "id": id, "result": v}),
                Err(e) => json!({"type": "error", "id": id, "error": e}),
            });
        };
        match c["cmd"].as_str().unwrap_or_default() {
            "pair" => {
                return match phone.pair(c["qr"].as_str().unwrap_or_default()) {
                    Ok(()) => json!({"ok": true}).to_string(),
                    Err(e) => err(e),
                };
            }
            "active" => phone.set_active(c["on"].as_bool() != Some(false)),
            "retry" => phone.retry_now(),
            "forget" => phone.forget(),
            "network_changed" => {
                let kind = c["kind"].as_str().map(str::to_string);
                self.spawn_reporting(c["id"].clone(), async move {
                    phone.network_changed_to(kind.as_deref()).await
                });
            }
            "proof" => phone.proof(
                c["id"].as_u64().unwrap_or(0),
                c["sig"].as_str().map(str::to_string),
            ),
            "sealed_payload" => {
                // for a hidden answer: only the value's SHA-256 enters the signed text
                return json!({"ok": true, "payload": Phone::sealed_payload(c["approval_id"].as_str().unwrap_or_default(), c["challenge"].as_str().unwrap_or_default(), c["text"].as_str().unwrap_or_default())}).to_string();
            }
            "call_payload" => {
                return json!({"ok": true, "payload": Phone::call_payload(c["challenge"].as_str().unwrap_or_default(), c["method"].as_str().unwrap_or_default(), &c["params"])}).to_string();
            }
            "payload" => {
                return json!({"ok": true, "payload": Phone::answer_payload(c["approval_id"].as_str().unwrap_or_default(), c["challenge"].as_str().unwrap_or_default(), c["method"].as_str().unwrap_or_default(), &c["params"])}).to_string();
            }
            "phase" => return json!({"ok": true, "phase": phone.phase()}).to_string(),
            "call" => {
                let (ev_out, ev_id) = (self.local.clone(), c["id"].clone());
                let cid = c["id"].clone();
                let key = c["id"].as_str().unwrap_or_default().to_string();
                let (running, ended) = (self.running.clone(), key.clone());
                // held across the spawn, so a call that ends at once removes its entry after it was put in
                let mut held = self.running.lock().unwrap();
                let h = self.spawn_reporting(cid, async move {
                    let proof = Some(c["proof"].clone()).filter(|p| !p.is_null());
                    let r = phone
                        .call(
                            c["method"].as_str().unwrap_or_default(),
                            c["params"].clone(),
                            proof,
                            |ev| {
                                let _ =
                                    ev_out.send(json!({"type": "event", "id": ev_id, "event": ev}));
                            },
                        )
                        .await;
                    done(r);
                    running.lock().unwrap().remove(&ended);
                });
                if !key.is_empty() {
                    held.insert(key, h);
                }
            }
            "cancel" => {
                // stop a running get or put (pause): what a file fetch already wrote stays, to go on later with
                // `offset`; or a call: its streams are dropped, so the Mac ends the call at its next event (bridge
                // `forward_stream`), and no result or error event follows for its id
                if let Some(h) = self
                    .running
                    .lock()
                    .unwrap()
                    .remove(c["target"].as_str().unwrap_or_default())
                {
                    h.abort();
                }
            }
            "get" => {
                let (pr_out, pr_id) = (self.local.clone(), c["id"].clone());
                let key = c["id"].as_str().unwrap_or_default().to_string();
                let h = self.spawn_reporting(c["id"].clone(), async move {
                    // `save_to`: chunks go straight into that file (appended when `append`), never all in memory
                    let mut file = match c["save_to"].as_str() {
                        Some(path) => match std::fs::OpenOptions::new().create(true).write(true).append(c["append"] == json!(true)).truncate(c["append"] != json!(true)).open(path) {
                            Ok(f) => Some(f),
                            Err(e) => return done(Err(LinkError::new(errors::FERB_ERROR, format!("The file could not be saved on the phone ({e})."))))
                        },
                        None => None,
                    };
                    let mut memory = Vec::new();
                    let mut last = std::time::Instant::now();
                    let r = phone
                        .get_with(
                            c["params"].clone(),
                            |part| {
                                use std::io::Write;
                                match file.as_mut() {
                                    Some(f) => {
                                        let _ = f.write_all(part);
                                    }
                                    None => memory.extend_from_slice(part),
                                }
                            },
                            |got, of| {
                                if last.elapsed().as_millis() > 120 || got == of {
                                    last = std::time::Instant::now();
                                    let _ = pr_out.send(json!({"type": "progress", "id": pr_id, "done": got, "of": of}));
                                }
                            },
                        )
                        .await;
                    done(r.map(|mut meta| {
                        match c["save_to"].as_str() {
                            Some(path) => meta["saved_to"] = json!(path),
                            None => meta["data"] = json!(base64::engine::general_purpose::STANDARD.encode(&memory)),
                        }
                        meta
                    }));
                });
                self.running.lock().unwrap().insert(key, h);
            }
            "put" => {
                let (pr_out, pr_id) = (self.local.clone(), c["id"].clone());
                let key = c["id"].as_str().unwrap_or_default().to_string();
                let (ac_out, ac_id) = (self.local.clone(), c["id"].clone());
                let h = self.spawn_reporting(c["id"].clone(), async move {
                    // read piece by piece from the phone's storage; `resume`, `group` and `rel` go to the Mac as given
                    let mut opts = json!({"name": c["name"].as_str().unwrap_or("file")});
                    for k in ["resume", "group", "rel"] {
                        if !c[k].is_null() {
                            opts[k] = c[k].clone();
                        }
                    }
                    let mut last = std::time::Instant::now();
                    let r = phone
                        .put_file(
                            std::path::Path::new(c["path"].as_str().unwrap_or_default()),
                            opts,
                            |accept| {
                                let _ = ac_out.send(json!({"type": "accepted", "id": ac_id, "upload_id": accept["upload_id"], "offset": accept["offset"]}));
                            },
                            |got, of| {
                                if last.elapsed().as_millis() > 120 || got == of {
                                    last = std::time::Instant::now();
                                    let _ = pr_out.send(json!({"type": "progress", "id": pr_id, "done": got, "of": of}));
                                }
                            },
                        )
                        .await;
                    done(r);
                });
                self.running.lock().unwrap().insert(key, h);
            }
            other => {
                return err(LinkError::new(
                    errors::BAD_REQUEST,
                    format!("unknown command \"{other}\""),
                ));
            }
        }
        json!({"ok": true}).to_string()
    }

    /// The next event, waiting at most `wait_ms` (and never more than `POLL_WAIT_MAX`); "" when none came. A client
    /// that is closed meanwhile answers at once.
    pub fn poll(&self, wait_ms: u64) -> String {
        if self.is_closed() {
            return err(closed());
        }
        let wait = Duration::from_millis(wait_ms).min(POLL_WAIT_MAX);
        self.rt().block_on(async {
            let mut ev = self.events.lock().await;
            let mut local = self.local_rx.lock().await;
            let next = async {
                tokio::select! {
                    v = ev.recv() => v,
                    v = local.recv() => v,
                    _ = self.closing.notified() => None,
                }
            };
            match tokio::time::timeout(wait, next).await {
                Ok(Some(v)) => v.to_string(),
                _ => String::new(),
            }
        })
    }

    /// Closes the link: transfers stop, the connection says goodbye, the endpoint closes. Bounded by
    /// `CLOSE_DEADLINE`; a second close does nothing.
    pub fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.closing.notify_waiters();
        for (_, h) in self.running.lock().unwrap().drain() {
            h.abort();
        }
        let phone = self.phone.clone();
        let _ = self
            .rt()
            .block_on(async { tokio::time::timeout(CLOSE_DEADLINE, phone.close()).await });
    }
}

impl Drop for LinkClient {
    fn drop(&mut self) {
        self.close();
        if let Some(rt) = self.rt.take() {
            rt.shutdown_timeout(CLOSE_DEADLINE);
        }
    }
}

// ---- handles: how the C and JNI doors name a client --------------------------------------------------------------
//
// A table from handle numbers to clients, so a handle used after its close is answered with `link_closed` instead of
// reaching freed memory. It holds no client of its own: each one is started and closed by the app.

fn clients() -> &'static Mutex<HashMap<u64, Arc<LinkClient>>> {
    static CLIENTS: std::sync::OnceLock<Mutex<HashMap<u64, Arc<LinkClient>>>> =
        std::sync::OnceLock::new();
    CLIENTS.get_or_init(Default::default)
}
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);

fn client(handle: u64) -> Option<Arc<LinkClient>> {
    clients()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&handle)
        .cloned()
}

/// Starts a client and gives back its handle: `{"ok":true,"handle":N,"id":..}` or `{"error":..}`.
pub fn client_start(config: &str) -> String {
    guarded("start", || match LinkClient::start(config) {
        Ok(c) => {
            let handle = NEXT_HANDLE.fetch_add(1, Ordering::SeqCst);
            let id = c.id();
            clients()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(handle, Arc::new(c));
            json!({"ok": true, "handle": handle, "id": id}).to_string()
        }
        Err(e) => err(e),
    })
}
pub fn client_command(handle: u64, json: &str) -> String {
    guarded("command", || match client(handle) {
        Some(c) => c.command(json),
        None => err(closed()),
    })
}
pub fn client_poll(handle: u64, wait_ms: u64) -> String {
    guarded("poll", || match client(handle) {
        Some(c) => c.poll(wait_ms),
        None => err(closed()),
    })
}
pub fn client_close(handle: u64) -> String {
    guarded("close", || {
        let c = clients()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&handle);
        match c {
            Some(c) => {
                c.close();
                json!({"ok": true}).to_string()
            }
            None => err(closed()),
        }
    })
}

// ---- the lab's three calls, on the one client the process keeps for them ---------------------------------------------

fn legacy() -> &'static Mutex<Option<u64>> {
    static LEGACY: Mutex<Option<u64>> = Mutex::new(None);
    &LEGACY
}

/// The lab's `start`: starts the process's one lab client, or says it runs (`already`).
pub fn start(config: &str) -> String {
    guarded("start", || {
        let mut slot = legacy().lock().unwrap_or_else(|p| p.into_inner());
        if let Some(c) = slot.and_then(client) {
            return json!({"ok": true, "id": c.id(), "already": true}).to_string();
        }
        let out = client_start(config);
        let v: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
        match v["handle"].as_u64() {
            Some(h) => {
                *slot = Some(h);
                json!({"ok": true, "id": v["id"]}).to_string()
            }
            None => out,
        }
    })
}
pub fn command(text: &str) -> String {
    match *legacy().lock().unwrap_or_else(|p| p.into_inner()) {
        Some(h) => client_command(h, text),
        None => err(LinkError::new(errors::BAD_REQUEST, "start was not called")),
    }
}
pub fn poll(wait_ms: u64) -> String {
    let h = *legacy().lock().unwrap_or_else(|p| p.into_inner());
    match h {
        Some(h) => client_poll(h, wait_ms),
        None => {
            std::thread::sleep(Duration::from_millis(wait_ms).min(POLL_WAIT_MAX));
            String::new()
        }
    }
}

// ---- C ABI (the iPhone app links these) ------------------------------------------------------------------------------

fn c_out(s: String) -> *mut std::ffi::c_char {
    std::ffi::CString::new(s.replace('\0', " "))
        .unwrap_or_default()
        .into_raw()
}
/// # Safety
/// `p` is null or a NUL-terminated string that stays valid for the call.
unsafe fn c_in<'a>(p: *const std::ffi::c_char) -> &'a str {
    if p.is_null() {
        ""
    } else {
        // SAFETY: the caller hands a valid NUL-terminated string (this function's contract).
        unsafe { std::ffi::CStr::from_ptr(p) }
            .to_str()
            .unwrap_or("")
    }
}
/// Starts a client; the reply carries its `handle`. Free the reply with `ferb_link_free`.
///
/// # Safety
/// `config` is null or a valid NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ferb_link_client_start(
    config: *const std::ffi::c_char,
) -> *mut std::ffi::c_char {
    // SAFETY: forwarded from this function's own contract.
    let config = unsafe { c_in(config) };
    c_out(client_start(config))
}
/// # Safety
/// `json` is null or a valid NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ferb_link_client_command(
    handle: u64,
    json: *const std::ffi::c_char,
) -> *mut std::ffi::c_char {
    // SAFETY: forwarded from this function's own contract.
    let json = unsafe { c_in(json) };
    c_out(client_command(handle, json))
}
#[unsafe(no_mangle)]
pub extern "C" fn ferb_link_client_poll(handle: u64, wait_ms: u64) -> *mut std::ffi::c_char {
    c_out(client_poll(handle, wait_ms))
}
#[unsafe(no_mangle)]
pub extern "C" fn ferb_link_client_close(handle: u64) -> *mut std::ffi::c_char {
    c_out(client_close(handle))
}
/// The lab's `start` (one client per process).
///
/// # Safety
/// `config` is null or a valid NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ferb_link_start(config: *const std::ffi::c_char) -> *mut std::ffi::c_char {
    // SAFETY: forwarded from this function's own contract.
    let config = unsafe { c_in(config) };
    c_out(start(config))
}
/// # Safety
/// `json` is null or a valid NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ferb_link_command(json: *const std::ffi::c_char) -> *mut std::ffi::c_char {
    // SAFETY: forwarded from this function's own contract.
    let json = unsafe { c_in(json) };
    c_out(command(json))
}
#[unsafe(no_mangle)]
pub extern "C" fn ferb_link_poll(wait_ms: u64) -> *mut std::ffi::c_char {
    c_out(poll(wait_ms))
}
/// Frees a string this library returned.
///
/// # Safety
/// `s` is null or a pointer returned by one of this library's functions, freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ferb_link_free(s: *mut std::ffi::c_char) {
    if !s.is_null() {
        // SAFETY: `s` came from `CString::into_raw` in `c_out` (this function's contract) and is freed once.
        drop(unsafe { std::ffi::CString::from_raw(s) });
    }
}

// ---- JNI (Android): class local.ferb.link.Native ---------------------------------------------------------------
#[cfg(target_os = "android")]
mod jni_glue {
    use jni::{
        JNIEnv,
        objects::{JClass, JString},
        sys::{jlong, jstring},
    };
    use std::panic::{AssertUnwindSafe, catch_unwind};
    fn text(env: &mut JNIEnv, s: &JString) -> String {
        env.get_string(s).map(|x| x.into()).unwrap_or_default()
    }
    fn out(env: &mut JNIEnv, s: String) -> jstring {
        env.new_string(s)
            .map(|x| x.into_raw())
            .unwrap_or(std::ptr::null_mut())
    }
    /// A JNI call whose Rust side may panic: the panic stays on this side and the app gets an error reply.
    fn door(mut env: JNIEnv, f: impl FnOnce(&mut JNIEnv) -> String) -> jstring {
        let reply = catch_unwind(AssertUnwindSafe(|| f(&mut env)))
            .unwrap_or_else(|_| super::err(super::internal("jni")));
        out(&mut env, reply)
    }
    /// iroh's DNS resolver asks Android for the network's DNS servers through `ndk_context`, which needs the
    /// process's JavaVM and application context once, before the first endpoint.
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_local_ferb_link_Native_init(
        env: JNIEnv,
        _c: JClass,
        context: jni::objects::JObject,
    ) {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            static INIT: std::sync::Once = std::sync::Once::new();
            INIT.call_once(|| {
                let (Ok(vm), Ok(global)) = (env.get_java_vm(), env.new_global_ref(&context)) else {
                    return;
                };
                // SAFETY: the JavaVM pointer and a global reference to the application context stay valid for the
                // life of the process (the global reference is leaked on purpose below).
                unsafe {
                    ndk_context::initialize_android_context(
                        vm.get_java_vm_pointer() as *mut std::ffi::c_void,
                        global.as_obj().as_raw() as *mut std::ffi::c_void,
                    );
                }
                std::mem::forget(global); // ndk_context keeps the raw pointer for the life of the process
            });
        }));
    }
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_local_ferb_link_Native_start(
        env: JNIEnv,
        _c: JClass,
        config: JString,
    ) -> jstring {
        door(env, |env| super::start(&text(env, &config)))
    }
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_local_ferb_link_Native_command(
        env: JNIEnv,
        _c: JClass,
        json: JString,
    ) -> jstring {
        door(env, |env| super::command(&text(env, &json)))
    }
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_local_ferb_link_Native_poll(
        env: JNIEnv,
        _c: JClass,
        wait_ms: jlong,
    ) -> jstring {
        door(env, |_| super::poll(wait_ms.max(0) as u64))
    }
    // the clients by handle
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_local_ferb_link_Native_open(
        env: JNIEnv,
        _c: JClass,
        config: JString,
    ) -> jstring {
        door(env, |env| super::client_start(&text(env, &config)))
    }
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_local_ferb_link_Native_send(
        env: JNIEnv,
        _c: JClass,
        handle: jlong,
        json: JString,
    ) -> jstring {
        door(env, |env| {
            super::client_command(handle.max(0) as u64, &text(env, &json))
        })
    }
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_local_ferb_link_Native_next(
        env: JNIEnv,
        _c: JClass,
        handle: jlong,
        wait_ms: jlong,
    ) -> jstring {
        door(env, |_| {
            super::client_poll(handle.max(0) as u64, wait_ms.max(0) as u64)
        })
    }
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_local_ferb_link_Native_close(
        env: JNIEnv,
        _c: JClass,
        handle: jlong,
    ) -> jstring {
        door(env, |_| super::client_close(handle.max(0) as u64))
    }
}

/// A test-only way to make a boundary call panic, to prove it is caught (`tests/ffi.rs`).
#[doc(hidden)]
pub fn guarded_for_test(f: impl FnOnce() -> String) -> String {
    guarded("test", f)
}
