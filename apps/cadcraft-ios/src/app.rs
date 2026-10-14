//! The app eframe runs on iPadOS: CADCraft's UI (`CadApp`, as on the desktop) plus what the host
//! owns: the safe area, taking saved documents to where the user keeps them, and saving the
//! preferences when the app leaves the foreground.
use std::path::PathBuf;

use cadcraft_ui_egui::CadApp;
use eframe::egui;

pub struct IosApp {
    cad: CadApp,
    /// The app's support folder (none: nowhere to keep documents and settings).
    root: Option<PathBuf>,
    was_focused: bool,
    last_safe_area: Option<String>,
    /// Takes the documents the app saves to where the user keeps them.
    sync: Option<crate::documents::DocumentSync>,
    /// Hardware-keyboard keys winit never delivers (Enter, arrows, Escape, Tab, function keys, shortcuts).
    keys: crate::hw_keys::HwKeys,
    workspace: crate::workspace::Workspace,
    workspace_error: Option<String>,
    /// Events to deliver on the next frame (key releases when the app loses focus).
    pending: Vec<egui::Event>,
}

impl IosApp {
    pub fn new(cad: CadApp, root: Option<PathBuf>, workspace: crate::workspace::Workspace, workspace_error: Option<String>) -> Self {
        let sync = root.clone().map(crate::documents::DocumentSync::new);
        Self {
            cad,
            root,
            was_focused: true,
            last_safe_area: None,
            sync,
            keys: crate::hw_keys::HwKeys::default(),
            pending: Vec::new(),
            workspace,
            workspace_error,
        }
    }

    /// Keep the panel layout current: after each change, and a last time on the way out.
    fn persist_workspace(&mut self, final_attempt: bool) {
        let result = if final_attempt { self.workspace.save_on_exit(&self.cad.ui) } else { self.workspace.save_if_changed(&self.cad.ui) };
        match result {
            Err(error) if self.workspace_error.as_ref() != Some(&error) => {
                log::warn!("CADCraft workspace: {error}");
                self.cad.set_status(format!("Workspace could not be saved: {error}"));
                self.workspace_error = Some(error);
            }
            Err(_) => {}
            Ok(()) => self.workspace_error = None,
        }
    }

    fn save_state(&self) {
        if let Some(root) = &self.root {
            crate::services::save_state(
                root,
                &crate::services::State {
                    profile: Some(self.cad.profile_json()),
                    prefs: Some(self.cad.prefs_json()),
                    autosave: Some(cadcraft_ui_egui::autosave::prefs_json()),
                },
            );
        }
    }
}

impl eframe::App for IosApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        crate::safe_area::report(ctx, &mut self.last_safe_area);
        // iPadOS may end a backgrounded app without a word: keep the settings current.
        let focused = ctx.input(|i| i.focused);
        if self.was_focused && !focused {
            self.save_state();
        }
        if !focused {
            // Key-ups may never arrive once the app is in the background.
            self.pending.extend(self.keys.release_all());
        }
        self.was_focused = focused;
        self.cad.logic(ctx);
        if let Some(sync) = &mut self.sync {
            sync.poll(|message| self.cad.set_status(message));
        }
        // iPadOS has no Quit: the app is left from the home screen.
        self.cad.quit_requested = false;
        self.persist_workspace(false);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.cad.ui(ui);
        crate::clipboard::handle_output(ui.ctx());
        // The wake is asked for inside the pass: one asked for before it starts is dropped.
        if let Some(wait) = self.keys.next_wake(ui.input(|i| i.time)) {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(wait.max(0.001)));
        }
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        raw_input.events.append(&mut self.pending);
        let now = raw_input.time.unwrap_or_default();
        let wants_text = ctx.egui_wants_keyboard_input();
        for (code, pressed) in crate::gc_keyboard::drain() {
            raw_input.events.extend(self.keys.handle(code, pressed, now, wants_text));
        }
        raw_input.events.extend(self.keys.tick(now));
        raw_input.events = crate::clipboard::shortcut_events(std::mem::take(&mut raw_input.events), crate::clipboard::text);
        if let Some(wait) = self.keys.next_wake(now) {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait.max(0.001)));
        }
        crate::safe_area::apply(raw_input);
        self.cad.raw_input_hook(raw_input);
        cadcraft_ui_egui::cmdline::capture_tab(ctx, raw_input);
    }

    fn on_exit(&mut self) {
        self.persist_workspace(true);
        self.save_state();
        self.cad.shutdown_autosave();
    }
}
