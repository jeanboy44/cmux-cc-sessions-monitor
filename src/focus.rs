//! Runs cmux focus calls off the UI thread.
//!
//! [`BackgroundFocus`] is a [`Cmux`] whose `focus` only dispatches the call to
//! a short-lived worker thread and returns `Ok(())` at once; the real outcome
//! arrives later through [`BackgroundFocus::try_result`], which the event loop
//! drains every frame. `sessions` and `read_screen` are passed through
//! synchronously (the UI thread never calls them).

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use anyhow::Result;

use crate::source::cmux::{Cmux, CmuxSessionRef};

/// Outcome of a dispatched focus: `Err` holds a one-line message for the footer.
pub type FocusResult = std::result::Result<(), String>;

pub struct BackgroundFocus {
    inner: Arc<dyn Cmux + Send + Sync>,
    tx: Sender<FocusResult>,
    rx: Receiver<FocusResult>,
}

impl BackgroundFocus {
    pub fn new(inner: Arc<dyn Cmux + Send + Sync>) -> Self {
        let (tx, rx) = channel();
        Self { inner, tx, rx }
    }

    /// A finished focus call, if any (non-blocking).
    pub fn try_result(&self) -> Option<FocusResult> {
        self.rx.try_recv().ok()
    }
}

fn first_line(e: &anyhow::Error) -> String {
    e.to_string().lines().next().unwrap_or_default().to_string()
}

impl Cmux for BackgroundFocus {
    fn sessions(&self) -> Result<Vec<CmuxSessionRef>> {
        self.inner.sessions()
    }

    fn tree(&self) -> Result<crate::source::cmux::TreeLayout> {
        self.inner.tree()
    }

    /// Dispatches the focus to a worker thread; always returns `Ok(())` once
    /// dispatched. Exactly one [`FocusResult`] is delivered per call, also
    /// when the call panics.
    fn focus(&self, workspace_id: &str, surface_id: &str) -> Result<()> {
        let (inner, tx) = (self.inner.clone(), self.tx.clone());
        let (ws, sf) = (workspace_id.to_string(), surface_id.to_string());
        let spawned = std::thread::Builder::new().name("focus".into()).spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| inner.focus(&ws, &sf)))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("focus failed")))
                .map_err(|e| first_line(&e));
            let _ = tx.send(result);
        });
        if let Err(e) = spawned {
            let _ = self.tx.send(Err(e.to_string()));
        }
        Ok(())
    }

    fn read_screen(&self, workspace_id: &str, surface_id: &str) -> Result<String> {
        self.inner.read_screen(workspace_id, surface_id)
    }
}
