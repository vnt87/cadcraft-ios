//! The iPadOS platform services CADCraft's UI asks for: document pickers (UIKit's, answering on a
//! later frame) and the settings kept in the app container.
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;

use cadcraft_ui_egui::Services;
use cadcraft_ui_egui::picks::PickRequest;
use serde::{Deserialize, Serialize};

/// The settings' file in the support folder.
const STATE: &str = "state.json";

/// The app's support folder, where its settings and managed documents live.
pub struct Platform {
    pub root: PathBuf,
}

fn support_directory() -> Result<PathBuf, String> {
    use objc2_foundation::{NSFileManager, NSSearchPathDirectory, NSSearchPathDomainMask};
    let directories = NSFileManager::defaultManager()
        .URLsForDirectory_inDomains(NSSearchPathDirectory::ApplicationSupportDirectory, NSSearchPathDomainMask::UserDomainMask);
    let path = directories.firstObject().and_then(|url| url.path()).ok_or("iOS Application Support directory is unavailable")?;
    let expected = crate::storage::app_support_dir(std::env::var_os("HOME"))?;
    let root = PathBuf::from(path.to_string()).join(expected.file_name().ok_or("Application Support name is missing")?);
    // The OS can spell the same container through /var or /private/var.
    std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    if root.canonicalize().map_err(|error| error.to_string())? != expected.canonicalize().map_err(|error| error.to_string())? {
        return Err("Application Support is outside the expected app container".into());
    }
    Ok(root)
}

/// Find the support folder and prepare the managed documents folder in it.
pub fn create() -> Result<Platform, String> {
    let root = support_directory()?;
    crate::documents::prepare(&root)?;
    Ok(Platform { root })
}

/// What CADCraft keeps between runs: each is the JSON text the app itself produced.
#[derive(Default, Serialize, Deserialize)]
pub struct State {
    /// Options: drafting settings, selection sizes, drawing-area colours.
    pub profile: Option<String>,
    /// The interface preferences (theme, language, palettes).
    pub prefs: Option<String>,
    /// Autosave preferences.
    pub autosave: Option<String>,
}

/// The saved settings, or none when there are none yet (or they cannot be read).
pub fn load_state(root: &std::path::Path) -> State {
    std::fs::read_to_string(root.join(STATE)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

/// Keep the settings now (the app is leaving the foreground or quitting). Written atomically (a
/// temporary file renamed over the old one: a crash leaves either version whole).
pub fn save_state(root: &std::path::Path, state: &State) {
    let tmp = root.join("state.json.tmp");
    let written = serde_json::to_string(state)
        .map_err(std::io::Error::other)
        .and_then(|text| std::fs::write(&tmp, text))
        .and_then(|()| std::fs::rename(&tmp, root.join(STATE)));
    if let Err(error) = written {
        log::warn!("settings: couldn't save them in {}: {error}", root.display());
    }
}

/// The services CADCraft's UI is given: pickers that answer on a later frame. Without a support
/// folder there is nowhere to keep documents, so no picker is offered.
pub fn ios(root: Option<PathBuf>, ui: egui::Context) -> Services {
    let Some(root) = root else { return Services::default() };
    let answers: Rc<RefCell<Option<mpsc::Receiver<Option<PathBuf>>>>> = Rc::default();
    let started = answers.clone();
    Services {
        begin_pick: Some(Box::new(move |request: &PickRequest| {
            let (tx, rx) = mpsc::channel();
            if !crate::documents::begin(request, &root, crate::documents::Reply::new(tx, ui.clone())) {
                return false;
            }
            *started.borrow_mut() = Some(rx);
            true
        })),
        poll_pick: Some(Box::new(move || {
            let mut slot = answers.borrow_mut();
            let received = slot.as_ref()?.try_recv();
            match received {
                Ok(answer) => {
                    *slot = None;
                    Some(answer.and_then(|path| path.to_str().map(str::to_owned)))
                }
                Err(mpsc::TryRecvError::Empty) => None,
                // The picker went away without answering: that is a cancel.
                Err(mpsc::TryRecvError::Disconnected) => {
                    *slot = None;
                    Some(None)
                }
            }
        })),
        ..Services::default()
    }
}
