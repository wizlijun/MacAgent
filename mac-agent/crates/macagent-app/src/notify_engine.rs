//! NotifyEngine: tracks in-flight notify registrations and per-session regex watchers.
//!
//! - `register_notify` / `complete_notify`: track `macagent notify` invocations
//! - `add_watcher` / `remove_watcher` / `list_watchers`: per-session regex watchers
//! - `feed_session_line`: check each watcher regex; on match → push notification + WatcherMatched

use crate::push_client::PushClient;
use macagent_core::ctrl_msg::{CtrlPayload, WatcherInfo};
use regex::Regex;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

// ── In-flight notify entry ────────────────────────────────────────────────────

struct NotifyEntry {
    argv: Vec<String>,
    started_at_ms: u64,
    session_hint: Option<String>,
    title: Option<String>,
}

// ── Watcher entry ─────────────────────────────────────────────────────────────

struct Watcher {
    id: String,
    regex: Regex,
    regex_str: String,
    name: String,
    hits: u32,
    last_match: Option<String>,
}

// ── NotifyEngine ──────────────────────────────────────────────────────────────

pub struct NotifyEngine {
    inner: Mutex<EngineInner>,
    push_client: Option<Arc<PushClient>>,
    ctrl_tx: mpsc::UnboundedSender<CtrlPayload>,
}

struct EngineInner {
    /// register_id → in-flight entry
    in_flight: HashMap<String, NotifyEntry>,
    /// sid → list of watchers
    watchers: HashMap<String, Vec<Watcher>>,
}

