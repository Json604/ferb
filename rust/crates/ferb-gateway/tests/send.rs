//! The call pipeline's future is `Send`: the daemon runs gateway calls on any runtime thread (no dedicated thread).
#![cfg(all(feature = "store", target_os = "macos"))]

use std::sync::Arc;

use ferb_gateway::gateway::{Cancel, Gateway};
use ferb_rpc::auth::RunGrant;
use serde_json::Map;

fn is_send<T: Send>(_: &T) {}

#[allow(dead_code)]
fn call_future_is_send(gw: Arc<Gateway>, grant: RunGrant) {
    let args = Map::new();
    let f = gw.call_as(&grant, "fs.list", &args, Cancel::new(), Arc::new(|| false));
    is_send(&f);
    let f = gw.call(&grant, "fs.list", &args);
    is_send(&f);
}

#[test]
fn gateway_is_send_and_sync() {
    fn both<T: Send + Sync>() {}
    both::<Gateway>();
}
