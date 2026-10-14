//! Pure Apple Pencil -> [`PenSample`] conversion. No UIKit types: the native bridge reads the
//! raw `UITouch` / `UIHoverGestureRecognizer` values into a [`PencilReading`] and this module
//! maps them to the W3C Pointer Events convention:
//!
//! - pressure 0..1 (0 = no contact: hovering or lifting),
//! - tilt X / tilt Y in degrees -90..90 (positive towards +x / +y),
//! - rotation (barrel roll) in degrees 0..360.
//!
//! UIKit reports the pose spherically (`altitudeAngle`, `azimuthAngleInView:`), the same spherical
//! convention as W3C `altitudeAngle` / `azimuthAngle`, so the W3C spherical-to-tilt conversion
//! applies unchanged.
//!
//! UNVERIFIED on hardware: that UIKit's azimuth points to the pen's cap (W3C) and not its tip, and
//! the zero and sign of `rollAngle` relative to W3C `twist`.

use std::f64::consts::{FRAC_PI_2, TAU};

/// Below this, `cos`/`sin` of an azimuth on an axis is treated as exactly 0 (pi/2 is not
/// representable, so `cos(pi/2)` is ~6e-17, which would otherwise turn a flat pen pointing
/// straight down into a 90 degree tilt X).
const AXIS_EPS: f64 = 1.0e-9;

/// One pen sample: pressure 0..1 (0 = no contact), tilt in degrees -90..90, rotation 0..360.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PenSample {
    pub pressure: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
    pub rotation: f32,
}

impl PenSample {
    /// Replace non-finite values and clamp every field to its documented range.
    fn sanitized(self) -> Self {
        let fix = |v: f32, lo: f32, hi: f32, fallback: f32| if v.is_finite() { v.clamp(lo, hi) } else { fallback };
        Self {
            pressure: fix(self.pressure, 0.0, 1.0, 1.0),
            tilt_x: fix(self.tilt_x, -90.0, 90.0, 0.0),
            tilt_y: fix(self.tilt_y, -90.0, 90.0, 0.0),
            rotation: fix(self.rotation, 0.0, 360.0, 0.0),
        }
    }
}

/// Raw values read from one Apple Pencil touch or hover event (UIKit units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PencilReading {
    /// `UITouch.force` (unitless; 1.0 is an average touch). Ignored while hovering.
    pub force: f64,
    /// `UITouch.maximumPossibleForce`.
    pub max_force: f64,
    /// `altitudeAngle`, radians: 0 flat on the glass, pi/2 upright.
    pub altitude: f64,
    /// `azimuthAngleInView:`, radians, 0 along +x, increasing towards +y.
    pub azimuth: f64,
    /// `rollAngle` in radians (Apple Pencil Pro on iOS 17.5+); `None` when not reported.
    pub roll: Option<f64>,
}

impl Default for PencilReading {
    /// An upright pen at full pressure with no roll.
    fn default() -> Self {
        Self { force: 1.0, max_force: 1.0, altitude: FRAC_PI_2, azimuth: 0.0, roll: None }
    }
}

/// `force / max_force` clamped to 0..1. A contact whose force cannot be normalised (zero,
/// negative or non-finite maximum, or a non-finite force) paints at full pressure, as a mouse
/// does and as `PenSample::sanitized` treats a non-finite pressure: never at 0, which the stylus
/// code reads as "not touching".
pub fn pressure(force: f64, max_force: f64) -> f32 {
    if !force.is_finite() || !max_force.is_finite() || max_force <= 0.0 {
        return 1.0;
    }
    (force / max_force).clamp(0.0, 1.0) as f32
}

/// Azimuth wrapped to [0, 2pi); `None` for a non-finite value.
fn wrap_azimuth(azimuth: f64) -> Option<f64> {
    if !azimuth.is_finite() {
        return None;
    }
    // `rem_euclid` can round a tiny negative angle up to exactly 2pi.
    let a = azimuth.rem_euclid(TAU);
    Some(if a >= TAU { 0.0 } else { a })
}

fn snap_axis(v: f64) -> f64 {
    if v.abs() < AXIS_EPS { 0.0 } else { v }
}

