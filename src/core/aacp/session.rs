//! Drives the RemoteHID head-tracking stream from start to finish.
//!
//! This is where the two things that actually make AirPods 3 head tracking work
//! live together:
//!
//! 1. **The receive MTU must be 2582.** With the kernel default of 672 the
//!    accessory silently omits every service whose descriptor does not fit, and
//!    that set includes `devmotion6` (833 bytes) — the motion sensor. Setting
//!    the MTU is handled in [`super`] before `connect()`.
//!
//! 2. **A stuck accessory has to be reset, and only a full link drop clears it.**
//!    AirPods 3 can end up in a state where it stops announcing its sensors even
//!    with both buds in ear; reconnecting immediately leaves it stuck. Waiting
//!    between the disconnect and the reconnect is part of the fix, which is why
//!    this module owns the retry loop rather than leaving it to the caller.
//!
//! The sequence itself is short: prelude, collect DeviceConnect frames, then a
//! SetReport writing Feature report 1 with the sample interval.
//!
//! Streams are cancelled when the accessory re-announces its devices (it happens
//! on a primary-bud switch), so the SetReport is re-sent whenever a DeviceConnect
//! arrives after the stream is already running.

use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use super::remotehid::{self, DeviceRegistry};

/// Sample interval written to Feature report 1, in microseconds.
pub const DEFAULT_INTERVAL_US: u32 = 10_000;

/// How long to wait for the DeviceConnect burst after the prelude.
const ANNOUNCE_WINDOW: Duration = Duration::from_secs(5);

/// Gap between the disconnect and the reconnect when recovering.
///
/// This is a narrow window. Too short and the accessory has not finished tearing
/// the link down; too long and it stops answering paging entirely
/// (`br-connection-page-timeout`), at which point only opening the case revives
/// it. Three seconds leaves enough time for the teardown without losing the
/// accessory. The earlier 15 s value was actively harmful: it produced exactly
/// that page timeout during testing.
pub const RECOVERY_GAP: Duration = Duration::from_secs(3);

/// What the accessory is doing, from the caller's point of view.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum StreamState {
    /// Nothing negotiated yet.
    #[default]
    Idle,
    /// Services were announced; these are their names.
    Announced(Vec<String>),
    /// SetReport went out and samples are expected.
    Streaming { device: u8, service: String },
    /// No announcements; the accessory needs a link reset.
    NeedsRecovery,
}

/// Tracks one negotiation attempt and decides what to do next.
#[derive(Debug, Default)]
pub struct StreamSupervisor {
    state: StreamState,
    attempts: u32,
    last_attempt: Option<Instant>,
    /// Set once the interval has been written for the current set of devices.
    started_for: Option<u8>,
}

impl StreamSupervisor {
    pub fn new() -> Self {
        Self {
            state: StreamState::Idle,
            attempts: 0,
            last_attempt: None,
            started_for: None,
        }
    }

    pub fn state(&self) -> &StreamState {
        &self.state
    }

    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    /// Records that a negotiation has begun.
    pub fn begin_attempt(&mut self) {
        self.attempts += 1;
        self.last_attempt = Some(Instant::now());
        self.state = StreamState::Idle;
        self.started_for = None;
        info!("RemoteHID negotiation attempt {}", self.attempts);
    }

    /// Records the services the accessory announced.
    ///
    /// Returns the device to start if the interval still has to be written, which
    /// is the case on the first announcement and again after a re-announcement
    /// (the accessory cancels the stream when it re-announces on a bud switch).
    pub fn record_announcement(
        &mut self,
        registry: &DeviceRegistry,
        generation: &mut remotehid::Generation,
    ) -> Option<Vec<u8>> {
        let services: Vec<String> = registry.all().map(|d| d.service.clone()).collect();
        if services.is_empty() {
            return None;
        }
        self.state = StreamState::Announced(services);

        let motion = registry.motion_device()?;
        if self.started_for == Some(motion.device) {
            debug!(
                "Already started on device {}; re-announcement needs no restart",
                motion.device
            );
            return Some(Vec::new());
        }

        self.started_for = Some(motion.device);
        self.state = StreamState::Streaming {
            device: motion.device,
            service: motion.service.clone(),
        };
        info!(
            "Starting {:.0} Hz on {} (device {})",
            1_000_000.0 / DEFAULT_INTERVAL_US as f64,
            motion.service,
            motion.device
        );
        Some(remotehid::make_set_interval(
            generation.next(),
            motion.device,
            DEFAULT_INTERVAL_US,
        ))
    }

