//! Spatial-audio session layer.
//!
//! ## Why this exists
//!
//! macOS gates the head-tracking stream on a host-side declaration that Linux
//! has no equivalent for. The chain, from `bluetoothd`'s own log:
//!
//! ```text
//! Spatial Audio mode for App: com.apple.WebKit.GPU From User (via BTHAL) is : 3
//! DeviceManager Set Spatial Mode: bundleID: ..., mode: 3 head Tracking 1
//! AudioLinkManager setSpatialModeWithBundleID: ..., mode:3, headtrack:1
//! Notify Spatial Audio Mode changed ... to 3 ... Head Tracking YES
//! Sent custom message ... type 0x800, len 14      <- 59 ms later
//! ```
//!
//! Three things have to be true before that enable is allowed out:
//!
//! 1. an application has claimed spatial audio,
//! 2. the requested mode is `HeadTracked` (3), and
//! 3. head tracking is explicitly on for that claim.
//!
//! On the device side the enable travels as a *downlink* for a registered
//! custom-message client, which is why sending it unprompted gets no reply:
//! `bluetoothd` shows `kCBMsgIdAccessoryRegisterCustomMessageClientMsg` and
//! `SubscribeCustomMessage types:` as the mechanism that creates that context,
//! and neither produces an AACP frame.
//!
//! This module models the state machine so the rest of the program can make the
//! decision "the host is in head-tracked spatial audio" the same way macOS does,
//! instead of firing an enable into the void.
//!
//! ## Relationship to what the accessory answers
//!
//! It is important to be precise about what this buys us. The probe in
//! `tools/ht_probe.py` established that the accessory returns nothing for any
//! enable, including ones carrying this session's real service id and mode 3.
//! So this layer is necessary but not yet sufficient: it reproduces the host
//! side of the chain, and the remaining unknown is whether an accessory accepts
//! the enable once a session exists, or insists on something that only the
//! macOS Bluetooth stack can produce.
//!
//! Keeping the state machine explicit is still the right move: it makes the
//! precondition testable, and if the accessory starts answering, this is where
//! that decision belongs.

use std::time::{Duration, Instant};
use tracing::{debug, info};

/// Spatial audio mode reported by the host audio stack.
///
/// The values match what `bluetoothd` logs (`mode: 3 head Tracking 1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpatialMode {
    /// Spatialisation off.
    Off,
    /// Fixed spatialisation: the stage does not follow the head.
    Fixed,
    /// The stage follows the head, but without sensor input.
    HeadAnchored,
    /// Head tracked: the accessory's motion sensors drive the stage.
    HeadTracked,
}

impl SpatialMode {
    pub fn as_u8(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Fixed => 1,
            Self::HeadAnchored => 2,
            Self::HeadTracked => 3,
        }
    }
}

/// One application's claim on spatial audio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpatialClaim {
    /// Identifier of the claiming application, the analogue of macOS' bundle id.
    pub app_id: String,
    pub mode: SpatialMode,
    /// Whether the application asked for sensor-driven head tracking.
    pub head_tracking: bool,
    pub claimed_at: Instant,
}

impl SpatialClaim {
    pub fn new(app_id: impl Into<String>, mode: SpatialMode, head_tracking: bool) -> Self {
        Self {
            app_id: app_id.into(),
            mode,
            head_tracking,
            claimed_at: Instant::now(),
        }
    }

    /// Whether this claim is the state macOS requires before enabling.
    pub fn permits_head_tracking(&self) -> bool {
        self.mode == SpatialMode::HeadTracked && self.head_tracking
    }
}

/// Tracks which application currently holds the spatial-audio claim.
///
/// Only one claim is honoured at a time, matching the accessory's single
/// head-tracking stream. A newer claim replaces an older one, which mirrors how
/// macOS hands the stage to whichever app is producing audio.
#[derive(Debug, Default)]
pub struct SpatialSession {
    current: Option<SpatialClaim>,
    /// Set once the enable has been sent for the current claim.
    enable_sent_for: Option<String>,
}

