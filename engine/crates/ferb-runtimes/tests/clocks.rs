//! Every adapter's call timing reads a clock that moves unless a test gives it another one. The Claude and API
//! adapters were built with a clock that stood still, and nothing in the daemon replaced it, so every recorded
//! Claude call said 0 ms (the owner's store, 2026-11-17..26).

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use ferb_runtimes::api::{openai, ApiRuntime};
use ferb_runtimes::claude::{ClaudeRuntime, ToolProxy};
use ferb_runtimes::codex::CodexRuntime;
use ferb_runtimes::forms::{ContextForms, Refusals};

struct NoProxy;

impl ToolProxy for NoProxy {
    fn base_url(&self, run_id: &str) -> String {
        format!("http://127.0.0.1:9/run/{run_id}/s")
    }
    fn seen(&self, _run_id: &str) -> BTreeSet<String> {
        BTreeSet::new()
    }
    fn forget(&self, _run_id: &str) {}
}

fn moves(clock: &(dyn Fn() -> f64 + Send + Sync)) -> bool {
    let before = clock();
    std::thread::sleep(Duration::from_millis(15));
    clock() - before >= 0.010
}

// decision: RUN-419, RUN-420
#[test]
fn every_adapter_starts_with_a_clock_that_moves() {
    let home = tempfile::tempdir().unwrap();
    let refusals = || Arc::new(Refusals::new(Box::new(|| 0.0)));

    let claude = ClaudeRuntime::new(
        ferb_runtimes::claude::Mode::Supervisor,
        Arc::new(NoProxy),
        home.path().to_path_buf(),
        &[],
        None,
        Some("claude-sonnet-5-5".into()),
        Arc::new(ContextForms),
        refusals(),
        ferb_runtimes::claude::Host::system(),
    );
    assert!(
        moves(&*claude.monotonic),
        "the Claude adapter's clock stands still"
    );

    let api = ApiRuntime::new(
        Arc::new(openai::OpenAIProvider::new(None)),
        true,
        "orchestrator",
        &[],
        None,
        None,
        Arc::new(|_name: String| Box::pin(async { None })),
        Arc::new(ContextForms),
        refusals(),
    )
    .expect("a runtime");
    assert!(
        moves(&*api.monotonic),
        "the API adapter's clock stands still"
    );

    let codex = CodexRuntime::new(
        home.path().join("codex"),
        ferb_runtimes::codex::Mode::Supervisor,
        home.path().to_path_buf(),
        "gpt-5.6-terra",
        &[],
        ferb_runtimes::codex::Host::system(),
    );
    assert!(
        moves(&*codex.monotonic),
        "the Codex adapter's clock stands still"
    );
}