    /// Decides what a caller should do after an attempt produced no announcement.
    ///
    /// `in_ear` matters: a unit sitting in its case legitimately powers the
    /// sensors down, so cycling the link there would be the wrong reaction and is
    /// reported as [`StreamState::Idle`] instead.
    pub fn after_silence(&mut self, in_ear: bool) -> Recovery {
        self.state = StreamState::Idle;
        if !in_ear {
            debug!("No announcement while out of ear; the sensors are powered down");
            return Recovery::WaitForWearer;
        }
        // A worn accessory that says nothing is usually still coming up, not
        // broken. Every extra link cycle makes the unit harder to revive - in
        // testing, aggressive reconnecting drove it to `page-timeout`, where only
        // opening the case helped. So wait on the existing link first and only
        // cycle once that has clearly failed.
        if self.attempts < SILENT_ATTEMPTS_BEFORE_CYCLE {
            debug!(
                "Silent accessory on attempt {}; waiting before touching the link",
                self.attempts
            );
            return Recovery::KeepWaiting;
        }
        warn!("Accessory still silent after {} attempts; cycling the link", self.attempts);
        self.state = StreamState::NeedsRecovery;
        Recovery::CycleLink
    }

    /// Decides what to do when an attempt announced some services but not the
    /// motion one.
    ///
    /// Cutting the link here is counter-productive: the accessory announces its
    /// services incrementally while it finishes coming up, so reconnecting resets
    /// that progress. Observed during testing - an early reconnect produced
    /// 0, 1, 2, 3 services on successive attempts, while leaving it alone reached
    /// all eight.
    pub fn after_partial(&mut self, announced: usize) -> Recovery {
        if announced >= FULL_SERVICE_COUNT {
            return Recovery::KeepWaiting;
        }
        debug!(
            "{} of {} services so far; the accessory is still initialising",
            announced, FULL_SERVICE_COUNT
        );
        Recovery::KeepWaiting
    }

    /// Whether the announce window has elapsed since the attempt began.
    pub fn announcement_window_closed(&self) -> bool {
        self.last_attempt
            .map(|t| t.elapsed() >= ANNOUNCE_WINDOW)
            .unwrap_or(false)
    }
}

/// What to do when an attempt produced nothing (or not enough).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovery {
    /// Drop and re-establish the Bluetooth link, then try again.
    CycleLink,
    /// The buds are not in ear; there is nothing to recover, just keep waiting.
    WaitForWearer,
    /// Some services arrived but not the motion one. The accessory is still
    /// working through its list, and tearing the link down here would restart it
    /// from the beginning - so keep the connection and wait.
    KeepWaiting,
}

/// Number of services a fully initialised AirPods 3 announces.
///
/// Verified on hardware: `cma`, `devmotion6`, `iedrelay`, `gyro`, `accel`,
/// `neorelay`, `mando`, `kadabra`. Fewer means the accessory is still coming up.
pub const FULL_SERVICE_COUNT: usize = 8;