impl NotifyEngine {
    pub fn new(
        push_client: Option<Arc<PushClient>>,
        ctrl_tx: mpsc::UnboundedSender<CtrlPayload>,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(EngineInner {
                in_flight: HashMap::new(),
                watchers: HashMap::new(),
            }),
            push_client,
            ctrl_tx,
        })
    }

    // ── notify lifecycle ─────────────────────────────────────────────────────

    pub async fn register_notify(
        &self,
        register_id: String,
        argv: Vec<String>,
        started_at_ms: u64,
        session_hint: Option<String>,
        title: Option<String>,
    ) {
        let mut inner = self.inner.lock().await;
        inner.in_flight.insert(
            register_id,
            NotifyEntry {
                argv,
                started_at_ms,
                session_hint,
                title,
            },
        );
    }

    pub async fn complete_notify(&self, register_id: String, exit_code: i32, ended_at_ms: u64) {
        let entry = {
            let mut inner = self.inner.lock().await;
            inner.in_flight.remove(&register_id)
        };
        let Some(entry) = entry else { return };

        let elapsed_s = (ended_at_ms.saturating_sub(entry.started_at_ms)) / 1000;
        let cmd = entry.argv.first().cloned().unwrap_or_else(|| "cmd".into());
        let title = entry.title.unwrap_or_else(|| format!("{} finished", cmd));
        let body = if exit_code == 0 {
            format!("Completed in {}s", elapsed_s)
        } else {
            format!("Exited {} in {}s", exit_code, elapsed_s)
        };

        if let Some(pc) = &self.push_client {
            let pc = Arc::clone(pc);
            let title_cl = title.clone();
            let body_cl = body.clone();
            let sid_hint = entry.session_hint.clone();
            tokio::spawn(async move {
                let thread_id = sid_hint.as_deref();
                if let Err(e) = pc.send(&title_cl, &body_cl, None, thread_id).await {
                    eprintln!("[notify_engine] push error: {e}");
                }
            });
        }
    }

    // ── watchers ─────────────────────────────────────────────────────────────

    /// Returns Err if regex is invalid.
    pub async fn add_watcher(
        &self,
        sid: String,
        watcher_id: String,
        regex_str: String,
        name: String,
    ) -> Result<(), String> {
        let re = Regex::new(&regex_str).map_err(|e| e.to_string())?;
        let mut inner = self.inner.lock().await;
        let list = inner.watchers.entry(sid).or_default();
        // Remove any existing watcher with same id (idempotent add).
        list.retain(|w| w.id != watcher_id);
        list.push(Watcher {
            id: watcher_id,
            regex: re,
            regex_str,
            name,
            hits: 0,
            last_match: None,
        });
        Ok(())
    }

    pub async fn remove_watcher(&self, sid: &str, watcher_id: &str) {
        let mut inner = self.inner.lock().await;
        if let Some(list) = inner.watchers.get_mut(sid) {
            list.retain(|w| w.id != watcher_id);
        }
    }

    pub async fn list_watchers(&self, sid: &str) -> Vec<WatcherInfo> {
        let inner = self.inner.lock().await;
        inner
            .watchers
            .get(sid)
            .map(|list| {
                list.iter()
                    .map(|w| WatcherInfo {
                        id: w.id.clone(),
                        regex: w.regex_str.clone(),
                        name: w.name.clone(),
                        hits: w.hits,
                        last_match: w.last_match.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    // ── feed ──────────────────────────────────────────────────────────────────

    /// Called for each new/changed terminal line. Checks each watcher regex;
    /// on match → push notification + WatcherMatched ctrl.
    pub async fn feed_session_line(&self, sid: &str, line_text: &str) {
        // Collect matching watcher info under the lock, then release before async ops.
        let matches: Vec<(String, String, String)> = {
            let mut inner = self.inner.lock().await;
            let Some(list) = inner.watchers.get_mut(sid) else {
                return;
            };
            let mut out = Vec::new();
            for w in list.iter_mut() {
                if w.regex.is_match(line_text) {
                    w.hits += 1;
                    w.last_match = Some(line_text.to_string());
                    out.push((w.id.clone(), w.name.clone(), line_text.to_string()));
                }
            }
            out
        };

        for (watcher_id, name, text) in matches {
            // Send WatcherMatched ctrl to iOS.
            let _ = self.ctrl_tx.send(CtrlPayload::WatcherMatched {
                sid: sid.to_string(),
                watcher_id: watcher_id.clone(),
                line_text: text.clone(),
            });

            // Push notification (best-effort).
            if let Some(pc) = &self.push_client {
                let pc = Arc::clone(pc);
                let title = format!("Watcher: {}", name);
                let body = text.clone();
                let sid_str = sid.to_string();
                tokio::spawn(async move {
                    if let Err(e) = pc.send(&title, &body, None, Some(sid_str.as_str())).await {
                        eprintln!("[notify_engine] watcher push error: {e}");
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn make_engine() -> (Arc<NotifyEngine>, mpsc::UnboundedReceiver<CtrlPayload>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let engine = NotifyEngine::new(None, tx);
        (engine, rx)
    }

    #[tokio::test]
    async fn add_and_list_watcher() {
        let (engine, _rx) = make_engine();
        engine
            .add_watcher("s1".into(), "w1".into(), "error".into(), "Errors".into())
            .await
            .unwrap();
        let list = engine.list_watchers("s1").await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "w1");
        assert_eq!(list[0].hits, 0);
    }

    #[tokio::test]
    async fn remove_watcher() {
        let (engine, _rx) = make_engine();
        engine
            .add_watcher("s1".into(), "w1".into(), "error".into(), "E".into())
            .await
            .unwrap();
        engine.remove_watcher("s1", "w1").await;
        assert!(engine.list_watchers("s1").await.is_empty());
    }

    #[tokio::test]
    async fn feed_match_increments_hits_and_emits_ctrl() {
        let (engine, mut rx) = make_engine();
        engine
            .add_watcher("s1".into(), "w1".into(), r"err".into(), "Err".into())
            .await
            .unwrap();
        engine.feed_session_line("s1", "fatal error occurred").await;

        let list = engine.list_watchers("s1").await;
        assert_eq!(list[0].hits, 1);
        assert_eq!(list[0].last_match.as_deref(), Some("fatal error occurred"));

        let msg = rx.try_recv().expect("ctrl message");
        assert!(matches!(msg, CtrlPayload::WatcherMatched { .. }));
    }

    #[tokio::test]
    async fn invalid_regex_returns_err() {
        let (engine, _rx) = make_engine();
        let result = engine
            .add_watcher("s1".into(), "w1".into(), "[invalid".into(), "X".into())
            .await;
        assert!(result.is_err());
    }
}
