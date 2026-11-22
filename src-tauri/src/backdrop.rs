//! What is left of the backdrop behind the glass (lib/backdrop.js, PARITY
//! GLASS-01..09) now the glass is gone (lane native-ui): the pages are opaque
//! native surfaces, so there are no desktop frames to capture or serve. The
//! tier is always "solid", and the pages still hear `glass:tier` with the
//! stored intensity, which Settings shows and keeps.
//!
//! The contrast arithmetic (`contrast`) stays for the shot harness.

pub mod contrast;

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::shell::{AppHandle, Emitter};

/// The pages that said hello, which hear `glass:tier`.
static PAGES: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

fn pages() -> std::sync::MutexGuard<'static, BTreeSet<String>> {
    PAGES.lock().unwrap_or_else(|e| e.into_inner())
}

/// Why the glass is solid: there is no glass to draw, so no reason of the
/// old list (capture-visible, high-contrast, capture-failed, private-app)
/// applies.
pub fn reasons() -> Vec<&'static str> {
    Vec::new()
}

pub fn tier() -> &'static str {
    "solid"
}

/// The `glass:tier` payload.
fn tier_payload() -> Value {
    json!({ "tier": tier(), "reasons": reasons(), "intensity": intensity() })
}

/// Tier and intensity to every page that said hello.
pub fn broadcast_tier() {
    let labels: Vec<String> = pages().iter().cloned().collect();
    let payload = tier_payload();
    let app = AppHandle::get();
    for l in labels {
        let _ = app.emit_to(l.as_str(), "glass:tier", payload.clone());
    }
}

/// Stored `glassIntensity` (0..1, default 0.75), as bits. Settings owns the
/// value and calls `set_intensity`, which rebroadcasts the tier.
static INTENSITY: AtomicU64 = AtomicU64::new(0x3FE8_0000_0000_0000);

pub fn intensity() -> f64 {
    f64::from_bits(INTENSITY.load(Ordering::Relaxed))
}

pub fn set_intensity(k: f64) {
    let k = if k.is_finite() {
        (k.clamp(0.0, 1.0) * 100.0).round() / 100.0
    } else {
        crate::store::DEFAULT_GLASS_INTENSITY
    };
    INTENSITY.store(k.to_bits(), Ordering::Relaxed);
    broadcast_tier();
}

/// `glass:hello`: a page was built. It gets the tier now.
pub fn hello(label: &str) {
    pages().insert(label.to_string());
    let _ = AppHandle::get().emit_to(label, "glass:tier", tier_payload());
}

/// The window went (windows::on_destroyed).
pub fn forget(label: &str) {
    pages().remove(label);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intensity_defaults_to_liquid() {
        assert_eq!(intensity(), 0.75);
    }

    #[test]
    fn always_solid_with_no_reasons() {
        let p = tier_payload();
        assert_eq!(p["tier"], "solid");
        assert_eq!(p["reasons"], json!([]));
    }

    #[test]
    fn pages_are_remembered_until_forgotten() {
        pages().insert("test-page".into());
        assert!(pages().contains("test-page"));
        forget("test-page");
        assert!(!pages().contains("test-page"));
    }
}