/// How many silent attempts to tolerate before touching the Bluetooth link.
///
/// Cycling the link is the only thing that clears a genuinely stuck unit, but it
/// also risks pushing one that is merely slow into `page-timeout`, where only a
/// physical case cycle helps. Waiting through the first attempt is the safer
/// trade.
pub const SILENT_ATTEMPTS_BEFORE_CYCLE: u32 = 2;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::aacp::remotehid::Kind;

    fn plist(service: &str) -> String {
        format!("<key>AccessoryService</key><string>{}</string>", service)
    }

    fn registry_with(services: &[&str]) -> (DeviceRegistry, remotehid::Generation) {
        let mut reg = DeviceRegistry::new();
        for (i, name) in services.iter().enumerate() {
            reg.record(i as u8 + 1, &plist(name), &[0x85, 0x01, 0x82, 0x22, 0x01]);
        }
        (reg, remotehid::Generation::default())
    }

    #[test]
    fn first_announcement_starts_the_motion_device() {
        let (reg, mut generation) = registry_with(&["neorelay", "devmotion6"]);
        let mut sup = StreamSupervisor::new();
        sup.begin_attempt();
        let packet = sup.record_announcement(&reg, &mut generation).expect("should start");
        assert!(!packet.is_empty(), "a SetReport has to go out");
        assert!(matches!(
            sup.state(),
            StreamState::Streaming { service, .. } if service == "devmotion6"
        ));
    }

    #[test]
    fn repeated_announcement_with_same_device_does_not_resend() {
        let (reg, mut generation) = registry_with(&["devmotion6"]);
        let mut sup = StreamSupervisor::new();
        sup.begin_attempt();
        sup.record_announcement(&reg, &mut generation);
        // The accessory re-announced the same device: no restart needed.
        let again = sup.record_announcement(&reg, &mut generation).expect("some result");
        assert!(again.is_empty(), "should not rebuild the SetReport");
    }

    #[test]
    fn reannouncement_after_new_attempt_restarts() {
        let (reg, mut generation) = registry_with(&["devmotion6"]);
        let mut sup = StreamSupervisor::new();
        sup.begin_attempt();
        let first = sup.record_announcement(&reg, &mut generation).unwrap();
        assert!(!first.is_empty());

        // A fresh attempt (new connection) must write the interval again.
        sup.begin_attempt();
        let second = sup.record_announcement(&reg, &mut generation).unwrap();
        assert!(!second.is_empty(), "a new connection needs a new SetReport");
    }

    #[test]
    fn falls_back_to_neorelay_when_devmotion6_is_absent() {
        let (reg, mut generation) = registry_with(&["neorelay", "mando"]);
        let mut sup = StreamSupervisor::new();
        sup.begin_attempt();
        sup.record_announcement(&reg, &mut generation);
        assert!(matches!(
            sup.state(),
            StreamState::Streaming { service, .. } if service == "neorelay"
        ));
    }

    #[test]
    fn empty_registry_produces_no_start() {
        let reg = DeviceRegistry::new();
        let mut generation = remotehid::Generation::default();
        let mut sup = StreamSupervisor::new();
        sup.begin_attempt();
        assert!(sup.record_announcement(&reg, &mut generation).is_none());
        assert_eq!(*sup.state(), StreamState::Idle);
    }

    #[test]
    fn partial_announcements_are_waited_out_not_reset() {
        let mut sup = StreamSupervisor::new();
        sup.begin_attempt();
        // Reconnecting on a partial list restarts the accessory's own
        // initialisation, so the answer is always to keep waiting.
        for n in [1usize, 2, 3, 7, 8] {
            assert_eq!(
                sup.after_partial(n),
                Recovery::KeepWaiting,
                "{} services should not trigger a reset",
                n
            );
        }
    }

    #[test]
    fn full_service_count_matches_hardware() {
        // The list observed from a fully initialised AirPods 3.
        assert_eq!(FULL_SERVICE_COUNT, 8);
    }

    #[test]
    fn worn_accessory_waits_before_touching_the_link() {
        let mut sup = StreamSupervisor::new();
        sup.begin_attempt();
        // First silent attempt: wait, do not reconnect.
        assert_eq!(sup.after_silence(true), Recovery::KeepWaiting);

        sup.begin_attempt();
        // Second silent attempt: now cycling is justified.
        assert_eq!(sup.after_silence(true), Recovery::CycleLink);
        assert_eq!(*sup.state(), StreamState::NeedsRecovery);
    }

    #[test]
    fn unworn_accessory_just_waits() {
        let mut sup = StreamSupervisor::new();
        sup.begin_attempt();
        assert_eq!(sup.after_silence(false), Recovery::WaitForWearer);
        assert_eq!(*sup.state(), StreamState::Idle);
    }

    #[test]
    fn attempts_are_counted() {
        let mut sup = StreamSupervisor::new();
        assert_eq!(sup.attempts(), 0);
        sup.begin_attempt();
        sup.begin_attempt();
        assert_eq!(sup.attempts(), 2);
    }

    #[test]
    fn announce_window_closes_after_the_deadline() {
        let mut sup = StreamSupervisor::new();
        assert!(!sup.announcement_window_closed(), "no attempt yet");
        sup.begin_attempt();
        assert!(!sup.announcement_window_closed(), "window just opened");
        // Wind the timestamp back to simulate the deadline passing.
        sup.last_attempt = Some(Instant::now() - ANNOUNCE_WINDOW - Duration::from_secs(1));
        assert!(sup.announcement_window_closed());
    }

    #[test]
    fn recovery_gap_stays_inside_the_working_window() {
        // Below ~2 s the link teardown is incomplete; above ~10 s the accessory
        // stops answering paging and only a case cycle recovers it. Measured on
        // AirPods 3, so the constant has to sit between those bounds.
        assert!(
            RECOVERY_GAP >= Duration::from_secs(2),
            "too short: the link has not finished tearing down"
        );
        assert!(
            RECOVERY_GAP <= Duration::from_secs(10),
            "too long: the accessory stops answering paging"
        );
    }

    #[test]
    fn registry_records_report_kinds_used_by_the_supervisor() {
        // Guard against the registry silently losing its notion of which service
        // streams: the supervisor picks the target purely by name.
        let (reg, _) = registry_with(&["cma", "devmotion6", "gyro"]);
        let motion = reg.motion_device().expect("devmotion6 should be chosen");
        assert_eq!(motion.service, "devmotion6");
        assert_eq!(Kind::InputReport.bits(), 2);
    }
}
