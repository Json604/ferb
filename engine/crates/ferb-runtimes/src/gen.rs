//! A pulled event stream with Python's async-generator timing: the body runs only while the consumer asks for the
//! next item, stops at each yield until asked again, and when the consumer goes away the body is told at its yield
//! so its clean-up still runs (Python's `aclose()` raising GeneratorExit there).

use std::future::Future;

use ferb_core::types::RuntimeEvent;
use tokio::sync::mpsc;

use crate::error::RuntimeError;

/// One item: an event, or what the body raised (the stream then ends).
pub type Item = Result<RuntimeEvent, RuntimeError>;

/// The consumer went away: the body should clean up and return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closed;

/// The body's side: hands one item over and waits until the next one is asked for.
pub struct Yielder {
    items: mpsc::Sender<Item>,
    pulls: mpsc::Receiver<()>,
}

impl Yielder {
    pub async fn emit(&mut self, event: RuntimeEvent) -> Result<(), Closed> {
        if self.items.send(Ok(event)).await.is_err() {
            return Err(Closed);
        }
        match self.pulls.recv().await {
            Some(()) => Ok(()),
            None => Err(Closed),
        }
    }

    async fn first_pull(&mut self) -> bool {
        self.pulls.recv().await.is_some()
    }
}

/// The consumer's side.
pub struct EventStream {
    pulls: Option<mpsc::Sender<()>>,
    items: mpsc::Receiver<Item>,
    asked: bool,
    done: bool,
}

impl EventStream {
    /// Spawns `body` (it starts on the first `next`). The body returns `Err` for what it raised.
    pub fn spawn<F, Fut>(body: F) -> EventStream
    where
        F: FnOnce(Yielder) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), RuntimeError>> + Send + 'static,
    {
        let (pull_tx, pull_rx) = mpsc::channel(1);
        let (item_tx, item_rx) = mpsc::channel(1);
        let mut yielder = Yielder {
            items: item_tx.clone(),
            pulls: pull_rx,
        };
        tokio::spawn(async move {
            if !yielder.first_pull().await {
                // Never iterated: Python's generator body never runs either.
                return;
            }
            let out = body(yielder).await;
            if let Err(e) = out {
                let _ = item_tx.send(Err(e)).await;
            }
        });
        EventStream {
            pulls: Some(pull_tx),
            items: item_rx,
            asked: false,
            done: false,
        }
    }

    /// An empty stream (Python: a generator that returns at once).
    pub fn empty() -> EventStream {
        let (_pull_tx, _) = mpsc::channel::<()>(1);
        let (_, item_rx) = mpsc::channel(1);
        EventStream {
            pulls: None,
            items: item_rx,
            asked: false,
            done: true,
        }
    }

    /// The next item; `None` when the body returned.
    pub async fn next(&mut self) -> Option<Item> {
        if self.done {
            return None;
        }
        if !self.asked {
            self.asked = true;
        }
        if let Some(p) = &self.pulls {
            if p.send(()).await.is_err() {
                self.done = true;
                return None;
            }
        }
        match self.items.recv().await {
            Some(item) => {
                if item.is_err() {
                    self.done = true;
                }
                Some(item)
            }
            None => {
                self.done = true;
                None
            }
        }
    }

    /// Everything left, until the body returns.
    pub async fn collect(mut self) -> Vec<Item> {
        let mut out = Vec::new();
        while let Some(item) = self.next().await {
            out.push(item);
        }
        out
    }

    /// Python's `aclose()`: the body is told at its yield and its clean-up runs; waits until it ended.
    pub async fn close(mut self) {
        self.pulls = None;
        self.items.close();
        while self.items.recv().await.is_some() {}
    }
}
