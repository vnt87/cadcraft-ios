//! Platform-free Apple Pencil state machine. The UIKit callbacks in `native.rs` only read raw
//! values and call into a [`PencilBridge`]; what reaches the app (the [`PenFeed`] and the hover
//! pointer) is decided here and unit-tested on any host.
//!
//! What winit already delivers: every `UITouch`, Pencil included, as `WindowEvent::Touch` with
//! `Force::Calibrated`, which egui-winit turns into `egui::Event::Touch { force }` plus an emulated
//! mouse pointer that follows the first touch. Tilt, roll, coalesced high-rate samples and the
//! touch type are lost, and hover is not delivered at all. So the bridge adds only what winit
//! drops: the pen sample with tilt and roll, and the hover position as an egui pointer move
//! ([`HoverInjector`]), which moves the crosshair and snap markers while the tip floats.

use super::mapping::{PenSample, PencilReading, contact_sample, hover_sample};
use std::sync::{Arc, Mutex, PoisonError};

/// The latest pen sample, shared between the UIKit callbacks and the app. `None` when the Pencil
/// is neither touching nor hovering.
#[derive(Clone, Debug, Default)]
pub struct PenFeed(Arc<Mutex<Option<PenSample>>>);

impl PenFeed {
    /// The latest sample. No CAD tool reads pressure or tilt yet (drafting has no brush), so only
    /// tests call this until one does.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn get(&self) -> Option<PenSample> {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set(&self, sample: Option<PenSample>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = sample;
    }
}

/// Where the hovering pen is, for the egui pointer (UIKit window points).
#[derive(Debug, Default)]
struct HoverShared {
    /// Latest hover position not yet handed to egui.
    pending: Option<(f32, f32)>,
    /// The hover ended (or turned into a touch): egui's pointer should go.
    gone: bool,
}

/// Owns the pen state between UIKit callbacks. Main-thread only in practice (UIKit), but holds
/// nothing thread-bound.
pub struct PencilBridge {
    feed: PenFeed,
    /// A Pencil touch is down (hover callbacks are ignored meanwhile).
    touching: bool,
    hover: Arc<Mutex<HoverShared>>,
    repaint: Option<egui::Context>,
}

impl PencilBridge {
    pub fn new(feed: PenFeed, repaint: Option<egui::Context>) -> Self {
        Self { feed, touching: false, hover: Arc::default(), repaint }
    }

    /// The egui plugin that turns hover positions into pointer moves.
    pub fn hover_injector(&self) -> HoverInjector {
        HoverInjector { shared: Arc::clone(&self.hover) }
    }

    fn wake(&self) {
        if let Some(ctx) = &self.repaint {
            ctx.request_repaint();
        }
    }

    fn hover_gone(&self) {
        let mut h = self.hover.lock().unwrap_or_else(PoisonError::into_inner);
        h.pending = None;
        h.gone = true;
    }

    /// A Pencil touch began, with its coalesced readings, oldest first.
    pub fn pen_began(&mut self, readings: &[PencilReading]) {
        self.touching = true;
        self.hover_gone();
        self.pen_moved(readings);
    }

    /// Coalesced readings of a Pencil touch that moved (or changed force), oldest first. The feed
    /// keeps the newest.
    pub fn pen_moved(&mut self, readings: &[PencilReading]) {
        if let Some(r) = readings.last() {
            self.feed.set(Some(contact_sample(r)));
        }
    }

    /// The Pencil touch ended or was cancelled.
    pub fn pen_ended(&mut self) {
        self.touching = false;
        self.feed.set(None);
    }

    /// The Pencil hovers at `pos` (window points) with `reading`'s pose.
    pub fn hover(&mut self, pos: Option<(f64, f64)>, reading: &PencilReading) {
        if self.touching {
            return;
        }
        self.feed.set(Some(hover_sample(reading)));
        if let Some((x, y)) = pos.filter(|(x, y)| x.is_finite() && y.is_finite()) {
            let mut h = self.hover.lock().unwrap_or_else(PoisonError::into_inner);
            h.pending = Some((x as f32, y as f32));
            h.gone = false;
        }
        self.wake();
    }

    /// The Pencil stopped hovering (left the range, or touched down).
    pub fn hover_ended(&mut self) {
        if self.touching {
            return;
        }
        self.feed.set(None);
        self.hover_gone();
        self.wake();
    }
}

/// egui plugin: turns the hovering pen's position into `PointerMoved` / `PointerGone`, so the
/// crosshair follows the floating tip (winit delivers no hover on iOS).
pub struct HoverInjector {
    shared: Arc<Mutex<HoverShared>>,
}

