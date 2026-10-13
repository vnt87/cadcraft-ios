//! File pickers that may answer on a later frame.
//!
//! `Services::pick_open` and `pick_save` block until the user chooses, which is fine where the
//! dialog runs on its own thread and not where the UI thread is the one that must keep running: on
//! iPadOS egui draws on UIKit's main thread, and a document picker needs that thread's run loop. A
//! platform with such a picker sets [`Services::begin_pick`] and [`Services::poll_pick`];
//! [`CadApp::pick`] starts the pick and runs what should follow it once the answer arrives.
//! Elsewhere `begin_pick` is unset (or declines) and `pick` falls back to the blocking closures, so
//! behaviour there is unchanged.

use serde_json::Value;

use crate::CadApp;

/// What a picker asks the user for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickKind {
    /// An existing file to read.
    Open,
    /// Where to write a file.
    Save,
}

/// One picker, as a platform's [`Services::begin_pick`](crate::Services) sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickRequest {
    pub kind: PickKind,
    pub title: String,
    /// The suggested file name (`Save` only).
    pub name: String,
    /// The file extensions on offer, without dots.
    pub exts: Vec<String>,
}

impl PickRequest {
    pub fn open(title: &str, exts: &[&str]) -> Self {
        Self { kind: PickKind::Open, title: title.into(), name: String::new(), exts: exts.iter().map(|e| (*e).into()).collect() }
    }

    pub fn save(title: &str, name: &str, exts: &[&str]) -> Self {
        Self { kind: PickKind::Save, title: title.into(), name: name.into(), exts: exts.iter().map(|e| (*e).into()).collect() }
    }
}

/// What to do with the chosen path; its result is reported like any command's.
pub(crate) type PickDone = Box<dyn FnOnce(&mut CadApp, String) -> Result<Value, String>>;

/// How a [`CadApp::pick`] went.
pub(crate) enum PickOutcome {
    /// The platform answered at once and the follow-up ran; this is its result.
    Done(Result<Value, String>),
    /// The platform answered at once: the user cancelled.
    Cancelled,
    /// The platform will answer on a later frame ([`CadApp::poll_pick`]).
    Pending,
    /// This host has no file dialog.
    Unavailable,
}

impl PickOutcome {
    /// The follow-up's result, or nothing to report when there was none (cancelled, still open or
    /// no dialog here).
    pub(crate) fn value(self) -> Result<Value, String> {
        match self {
            PickOutcome::Done(r) => r,
            _ => Ok(Value::Null),
        }
    }
}

impl CadApp {
    /// Whether this host can ask for a file of `kind` at all (a deferred or a blocking picker).
    pub(crate) fn can_pick(&self, kind: PickKind) -> bool {
        self.services.begin_pick.is_some()
            || match kind {
                PickKind::Open => self.services.pick_open.is_some(),
                PickKind::Save => self.services.pick_save.is_some(),
            }
    }

    /// Ask the user for a file and run `then` with the choice. Only one deferred pick is under way
    /// at a time; a second request while one is open is dropped.
    pub(crate) fn pick(&mut self, request: PickRequest, then: impl FnOnce(&mut CadApp, String) -> Result<Value, String> + 'static) -> PickOutcome {
        if self.pending_pick.is_some() {
            return PickOutcome::Pending;
        }
        if let Some(begin) = self.services.begin_pick.as_mut()
            && begin(&request)
        {
            self.pending_pick = Some(Box::new(then));
            return PickOutcome::Pending;
        }
        let chosen = match request.kind {
            PickKind::Open => self.services.pick_open.as_ref().map(|pick| pick()),
            PickKind::Save => self.services.pick_save.as_ref().map(|pick| pick(&request.name)),
        };
        match chosen {
            None => PickOutcome::Unavailable,
            Some(None) => PickOutcome::Cancelled,
            Some(Some(path)) => PickOutcome::Done(then(self, path)),
        }
    }

    /// Finish the pick under way when the platform has its answer (called every frame). A failure of
    /// what followed is echoed to the command line and the status bar, as a typed command's would be.
    pub fn poll_pick(&mut self) {
        if self.pending_pick.is_none() {
            return;
        }
        let Some(poll) = self.services.poll_pick.as_mut() else { return };
        let Some(answer) = poll() else { return };
        if let Some(then) = self.pending_pick.take()
            && let Some(path) = answer
            && let Err(e) = then(self, path)
        {
            self.session.echo(e.clone());
            self.set_status(e);
        }
    }