/// W3C Pointer Events spherical -> planar tilt, degrees `(tilt_x, tilt_y)`, each -90..90.
///
/// `tilt_x = atan(cos(azimuth) / tan(altitude))`, `tilt_y = atan(sin(azimuth) / tan(altitude))`,
/// written with `atan2` so a pen lying flat (altitude 0) gives +-90 on the axes it leans along
/// instead of dividing by zero (the W3C special cases). Altitude is clamped to 0..pi/2; a
/// non-finite altitude or azimuth gives an upright pen (0, 0).
pub fn tilt_degrees(altitude: f64, azimuth: f64) -> (f32, f32) {
    let Some(az) = wrap_azimuth(azimuth) else { return (0.0, 0.0) };
    if !altitude.is_finite() {
        return (0.0, 0.0);
    }
    let alt = altitude.clamp(0.0, FRAC_PI_2);
    let (cos_az, sin_az) = (snap_axis(az.cos()), snap_axis(az.sin()));
    let (cos_alt, sin_alt) = (snap_axis(alt.cos()), alt.sin());
    let tx = (cos_az * cos_alt).atan2(sin_alt).to_degrees();
    let ty = (sin_az * cos_alt).atan2(sin_alt).to_degrees();
    (tx.clamp(-90.0, 90.0) as f32, ty.clamp(-90.0, 90.0) as f32)
}

/// Barrel roll in radians -> rotation in degrees [0, 360); 0 when absent or non-finite.
pub fn rotation_degrees(roll: Option<f64>) -> f32 {
    match roll {
        Some(r) if r.is_finite() => {
            // A tiny negative roll is just under 360 in f64 and rounds to 360.0 in f32.
            let d = r.to_degrees().rem_euclid(360.0) as f32;
            if d >= 360.0 { 0.0 } else { d }
        }
        _ => 0.0,
    }
}

/// A pen in contact with the glass.
pub fn contact_sample(r: &PencilReading) -> PenSample {
    let (tilt_x, tilt_y) = tilt_degrees(r.altitude, r.azimuth);
    PenSample { pressure: pressure(r.force, r.max_force), tilt_x, tilt_y, rotation: rotation_degrees(r.roll) }.sanitized()
}

