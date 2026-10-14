//! Audited UIKit boundary for Apple Pencil input. Main thread only; every callback only reads
//! values out of UIKit objects borrowed for the call and hands them to the safe
//! [`PencilBridge`](super::bridge::PencilBridge).
//!
//! Installed on the key `UIWindow` winit created (the window, not winit's view, so nothing here
//! depends on winit's private view class):
//!
//! - `TouchObserver`, a `UIGestureRecognizer` subclass that never recognizes and never delays or
//!   cancels touches. UIKit delivers each touch to a window's gesture recognizers before the
//!   hit-tested view. It reads every coalesced Pencil sample with tilt and roll.
//! - A `UIHoverGestureRecognizer` limited to Pencil touches: hover pose into the feed and the
//!   tip's position into egui's pointer (winit delivers no hover on iOS).
//!
//! UNVERIFIED on a device until the user reports: the hover recognizer's touch-type filter
//! excluding trackpad/mouse hover, and the hover position in egui points.

use super::bridge::{PenFeed, PencilBridge};
use super::mapping::PencilReading;
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_foundation::{NSArray, NSNumber, NSObject, NSObjectProtocol, NSSet};
use objc2_ui_kit::{UIEvent, UIGestureRecognizer, UIGestureRecognizerState, UIHoverGestureRecognizer, UITouch, UITouchType, UIWindow};
use std::cell::RefCell;

/// Objects UIKit holds weakly (target, delegate) or that must outlive the window's use of them.
struct Installed {
    _observer: Retained<TouchObserver>,
    _hover: Retained<UIHoverGestureRecognizer>,
    _target: Retained<PencilTarget>,
}

thread_local! {
    static BRIDGE: RefCell<Option<PencilBridge>> = const { RefCell::new(None) };
    static INSTALLED: RefCell<Option<Installed>> = const { RefCell::new(None) };
}

/// Run `f` on the bridge; a re-entrant callback (UIKit calling back into us from inside one) is
/// skipped rather than panicking on the `RefCell`.
fn with_bridge<R>(f: impl FnOnce(&mut PencilBridge) -> R) -> Option<R> {
    BRIDGE.with(|cell| match cell.try_borrow_mut() {
        Ok(mut guard) => guard.as_mut().map(f),
        Err(_) => {
            log::warn!("Apple Pencil callback re-entered; sample dropped");
            None
        }
    })
}

/// `rollAngle` exists from iOS 17.5 (the app deploys to 17.0): ask before sending it.
fn touch_roll(touch: &UITouch) -> Option<f64> {
    touch.respondsToSelector(sel!(rollAngle)).then(|| touch.rollAngle())
}

fn touch_reading(touch: &UITouch) -> PencilReading {
    PencilReading {
        force: touch.force(),
        max_force: touch.maximumPossibleForce(),
        altitude: touch.altitudeAngle(),
        // `None`: the window's coordinate space, the one winit reports touch positions in.
        azimuth: touch.azimuthAngleInView(None),
        roll: touch_roll(touch),
    }
}

/// Every sample UIKit coalesced into this event for `touch`, oldest first (the touch itself when
/// there are none).
fn coalesced(touch: &UITouch, event: Option<&UIEvent>) -> Vec<PencilReading> {
    let all = event.and_then(|e| e.coalescedTouchesForTouch(touch));
    match all {
        Some(list) if list.count() > 0 => list.iter().map(|t| touch_reading(&t)).collect(),
        _ => vec![touch_reading(touch)],
    }
}

#[derive(Clone, Copy)]
enum Phase {
    Began,
    Moved,
    Ended,
}

fn observe(touches: &NSSet<UITouch>, event: Option<&UIEvent>, phase: Phase) {
    for touch in touches.iter() {
        let pencil = touch.r#type() == UITouchType::Pencil;
        match (phase, pencil) {
            (Phase::Began, true) => {
                let readings = coalesced(&touch, event);
                with_bridge(|b| b.pen_began(&readings));
            }
            (Phase::Moved, true) => {
                let readings = coalesced(&touch, event);
                with_bridge(|b| b.pen_moved(&readings));
            }
            (Phase::Ended, true) => {
                with_bridge(PencilBridge::pen_ended);
            }
            _ => {}
        }
    }
}

define_class!(
    // SAFETY: UIGestureRecognizer may be subclassed (UIGestureRecognizerSubclass.h); this
    // subclass adds no ivars, is main-thread only like UIKit, and every override calls `super`
    // with the arguments it received, as UIKit requires.
    #[unsafe(super(UIGestureRecognizer, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "CADCraftPencilTouchObserver"]
    struct TouchObserver;

    impl TouchObserver {
        // SAFETY (all four): UIKit calls these on the main thread with a valid NSSet of UITouch
        // and an event that is borrowed for the duration of the call (taken as `Option` in case
        // UIKit passes nil). Nothing is retained past the call.
        #[unsafe(method(touchesBegan:withEvent:))]
        fn touches_began(&self, touches: &NSSet<UITouch>, event: Option<&UIEvent>) {
            // SAFETY: forwards the exact arguments to the superclass implementation.
            let _: () = unsafe { msg_send![super(self), touchesBegan: touches, withEvent: event] };
            observe(touches, event, Phase::Began);
        }

        #[unsafe(method(touchesMoved:withEvent:))]
        fn touches_moved(&self, touches: &NSSet<UITouch>, event: Option<&UIEvent>) {
            // SAFETY: as above.
            let _: () = unsafe { msg_send![super(self), touchesMoved: touches, withEvent: event] };
            observe(touches, event, Phase::Moved);
        }

        #[unsafe(method(touchesEnded:withEvent:))]
        fn touches_ended(&self, touches: &NSSet<UITouch>, event: Option<&UIEvent>) {
            // SAFETY: as above.
            let _: () = unsafe { msg_send![super(self), touchesEnded: touches, withEvent: event] };
            observe(touches, event, Phase::Ended);
            self.fail_when_idle(event);
        }

        #[unsafe(method(touchesCancelled:withEvent:))]
        fn touches_cancelled(&self, touches: &NSSet<UITouch>, event: Option<&UIEvent>) {
            // SAFETY: as above.
            let _: () = unsafe { msg_send![super(self), touchesCancelled: touches, withEvent: event] };
            observe(touches, event, Phase::Ended);
            self.fail_when_idle(event);
        }
    }
);

