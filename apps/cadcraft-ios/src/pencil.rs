//! Apple Pencil input: hover, pressure, tilt and barrel roll.
//!
//! - [`mapping`]: pure UIKit-units -> [`PenSample`] conversion (host-tested).
//! - [`bridge`]: pure state machine between UIKit callbacks and the app: the latest [`PenSample`]
//!   ([`PenFeed`]) and the hover position as an egui pointer move (host-tested).
//! - `native` (iOS only): the audited UIKit boundary that reads `UITouch` and
//!   `UIHoverGestureRecognizer` and calls into the bridge.

pub mod bridge;
pub mod mapping;

#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
mod native;
#[cfg(target_os = "ios")]
pub use native::install;