impl HoverInjector {
    /// Append the pending hover events to `events`. `zoom` is egui's zoom factor: UIKit window
    /// points are egui points at zoom 1. A frame where a touch is down or moving keeps the touch's
    /// pointer: the hover is dropped (it would fight the stroke).
    pub fn inject(&self, events: &mut Vec<egui::Event>, zoom: f32) {
        let mut h = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        let touching = events.iter().any(|e| matches!(e, egui::Event::Touch { phase: egui::TouchPhase::Start | egui::TouchPhase::Move, .. }));
        let pending = h.pending.take();
        if std::mem::take(&mut h.gone) && !touching {
            events.push(egui::Event::PointerGone);
        }
        if touching {
            return;
        }
        let zoom = if zoom.is_finite() && zoom > 0.0 { zoom } else { 1.0 };
        if let Some((x, y)) = pending {
            let p = egui::pos2(x / zoom, y / zoom);
            if p.x.is_finite() && p.y.is_finite() {
                events.push(egui::Event::PointerMoved(p));
            }
        }
    }
}

impl egui::Plugin for HoverInjector {
    fn debug_name(&self) -> &'static str {
        "cadcraft-ios pencil hover"
    }

    fn input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        self.inject(&mut input.events, ctx.zoom_factor());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::FRAC_PI_4;

    fn reading(force: f64) -> PencilReading {
        PencilReading { force, max_force: 4.0, altitude: FRAC_PI_4, azimuth: 0.0, roll: None }
    }

    fn bridge() -> (PencilBridge, PenFeed) {
        let feed = PenFeed::default();
        (PencilBridge::new(feed.clone(), None), feed)
    }

    #[test]
    fn a_touch_feeds_the_newest_coalesced_sample() {
        let (mut b, feed) = bridge();
        b.pen_began(&[reading(1.0), reading(2.0)]);
        assert_eq!(feed.get().map(|s| s.pressure), Some(0.5));
        b.pen_moved(&[reading(3.0)]);
        assert_eq!(feed.get().map(|s| s.pressure), Some(0.75));
        assert!((feed.get().map_or(0.0, |s| s.tilt_x) - 45.0).abs() < 1e-3);
        b.pen_ended();
        assert_eq!(feed.get(), None);
    }

    #[test]
    fn hover_feeds_zero_pressure_and_is_ignored_while_touching() {
        let (mut b, feed) = bridge();
        let inj = b.hover_injector();
        b.hover(Some((100.0, 50.0)), &reading(3.0));
        assert_eq!(feed.get().map(|s| s.pressure), Some(0.0));
        let mut ev = Vec::new();
        inj.inject(&mut ev, 2.0);
        assert_eq!(ev, vec![egui::Event::PointerMoved(egui::pos2(50.0, 25.0))]);
        b.pen_began(&[reading(2.0)]);
        b.hover(Some((1.0, 1.0)), &reading(0.0));
        assert_eq!(feed.get().map(|s| s.pressure), Some(0.5), "touching wins over a late hover");
        b.hover_ended();
        assert!(feed.get().is_some(), "a hover end during the touch changes nothing");
        b.pen_ended();
        b.hover_ended();
        assert_eq!(feed.get(), None);
    }

    #[test]
    fn the_injector_skips_touch_frames_and_hostile_values() {
        let (mut b, _) = bridge();
        let inj = b.hover_injector();
        b.hover(Some((f64::NAN, 3.0)), &reading(0.0));
        let mut ev = Vec::new();
        inj.inject(&mut ev, 1.0);
        assert!(ev.is_empty(), "no position, no move");
        b.hover(Some((10.0, 20.0)), &reading(0.0));
        let start = egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: egui::TouchId(1),
            phase: egui::TouchPhase::Start,
            pos: egui::pos2(0.0, 0.0),
            force: None,
        };
        let mut ev = vec![start.clone()];
        inj.inject(&mut ev, 1.0);
        assert_eq!(ev, vec![start], "a touch keeps the pointer");
        b.hover(Some((10.0, 20.0)), &reading(0.0));
        let mut ev = Vec::new();
        inj.inject(&mut ev, f32::NAN);
        assert_eq!(ev, vec![egui::Event::PointerMoved(egui::pos2(10.0, 20.0))], "bad zoom falls back to 1");
        b.hover_ended();
        let mut ev = Vec::new();
        inj.inject(&mut ev, 1.0);
        assert_eq!(ev, vec![egui::Event::PointerGone]);
        let mut ev = Vec::new();
        inj.inject(&mut ev, 1.0);
        assert!(ev.is_empty(), "reported once");
    }
}