impl TouchObserver {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        // SAFETY: `init` is UIGestureRecognizer's inherited initializer (target and action nil);
        // the class has no ivars to initialize. Runs on the main thread (`mtm`).
        let this: Retained<Self> = unsafe { msg_send![Self::alloc(mtm), init] };
        // A pure observer: never hold back or cancel the touches winit's view receives.
        this.setCancelsTouchesInView(false);
        this.setDelaysTouchesBegan(false);
        this.setDelaysTouchesEnded(false);
        this
    }

    /// Once every touch of the sequence is over, leave `Possible` so UIKit resets the recognizer
    /// for the next sequence instead of keeping it pending.
    fn fail_when_idle(&self, event: Option<&UIEvent>) {
        use objc2_ui_kit::UITouchPhase;
        let live =
            event.and_then(UIEvent::allTouches).is_some_and(|all| all.iter().any(|t| !matches!(t.phase(), UITouchPhase::Ended | UITouchPhase::Cancelled)));
        if !live {
            self.setState(UIGestureRecognizerState::Failed);
        }
    }
}

fn hover_reading(recognizer: &UIHoverGestureRecognizer) -> PencilReading {
    // altitude/azimuth: iOS 16.4; roll: iOS 17.5. Missing ones read as an upright, unrolled pen.
    let has = |s| recognizer.respondsToSelector(s);
    PencilReading {
        force: 0.0,
        max_force: 1.0,
        altitude: if has(sel!(altitudeAngle)) { recognizer.altitudeAngle() } else { std::f64::consts::FRAC_PI_2 },
        azimuth: if has(sel!(azimuthAngleInView:)) { recognizer.azimuthAngleInView(None) } else { 0.0 },
        roll: has(sel!(rollAngle)).then(|| recognizer.rollAngle()),
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no ivars; main-thread only.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "CADCraftPencilTarget"]
    struct PencilTarget;

    impl PencilTarget {
        // SAFETY: the hover recognizer's action, `- (void)hover:(UIHoverGestureRecognizer *)`,
        // called by UIKit on the main thread with the recognizer it was registered on.
        #[unsafe(method(hover:))]
        fn hover(&self, recognizer: &UIHoverGestureRecognizer) {
            match recognizer.state() {
                UIGestureRecognizerState::Began | UIGestureRecognizerState::Changed => {
                    // `None`: window coordinates, like winit's touch positions.
                    let p = recognizer.locationInView(None);
                    let reading = hover_reading(recognizer);
                    with_bridge(|b| b.hover(Some((p.x, p.y)), &reading));
                }
                _ => {
                    with_bridge(PencilBridge::hover_ended);
                }
            }
        }
    }

    // SAFETY: NSObjectProtocol has no additional invariants.
    unsafe impl NSObjectProtocol for PencilTarget {}

);

impl PencilTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        // SAFETY: NSObject's `init` on a freshly allocated instance without ivars, main thread.
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}

fn key_window(mtm: MainThreadMarker) -> Result<Retained<UIWindow>, String> {
    let window = crate::scenes::window(mtm);
    window.ok_or_else(|| "No UIKit window to attach Apple Pencil input to".into())
}

/// Attach Apple Pencil input to the app's window: samples go to `feed`, hover moves egui's
/// pointer through a plugin on `ctx`. Call once on the main thread after winit created the
/// window (eframe's app-creation callback). A second call is a no-op.
pub fn install(feed: PenFeed, ctx: &egui::Context) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("Apple Pencil input must be installed on the main thread")?;
    if INSTALLED.with(|i| i.borrow().is_some()) {
        return Ok(());
    }
    let window = key_window(mtm)?;
    let bridge = PencilBridge::new(feed, Some(ctx.clone()));
    ctx.add_plugin(bridge.hover_injector());
    BRIDGE.with(|cell| match cell.try_borrow_mut() {
        Ok(mut b) => {
            *b = Some(bridge);
            Ok(())
        }
        Err(_) => Err("Apple Pencil bridge is busy".to_string()),
    })?;

    let observer = TouchObserver::new(mtm);
    window.addGestureRecognizer(&observer);

    let target = PencilTarget::new(mtm);
    // SAFETY: `target` implements `hover:` with the action signature UIKit uses, and is kept
    // alive in `INSTALLED` for the app's lifetime (UIKit does not retain targets).
    let hover = unsafe { UIHoverGestureRecognizer::initWithTarget_action(UIHoverGestureRecognizer::alloc(mtm), Some(&target), Some(sel!(hover:))) };
    // Pencil hover only, not trackpad/mouse pointer hover (that is `indirectPointer`).
    hover.setAllowedTouchTypes(&NSArray::from_retained_slice(&[NSNumber::new_isize(UITouchType::Pencil.0)]));
    hover.setCancelsTouchesInView(false);
    window.addGestureRecognizer(&hover);


    INSTALLED.with(|i| {
        if let Ok(mut i) = i.try_borrow_mut() {
            *i = Some(Installed { _observer: observer, _hover: hover, _target: target });
        }
    });
    log::info!("Apple Pencil input installed (pressure, tilt, roll, hover)");
    Ok(())
}
