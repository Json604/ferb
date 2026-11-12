//! FERB's local RPC: the wire, admission and roles. Port of `ferb/daemon/server.py`, `auth.py` and
//! `app_peer.py` (ENG-1: the unchanged FERB.app, CLI/MCP shim and native host keep working against it).
//!
//! - `wire`: the frames, byte for byte as Python writes them, and requests refused in pydantic's words.
//! - `auth`: roles, the method table (who may call what, and what is FERB.app's alone), run tokens.
//! - `server`: the Unix-socket server: hello first, the owner's uid, the role's methods, the starting gate,
//!   unary and stream handlers, and what happens when a caller leaves.
//! - `app_peer`, `security`, `peer`: the FERB.app-only check (audit token -> code signature -> requirement).
//!
//! The handlers themselves are other ports; they register through `Server::register_unary` and
//! `Server::register_stream`. Everything here is checked against `rust/fixtures/rpc`, written by the Python
//! build (ENG-3).

pub mod app_peer;
pub mod auth;
pub mod framing;
pub mod peer;
pub mod security;
pub mod server;
pub mod wire;

pub use app_peer::{AppPeers, Mode, Security};
pub use auth::{
    allowed, app_only, AuthError, Clock, GrantSpec, Identity, Method, Role, RunGrant, RunTokens,
    SystemClock, METHODS,
};
pub use peer::AuditToken;
pub use server::{
    caller_left, caller_left_flag, ping, BoxFuture, CallerLeft, EventStream, Failure, HandlerError,
    HandlerResult, NativeChannel, NativeConnection, Server, Session, StartError, Stream, Unary,
};
pub use wire::{Id, Params, Request};
