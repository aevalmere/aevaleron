//! The margin around the bar and the output windows (lib/stage.js, PARITY
//! WIN-11). It held the glass's shadow; the native windows are opaque and
//! DWM draws their shadow, so it is 0 and the OS window is the glass. The
//! window manager still speaks the glass rectangle.

use crate::state::Rect;

pub const MARGIN: f64 = 0.0;

pub fn inflate(r: Rect, m: f64) -> Rect {
    Rect::new(r.x - m, r.y - m, r.width + 2.0 * m, r.height + 2.0 * m)
}

pub fn deflate(r: Rect, m: f64) -> Rect {
    Rect::new(
        r.x + m,
        r.y + m,
        (r.width - 2.0 * m).max(1.0),
        (r.height - 2.0 * m).max(1.0),
    )
}

/// A glass size limit as a window limit; 0 (no limit) stays 0.
pub fn limit(v: f64, m: f64) -> f64 {
    if v > 0.0 {
        v + 2.0 * m
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn margin_round_trip() {
        const M: f64 = 20.0;
        let g = Rect::new(100.0, 50.0, 420.0, 76.0);
        assert_eq!(inflate(g, M), Rect::new(80.0, 30.0, 460.0, 116.0));
        assert_eq!(deflate(inflate(g, M), M), g);
        assert_eq!(deflate(Rect::new(0.0, 0.0, 10.0, 10.0), M).width, 1.0);
        assert_eq!(limit(0.0, M), 0.0);
        assert_eq!(limit(320.0, M), 360.0);
        assert_eq!(inflate(g, MARGIN), g);
    }
}