impl SpatialSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current(&self) -> Option<&SpatialClaim> {
        self.current.as_ref()
    }

    /// Records that an application wants spatial audio.
    ///
    /// Returns `true` when this call changed the state in a way that requires an
    /// enable to be sent — that is, the claim now permits head tracking and no
    /// enable has gone out for this exact claim yet.
    pub fn claim(&mut self, claim: SpatialClaim) -> bool {
        let previous = self.current.replace(claim.clone());

        if previous.as_ref() == Some(&claim) {
            return false;
        }

        info!(
            "Spatial session claimed by '{}' mode={:?} head_tracking={}",
            claim.app_id, claim.mode, claim.head_tracking
        );

        if claim.permits_head_tracking() && self.enable_sent_for.as_deref() != Some(&claim.app_id) {
            // A new app taking the claim invalidates any previous enable.
            self.enable_sent_for = None;
            return true;
        }
        false
    }

    /// Records that an application released its claim.
    ///
    /// Returns `true` when head tracking was active and therefore has to be
    /// stopped.
    pub fn release(&mut self, app_id: &str) -> bool {
        let Some(claim) = self.current.as_ref() else {
            return false;
        };
        if claim.app_id != app_id {
            debug!(
                "Ignoring release from '{}'; the claim belongs to '{}'",
                app_id, claim.app_id
            );
            return false;
        }
        let was_tracking = claim.permits_head_tracking();
        info!("Spatial session released by '{}'", app_id);
        self.current = None;
        self.enable_sent_for = None;
        was_tracking
    }

    /// Whether the accessory should currently be streaming head tracking.
    pub fn wants_head_tracking(&self) -> bool {
        self.current
            .as_ref()
            .map(|c| c.permits_head_tracking())
            .unwrap_or(false)
    }

    /// Marks the enable as sent for the current claim, so it is not repeated on
    /// every poll.
    pub fn mark_enable_sent(&mut self) {
        if let Some(claim) = self.current.as_ref() {
            self.enable_sent_for = Some(claim.app_id.clone());
        }
    }

    /// Drops claims older than `max_age`.
    ///
    /// An application that crashes never releases its claim, and macOS treats a
    /// missing audio stream as an implicit release. Without this a stale claim
    /// would keep the accessory streaming forever.
    pub fn expire_stale(&mut self, max_age: Duration) -> bool {
        let Some(claim) = self.current.as_ref() else {
            return false;
        };
        if claim.claimed_at.elapsed() <= max_age {
            return false;
        }
        let was_tracking = claim.permits_head_tracking();
        info!(
            "Spatial claim by '{}' expired after {:?}",
            claim.app_id, max_age
        );
        self.current = None;
        self.enable_sent_for = None;
        was_tracking
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracking_claim(app: &str) -> SpatialClaim {
        SpatialClaim::new(app, SpatialMode::HeadTracked, true)
    }

    #[test]
    fn mode_values_match_macos_logging() {
        // bluetoothd logs `mode: 3 head Tracking 1` for the head-tracked case.
        assert_eq!(SpatialMode::HeadTracked.as_u8(), 3);
        assert_eq!(SpatialMode::HeadAnchored.as_u8(), 2);
    }

    #[test]
    fn head_tracked_claim_requests_enable() {
        let mut session = SpatialSession::new();
        assert!(session.claim(tracking_claim("com.example.Player")));
        assert!(session.wants_head_tracking());
    }

    #[test]
    fn fixed_mode_does_not_request_enable() {
        let mut session = SpatialSession::new();
        assert!(!session.claim(SpatialClaim::new("app", SpatialMode::Fixed, true)));
        assert!(!session.wants_head_tracking());
    }

    #[test]
    fn head_tracked_without_tracking_flag_does_not_request_enable() {
        let mut session = SpatialSession::new();
        assert!(!session.claim(SpatialClaim::new("app", SpatialMode::HeadTracked, false)));
        assert!(!session.wants_head_tracking());
    }

    #[test]
    fn repeated_claim_does_not_re_request_enable() {
        let mut session = SpatialSession::new();
        assert!(session.claim(tracking_claim("app")));
        session.mark_enable_sent();
        // Same claim arriving again must not spam the accessory.
        assert!(!session.claim(tracking_claim("app")));
    }

    #[test]
    fn a_different_app_takes_the_claim_and_re_requests() {
        let mut session = SpatialSession::new();
        assert!(session.claim(tracking_claim("first")));
        session.mark_enable_sent();
        // macOS hands the stage to whoever is producing audio.
        assert!(session.claim(tracking_claim("second")));
        assert_eq!(session.current().unwrap().app_id, "second");
    }

    #[test]
    fn release_reports_whether_tracking_had_to_stop() {
        let mut session = SpatialSession::new();
        session.claim(tracking_claim("app"));
        assert!(session.release("app"));
        assert!(!session.wants_head_tracking());
        // Releasing again is a no-op.
        assert!(!session.release("app"));
    }

    #[test]
    fn release_from_a_different_app_is_ignored() {
        let mut session = SpatialSession::new();
        session.claim(tracking_claim("owner"));
        assert!(!session.release("intruder"));
        assert!(session.wants_head_tracking());
    }

    #[test]
    fn stale_claim_expires() {
        let mut session = SpatialSession::new();
        session.claim(tracking_claim("crashed-app"));
        // Zero max-age expires immediately.
        assert!(session.expire_stale(Duration::from_nanos(0)));
        assert!(!session.wants_head_tracking());
    }

    #[test]
    fn fresh_claim_survives_expiry_check() {
        let mut session = SpatialSession::new();
        session.claim(tracking_claim("app"));
        assert!(!session.expire_stale(Duration::from_secs(3600)));
        assert!(session.wants_head_tracking());
    }
}