/// A pen hovering above the glass: pressure 0 (the stylus code treats it as "no contact" and
/// keeps showing the brush cursor), with the pose when the OS reports one.
pub fn hover_sample(r: &PencilReading) -> PenSample {
    PenSample { pressure: 0.0, ..contact_sample(r) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_4, PI};

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn pressure_normalises_and_clamps() {
        assert!(close(pressure(2.0, 4.0), 0.5));
        assert_eq!(pressure(0.0, 4.0), 0.0);
        assert_eq!(pressure(9.0, 4.0), 1.0, "over the maximum clamps");
        assert_eq!(pressure(-1.0, 4.0), 0.0, "negative force clamps");
    }

    #[test]
    fn pressure_guards_zero_negative_and_non_finite_maximum() {
        for max in [0.0, -0.0, -3.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(pressure(1.0, max), 1.0, "max {max}");
        }
        for force in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(pressure(force, 4.0), 1.0, "force {force}");
        }
        // Tiny but positive maximum: still a ratio, still clamped.
        assert_eq!(pressure(1.0, f64::MIN_POSITIVE), 1.0);
    }

    #[test]
    fn an_upright_pen_has_no_tilt_whatever_the_azimuth() {
        for az in [0.0, 1.0, PI, 4.0, -2.0, 100.0] {
            let (x, y) = tilt_degrees(FRAC_PI_2, az);
            assert!(close(x, 0.0) && close(y, 0.0), "az {az}: {x} {y}");
        }
        // Above pi/2 clamps to upright.
        assert_eq!(tilt_degrees(2.0, 0.3), tilt_degrees(FRAC_PI_2, 0.3));
    }

    #[test]
    fn leaning_along_an_axis_tilts_only_that_axis() {
        // 45 degree altitude: the pen leans 45 degrees.
        let (x, y) = tilt_degrees(FRAC_PI_4, 0.0);
        assert!(close(x, 45.0) && close(y, 0.0), "{x} {y}");
        let (x, y) = tilt_degrees(FRAC_PI_4, FRAC_PI_2);
        assert!(close(x, 0.0) && close(y, 45.0), "towards +y (the user): {x} {y}");
        let (x, y) = tilt_degrees(FRAC_PI_4, PI);
        assert!(close(x, -45.0) && close(y, 0.0), "{x} {y}");
        let (x, y) = tilt_degrees(FRAC_PI_4, 3.0 * FRAC_PI_2);
        assert!(close(x, 0.0) && close(y, -45.0), "{x} {y}");
    }

    #[test]
    fn matches_the_w3c_formula_off_axis() {
        let (alt, az) = (0.7_f64, 0.9_f64);
        let (x, y) = tilt_degrees(alt, az);
        let wx = (az.cos() / alt.tan()).atan().to_degrees() as f32;
        let wy = (az.sin() / alt.tan()).atan().to_degrees() as f32;
        assert!(close(x, wx) && close(y, wy), "{x} {y} vs {wx} {wy}");
    }

    #[test]
    fn a_flat_pen_follows_the_w3c_special_cases() {
        // Altitude 0 (and below, clamped): +-90 on each axis the pen leans along.
        assert_eq!(tilt_degrees(0.0, 0.0), (90.0, 0.0));
        assert_eq!(tilt_degrees(0.0, FRAC_PI_2), (0.0, 90.0));
        assert_eq!(tilt_degrees(0.0, PI), (-90.0, 0.0));
        assert_eq!(tilt_degrees(0.0, 3.0 * FRAC_PI_2), (0.0, -90.0));
        assert_eq!(tilt_degrees(-1.0, TAU), (90.0, 0.0));
        // Between the axes both tilts are +-90.
        assert_eq!(tilt_degrees(0.0, FRAC_PI_4), (90.0, 90.0));
        assert_eq!(tilt_degrees(0.0, PI - FRAC_PI_4), (-90.0, 90.0));
        assert_eq!(tilt_degrees(0.0, PI + FRAC_PI_4), (-90.0, -90.0));
        assert_eq!(tilt_degrees(0.0, TAU - FRAC_PI_4), (90.0, -90.0));
    }

    #[test]
    fn azimuth_wraps() {
        let base = tilt_degrees(0.6, 0.5);
        for k in [-3.0, -1.0, 1.0, 7.0] {
            let (x, y) = tilt_degrees(0.6, 0.5 + k * TAU);
            assert!(close(x, base.0) && close(y, base.1), "k {k}: {x} {y}");
        }
        // Just below zero is just below a full turn, not a jump.
        let (x, y) = tilt_degrees(FRAC_PI_4, -1e-12);
        assert!(close(x, 45.0) && close(y, 0.0), "{x} {y}");
    }

    #[test]
    fn non_finite_angles_mean_upright() {
        for (alt, az) in [(f64::NAN, 0.0), (0.5, f64::NAN), (f64::INFINITY, 0.0), (0.5, f64::NEG_INFINITY)] {
            assert_eq!(tilt_degrees(alt, az), (0.0, 0.0), "{alt} {az}");
        }
    }

    #[test]
    fn roll_maps_to_rotation_in_degrees() {
        assert_eq!(rotation_degrees(None), 0.0);
        assert_eq!(rotation_degrees(Some(f64::NAN)), 0.0);
        assert_eq!(rotation_degrees(Some(f64::INFINITY)), 0.0);
        assert!(close(rotation_degrees(Some(FRAC_PI_2)), 90.0));
        assert!(close(rotation_degrees(Some(-FRAC_PI_2)), 270.0), "negative roll wraps");
        assert!(close(rotation_degrees(Some(5.0 * PI)), 180.0));
        let r = rotation_degrees(Some(-1e-15));
        assert!((0.0..360.0).contains(&r), "{r}");
    }

    #[test]
    fn contact_and_hover_samples() {
        let r = PencilReading { force: 1.0, max_force: 4.0, altitude: FRAC_PI_4, azimuth: 0.0, roll: Some(PI) };
        let s = contact_sample(&r);
        assert!(close(s.pressure, 0.25) && close(s.tilt_x, 45.0) && close(s.tilt_y, 0.0) && close(s.rotation, 180.0));
        let h = hover_sample(&r);
        assert_eq!(h.pressure, 0.0, "hover never paints");
        assert_eq!((h.tilt_x, h.tilt_y, h.rotation), (s.tilt_x, s.tilt_y, s.rotation));
        // Garbage in, documented ranges out.
        let g = contact_sample(&PencilReading { force: f64::NAN, max_force: 0.0, altitude: f64::NAN, azimuth: f64::INFINITY, roll: Some(f64::NAN) });
        assert_eq!(g, PenSample { pressure: 1.0, tilt_x: 0.0, tilt_y: 0.0, rotation: 0.0 });
    }
}
