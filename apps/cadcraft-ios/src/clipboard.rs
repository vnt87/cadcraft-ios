//! The system clipboard and URLs, the two services egui-winit has no iPadOS backend for.
//!
//! * Copy: a `CopyText` command egui queues (a copied selection as DXF text, a log line) goes to
//!   `UIPasteboard`, so it can be pasted in another app.
//! * Paste: winit never delivers Cmd/Ctrl+C, X and V from a hardware keyboard, and
//!   [`crate::hw_keys`] sends them as key presses, which egui does not turn into clipboard events
//!   itself. [`shortcut_events`] does, with the pasteboard text for Paste.
//! * Links: an `OpenUrl` command is handed to UIKit.
#[cfg(target_os = "ios")]
use eframe::egui::{self, OutputCommand};
use eframe::egui::{Event, Key};

#[cfg(target_os = "ios")]
/// Longest text taken from or put on the pasteboard (a drawing copied as DXF can be large).
const MAX_TEXT: usize = 64 * 1024 * 1024;

/// `true` for a URL worth handing to UIKit: one scheme, no whitespace or control characters.
pub fn valid_url(value: &str) -> bool {
    let Some((scheme, rest)) = value.split_once(':') else { return false };
    !rest.is_empty()
        && !scheme.is_empty()
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && !value.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// Turn the Cmd/Ctrl+C, X and V key presses into `Copy`, `Cut` and `Paste` events. `paste` supplies
/// the clipboard text (asked only for a V press); a press with nothing to paste does nothing.
pub fn shortcut_events(events: Vec<Event>, mut paste: impl FnMut() -> Option<String>) -> Vec<Event> {
    events
        .into_iter()
        .filter_map(|event| match &event {
            Event::Key { key, pressed: true, modifiers, repeat: false, .. } if modifiers.command || modifiers.ctrl => match key {
                Key::C => Some(Event::Copy),
                Key::X => Some(Event::Cut),
                Key::V => paste().map(Event::Paste),
                _ => Some(event),
            },
            // The release of a key whose press became a clipboard event has nothing left to do.
            _ => Some(event),
        })
        .collect()
}

/// Send the copy and link requests this frame's UI queued to UIKit.
#[cfg(target_os = "ios")]
pub fn handle_output(ctx: &egui::Context) {
    let (copied, urls): (Vec<String>, Vec<String>) = ctx.output(|o| {
        let mut copied = Vec::new();
        let mut urls = Vec::new();
        for command in &o.commands {
            match command {
                OutputCommand::CopyText(text) => copied.push(text.clone()),
                OutputCommand::OpenUrl(open) => urls.push(open.url.clone()),
                _ => {}
            }
        }
        (copied, urls)
    });
    if let Some(text) = copied.last()
        && let Err(error) = set_text(text)
    {
        log::warn!("CADCraft clipboard copy failed: {error}");
    }
    for url in urls {
        if let Err(error) = open_url(&url) {
            log::warn!("CADCraft could not open {url}: {error}");
        }
    }
}

#[cfg(target_os = "ios")]
pub use native::{open_url, set_text, text};

#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
mod native {
    use objc2::MainThreadMarker;
    use objc2_foundation::{NSDictionary, NSString, NSURL};
    use objc2_ui_kit::{UIApplication, UIPasteboard};

    fn main_thread() -> Result<MainThreadMarker, String> {
        MainThreadMarker::new().ok_or_else(|| "UIKit clipboard and URL services require the main thread".into())
    }

    /// The pasteboard's text, if it holds some (and not an absurd amount).
    pub fn text() -> Option<String> {
        main_thread().ok()?;
        // SAFETY: `string` is a plain NSString getter on the general pasteboard, called on the main thread.
        let text = unsafe { UIPasteboard::generalPasteboard().string() }?.to_string();
        (text.len() <= super::MAX_TEXT).then_some(text)
    }

    pub fn set_text(text: &str) -> Result<(), String> {
        main_thread()?;
        if text.len() > super::MAX_TEXT {
            return Err("text is too large for the clipboard".into());
        }
        // SAFETY: `setString:` takes an NSString that lives for the call; the pasteboard copies it, and
        // this runs on the main thread.
        unsafe { UIPasteboard::generalPasteboard().setString(Some(&NSString::from_str(text))) };
        Ok(())
    }

    pub fn open_url(value: &str) -> Result<(), String> {
        let mtm = main_thread()?;
        if !super::valid_url(value) {
            return Err("invalid URL".into());
        }
        let url = NSURL::URLWithString(&NSString::from_str(value)).ok_or_else(|| "invalid URL".to_string())?;
        let app = UIApplication::sharedApplication(mtm);
        if !app.canOpenURL(&url) {
            return Err("no application can open this URL".into());
        }
        let completion = block2::RcBlock::new(|success: objc2::runtime::Bool| {
            if !success.as_bool() {
                log::warn!("CADCraft system URL request was rejected");
            }
        });
        // SAFETY: Empty NSDictionary has the declared UIKit key/value generic types. App and URL
        // remain retained for the call on the proven main thread; UIKit copies the completion block
        // for asynchronous use, and it captures no objects.
        unsafe {
            app.openURL_options_completionHandler(&url, &NSDictionary::new(), Some(&completion));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Modifiers;

    fn press(key: Key, modifiers: Modifiers) -> Event {
        Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers }
    }

    fn command() -> Modifiers {
        Modifiers { command: true, mac_cmd: true, ..Default::default() }
    }

    #[test]
    fn command_c_x_v_become_clipboard_events() {
        let out = shortcut_events(vec![press(Key::C, command()), press(Key::X, command()), press(Key::V, command())], || Some("LINE".into()));
        assert!(matches!(out.as_slice(), [Event::Copy, Event::Cut, Event::Paste(text)] if text == "LINE"));
    }

    #[test]
    fn control_works_too_and_plain_letters_do_not() {
        let ctrl = Modifiers { ctrl: true, ..Default::default() };
        let out = shortcut_events(vec![press(Key::C, ctrl), press(Key::C, Modifiers::default())], || None);
        assert!(matches!(out.as_slice(), [Event::Copy, Event::Key { key: Key::C, .. }]));
    }

    #[test]
    fn paste_with_an_empty_clipboard_does_nothing_and_other_keys_pass_through() {
        let mut asked = 0;
        let out = shortcut_events(vec![press(Key::V, command()), press(Key::Z, command())], || {
            asked += 1;
            None
        });
        assert_eq!(asked, 1);
        assert!(matches!(out.as_slice(), [Event::Key { key: Key::Z, .. }]));
    }

    #[test]
    fn repeats_and_releases_are_left_alone() {
        let repeat = Event::Key { key: Key::V, physical_key: Some(Key::V), pressed: true, repeat: true, modifiers: command() };
        let release = Event::Key { key: Key::V, physical_key: Some(Key::V), pressed: false, repeat: false, modifiers: command() };
        let out = shortcut_events(vec![repeat, release], || Some("x".into()));
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|e| matches!(e, Event::Key { .. })));
    }

    #[test]
    fn only_plain_urls_are_handed_to_the_system() {
        assert!(valid_url("https://getartcraft.com/apps/cadcraft"));
        assert!(valid_url("mailto:someone@example.com"));
        for bad in ["", "no scheme", "https:", "1http://x", "https://a b", "ht tp://x", "https://x\n", ":x", "javascript alert"] {
            assert!(!valid_url(bad), "{bad:?}");
        }
    }
}