    /// A pick is waiting for the user's answer.
    pub fn picking(&self) -> bool {
        self.pending_pick.is_some()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use cadcraft_engine::Session;

    use super::{PickKind, PickRequest};
    use crate::{CadApp, Services};

    /// A platform whose picker answers later: records what was asked and answers when told to.
    #[derive(Clone, Default)]
    struct Deferred {
        asked: Rc<RefCell<Vec<PickRequest>>>,
        answer: Rc<RefCell<Option<Option<String>>>>,
        polls: Rc<Cell<usize>>,
    }

    impl Deferred {
        fn app(&self) -> CadApp {
            let (asked, answer, polls) = (self.asked.clone(), self.answer.clone(), self.polls.clone());
            let services = Services {
                begin_pick: Some(Box::new(move |request: &PickRequest| {
                    asked.borrow_mut().push(request.clone());
                    true
                })),
                poll_pick: Some(Box::new(move || {
                    polls.set(polls.get() + 1);
                    answer.borrow_mut().take()
                })),
                ..Services::default()
            };
            CadApp::new(Session::new(), services)
        }
    }

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cadcraft-picks-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn a_deferred_export_waits_for_the_answer_then_runs() {
        let dir = temp("export");
        let platform = Deferred::default();
        let mut app = platform.app();
        app.start("export");
        assert!(app.picking());
        let asked = platform.asked.borrow().clone();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].kind, PickKind::Save);
        assert_eq!(asked[0].name, "Drawing1.svg");
        // Still choosing: the export has not run and the pick stays open.
        app.poll_pick();
        assert!(app.picking());
        assert!(app.status.is_none());
        // The test build has no file writer, so the export that follows the answer reports that.
        let target = dir.join("part.svg");
        *platform.answer.borrow_mut() = Some(Some(target.to_string_lossy().to_string()));
        app.poll_pick();
        assert!(!app.picking());
        assert!(app.status.is_some(), "the export ran with the answer");
        std::fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn a_cancelled_deferred_pick_does_nothing_and_frees_the_slot() {
        let platform = Deferred::default();
        let mut app = platform.app();
        app.start("import");
        assert!(app.picking());
        assert_eq!(platform.asked.borrow()[0].kind, PickKind::Open);
        *platform.answer.borrow_mut() = Some(None);
        app.poll_pick();
        assert!(!app.picking());
        let before = platform.polls.get();
        app.poll_pick();
        assert_eq!(platform.polls.get(), before, "an idle app does not poll the platform");
        app.start("export");
        assert!(app.picking(), "a new pick can start");
    }

    #[test]
    fn a_second_request_while_one_is_open_is_dropped() {
        let platform = Deferred::default();
        let mut app = platform.app();
        app.start("export");
        app.start("import");
        assert_eq!(platform.asked.borrow().len(), 1);
        assert_eq!(platform.asked.borrow()[0].kind, PickKind::Save);
    }

    #[test]
    fn a_deferred_open_reads_the_chosen_drawing_in_the_background() {
        let platform = Deferred::default();
        let mut app = platform.app();
        crate::menus::activate(&mut app, "open");
        assert!(app.picking());
        assert_eq!(platform.asked.borrow()[0].kind, PickKind::Open);
        let missing = temp("open").join("missing.dxf");
        *platform.answer.borrow_mut() = Some(Some(missing.to_string_lossy().to_string()));
        app.poll_pick();
        assert!(!app.picking());
        assert!(app.opening.busy() || app.status.is_some(), "the read starts in the background");
        let t = Instant::now();
        while app.opening.busy() && t.elapsed() < Duration::from_secs(10) {
            app.finish_opening();
        }
        assert!(app.status.is_some() || !app.opening.busy());
    }

    #[test]
    fn what_goes_wrong_after_a_deferred_pick_is_reported_not_lost() {
        let platform = Deferred::default();
        let mut app = platform.app();
        app.start("export");
        let nowhere = temp("nowhere").join("no-such-dir").join("part.svg");
        *platform.answer.borrow_mut() = Some(Some(nowhere.to_string_lossy().to_string()));
        app.poll_pick();
        assert!(!app.picking());
        assert!(app.status.is_some(), "the failed export is reported");
    }

    #[test]
    fn without_begin_pick_the_blocking_closures_answer_at_once() {
        let dir = temp("blocking");
        let answer = dir.join("blocking.svg").to_string_lossy().to_string();
        let asked = Rc::new(Cell::new(0));
        let counted = asked.clone();
        let services = Services {
            pick_save: Some(Box::new(move |_: &str| {
                counted.set(counted.get() + 1);
                Some(answer.clone())
            })),
            ..Services::default()
        };
        let mut app = CadApp::new(Session::new(), services);
        app.start("export");
        assert!(!app.picking(), "a blocking pick never leaves one pending");
        assert_eq!(asked.get(), 1, "desktop behaviour is unchanged: the closure answered at once");
        std::fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn a_platform_that_declines_begin_pick_falls_back_to_the_blocking_closures() {
        let dir = temp("declined");
        let answer = dir.join("declined.svg").to_string_lossy().to_string();
        let asked = Rc::new(Cell::new(0));
        let counted = asked.clone();
        let services = Services {
            begin_pick: Some(Box::new(|_| false)),
            pick_save: Some(Box::new(move |_: &str| {
                counted.set(counted.get() + 1);
                Some(answer.clone())
            })),
            ..Services::default()
        };
        let mut app = CadApp::new(Session::new(), services);
        app.start("export");
        assert!(!app.picking());
        assert_eq!(asked.get(), 1);
        std::fs::remove_dir_all(dir).expect("cleanup");
    }
}
