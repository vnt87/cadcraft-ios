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
}

impl IosApp {
    pub fn new(cad: CadApp, root: Option<PathBuf>) -> Self {
        let sync = root.clone().map(crate::documents::DocumentSync::new);
        Self { cad, root, was_focused: true, last_safe_area: None, sync }
    }

    fn save_state(&self) {
        if let Some(root) = &self.root {
            crate::services::save_state(root, &crate::services::State {
                profile: Some(self.cad.profile_json()),
                prefs: Some(self.cad.prefs_json()),
                autosave: Some(cadcraft_ui_egui::autosave::prefs_json()),
            });
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
        self.was_focused = focused;
        self.cad.logic(ctx);
        if let Some(sync) = &mut self.sync {
            sync.poll(|message| self.cad.set_status(message));
        }
        // iPadOS has no Quit: the app is left from the home screen.
        self.cad.quit_requested = false;
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.cad.ui(ui);
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        crate::safe_area::apply(raw_input);
        self.cad.raw_input_hook(raw_input);
        cadcraft_ui_egui::cmdline::capture_tab(ctx, raw_input);
    }

    fn on_exit(&mut self) {
        self.save_state();
        self.cad.shutdown_autosave();
    }
}
