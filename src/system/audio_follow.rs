//! Head-tracked audio: pans the stereo image to follow the wearer's head.
//!
//! ## What this does and does not reproduce
//!
//! macOS spatial audio renders stereo into binaural audio with an HRTF: each
//! source is placed at a fixed point in space and the mix changes as the head
//! moves. Reproducing that needs a SOFA dataset and PipeWire's `sofa` filter,
//! which is not packaged on Fedora.
//!
//! What is implemented here is the part that works without any extra plugin: the
//! two output channels are re-mixed so the image follows the head. Turning left
//! raises the left channel and lowers the right, which is the audible cue a
//! listener expects when they turn away from a fixed source. It is a plain
//! amplitude pan, so it will not convince anyone that the stage is in the room
//! the way HRTF does - but it responds correctly and immediately.
//!
//! ## Why the graph is built in Rust rather than shipped as a config file
//!
//! PipeWire can run a filter chain from a config file, and a config-driven chain
//! was the alternative. It was rejected because the gains have to change at the
//! sample rate of the sensor (100 Hz); rewriting a config file and reloading the
//! node would stall the audio and land far outside that budget. Holding the node
//! and updating its control ports directly keeps the update under a millisecond.
//!
//! ## Packaging cost, stated plainly
//!
//! Linking libpipewire means the build needs `libpipewire-0.3` headers and,
//! because `pipewire-sys` runs bindgen, `libclang`. That is a real cost for
//! packagers and it is why this file is the only place in the project that
//! touches PipeWire: everything else still builds against GTK alone, and this
//! module is behind a feature flag so a package can be built without it.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;


/// Maximum gain boost applied to the near channel, in millibels.
///
/// 600 mB is about +6 dB, which puts the near channel clearly forward without
/// clipping on typical material.
const MAX_BOOST_MB: i32 = 600;
/// Maximum cut applied to the far channel.
const MAX_CUT_MB: i32 = -600;
/// Head yaw, in degrees, that reaches full pan.
const FULL_PAN_DEG: f32 = 45.0;

/// How far the HRTF azimuth may drift from where it was last written before it
/// is pushed to PipeWire, in degrees.
///
/// Every write costs a full convolver rebuild: `spatializer_reload()` resolves
/// two HRIRs with `mysofa_getfilter_float` and allocates two new convolvers on
/// the main loop, then cross-fades them over one quantum on the data loop. That
/// is milliseconds of work, not microseconds, so writing on every sensor sample
/// (100 Hz) would saturate a core and make the cross-fade never complete.
///
/// Five degrees is below the ~3.6 degree minimum audible angle for front-ward
/// sources, so the steps stay inaudible while bounding the rebuild rate: a slow
/// head turn produces single-digit updates per second instead of a hundred.
pub const AZIMUTH_STEP_DEG: f32 = 5.0;

/// Smallest interval between HRTF updates, as a hard floor on the rebuild rate.
///
/// The step threshold alone has no upper bound on rate: shaking the head sweeps
/// through many 5-degree bins per second. 50 ms caps the rebuild cost at 20
/// per second regardless of how the wearer moves.
pub const MIN_UPDATE_INTERVAL_MS: u64 = 50;

/// Azimuth written when the head is untracked, in degrees.
///
/// Zero is straight ahead, which puts the mix back where it started.
const AZIMUTH_CENTRE_DEG: f32 = 0.0;

/// The direction convention, measured rather than assumed.
///
/// Replicating `plugin_sofa.c`'s exact call order against the bundled MIT KEMAR
/// dataset (`mysofa_s2c` on `[azimuth, elevation, radius]`, then
/// `mysofa_getfilter_float`) gives:
///
/// | azimuth | cartesian (x, y, z) | louder ear |
/// |---------|---------------------|------------|
/// | 90      | ( 0.0,  1.5, 0.0)   | left       |
/// | 270     | ( 0.0, -1.5, 0.0)   | right      |
/// | 30      | ( 1.3,  0.75, 0.0)  | left       |
/// | 330     | ( 1.3, -0.75, 0.0)  | right      |
///
/// So azimuth increases anti-clockwise seen from above: 0 is in front, 90 is
/// left, 270 is right. The dataset is symmetric at 0 and 180 (equal energy in
/// both ears), which is why only the off-axis rows prove the sign.
///
/// ## Why the sign is negated
///
/// Spatial audio keeps a source *固定的位置*: turning the head left must move the
/// perceived source to the right of the head, which is what makes the stage feel
/// like it stays put. [`HeadPose::yaw`] uses the opposite convention (positive is
/// a turn to the right, see the compass), so the azimuth is `-yaw`.
pub fn azimuth_for_yaw(yaw_deg: f32) -> f32 {
    if !yaw_deg.is_finite() {
        return AZIMUTH_CENTRE_DEG;
    }
    // Keep the result in [0, 360) without the drift that repeated `+ 360.0`
    // accumulates on a value that is already in range.
    let az = -yaw_deg;
    az.rem_euclid(360.0)
}

/// Decides when the HRTF needs retuning, so the rebuild rate stays bounded.
///
/// Kept separate from the PipeWire calls so the policy can be tested without a
/// server, and so the sensor thread only has to call [`should_update`].
///
/// [`should_update`]: AzimuthSchedule::should_update
#[derive(Debug)]
pub struct AzimuthSchedule {
    /// The azimuth most recently handed to PipeWire, in degrees.
    written: f32,
    /// When that happened, as a count of elapsed milliseconds.
    last_write_ms: Option<u64>,
    /// Set by [`reset`], so the next update ignores the step and interval tests.
    forced: bool,
}

impl Default for AzimuthSchedule {
    fn default() -> Self {
        Self::new()
    }
}

impl AzimuthSchedule {
    pub fn new() -> Self {
        Self {
            written: AZIMUTH_CENTRE_DEG,
            last_write_ms: None,
            forced: false,
        }
    }

    /// The azimuth currently in effect.
    pub fn written(&self) -> f32 {
        self.written
    }

    /// Whether a new yaw is far enough from the last write, and late enough, to
    /// justify another convolver rebuild.
    ///
    /// `now_ms` is elapsed milliseconds from an arbitrary origin; the struct only
    /// ever compares differences, so it holds no clock of its own.
    pub fn should_update(&self, yaw_deg: f32, now_ms: u64) -> bool {
        if !yaw_deg.is_finite() {
            return false;
        }
        if self.forced {
            return true;
        }
        let target = azimuth_for_yaw(yaw_deg);
        if angle_between(target, self.written) < AZIMUTH_STEP_DEG {
            return false;
        }
        match self.last_write_ms {
            None => true,
            Some(last) => now_ms.saturating_sub(last) >= MIN_UPDATE_INTERVAL_MS,
        }
    }

    /// Records that `yaw_deg` was written, and returns the azimuth to send.
    pub fn commit(&mut self, yaw_deg: f32, now_ms: u64) -> f32 {
        let target = azimuth_for_yaw(yaw_deg);
        self.written = target;
        self.last_write_ms = Some(now_ms);
        self.forced = false;
        target
    }

    /// Makes the next [`should_update`] return true, whatever the position.
    ///
    /// Used when tracking stops: the mix has to return to centre, and that is
    /// exactly the case the step test would suppress, since the last written
    /// azimuth may already be within one step of it.
    ///
    /// [`should_update`]: AzimuthSchedule::should_update
    pub fn reset(&mut self) {
        self.written = AZIMUTH_CENTRE_DEG;
        self.last_write_ms = None;
        self.forced = true;
    }
}

/// Signed angular distance between two azimuths, in degrees, in `[0, 180]`.
///
/// A plain subtraction would report 359 degrees of movement for a one-degree
/// wobble across the wrap point, which would retune on every sample.
fn angle_between(a: f32, b: f32) -> f32 {
    let diff = (a - b).rem_euclid(360.0);
    if diff > 180.0 {
        360.0 - diff
    } else {
        diff
    }
}

/// Shared, lock-free pan value in millibels.
///
/// The audio callback and the sensor loop run on different threads and must not
/// block each other, so the value is a pair of atomics rather than a mutex.
#[derive(Debug)]
pub struct PanState {
    left_mb: AtomicI32,
    right_mb: AtomicI32,
    /// Set when the wearer is not being tracked, so the mix returns to centre.
    centered: AtomicBool,
}

impl PanState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            left_mb: AtomicI32::new(0),
            right_mb: AtomicI32::new(0),
            centered: AtomicBool::new(true),
        })
    }

    /// Applies a head yaw, in degrees. Positive turns right.
    ///
    /// The mapping is linear in yaw up to [`FULL_PAN_DEG`] and clamped beyond it,
    /// so a small turn produces a small shift and a full turn is bounded rather
    /// than pulling one channel to silence.
    pub fn set_yaw(&self, yaw_deg: f32) {
        if !yaw_deg.is_finite() {
            return;
        }
        let norm = (yaw_deg / FULL_PAN_DEG).clamp(-1.0, 1.0);
        // Turning right makes the right channel the near one.
        let left = (-norm * MAX_BOOST_MB as f32) as i32;
        let right = (norm * MAX_BOOST_MB as f32) as i32;
        self.left_mb.store(left.clamp(MAX_CUT_MB, MAX_BOOST_MB), Ordering::Relaxed);
        self.right_mb
            .store(right.clamp(MAX_CUT_MB, MAX_BOOST_MB), Ordering::Relaxed);
        self.centered.store(false, Ordering::Relaxed);
    }

    /// Returns the mix to centre, for when tracking stops.
    pub fn centre(&self) {
        self.left_mb.store(0, Ordering::Relaxed);
        self.right_mb.store(0, Ordering::Relaxed);
        self.centered.store(true, Ordering::Relaxed);
    }

    pub fn gains_mb(&self) -> (i32, i32) {
        (
            self.left_mb.load(Ordering::Relaxed),
            self.right_mb.load(Ordering::Relaxed),
        )
    }

    pub fn is_centered(&self) -> bool {
        self.centered.load(Ordering::Relaxed)
    }
}

/// Description of the filter graph this module builds.
///
/// Kept separate from the PipeWire calls so the shape can be reasoned about -
/// and printed in logs - without a running server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterPlan {
    /// Node name, so `wpctl` and friends show something recognisable.
    pub node_name: String,
    /// Human-readable description shown in mixer UIs.
    pub description: String,
}

/// The filter-chain capture node, whose Props carry the sofa controls.
///
/// The name is fixed by `99-rustpods-hrtf.conf`. Changing one without the other
/// leaves the client binding nothing, which looks like working audio that never
/// moves.
pub const CAPTURE_NODE_NAME: &str = "rustpods_hrtf_in";

impl Default for FilterPlan {
    fn default() -> Self {
        Self {
            node_name: CAPTURE_NODE_NAME.to_string(),
            description: "RustPods head-tracked stereo".to_string(),
        }
    }
}

#[cfg(feature = "audio-follow")]
pub use pipewire_link::{HrtfClient, HrtfCommand};

/// The PipeWire side: binds the filter-chain node and writes azimuth to it.
///
/// ## Why a thread and a channel
///
/// PipeWire's `MainLoop` is reference counted and not `Send`, so it cannot be
/// shared with the sensor loop. The standard arrangement from the crate's own
/// docs applies: the loop owns a thread, and a [`pw::channel`] carries commands
/// into it. The sensor loop never blocks on PipeWire; it just sends a float.
///
/// ## Why the node is bound, not created
///
/// `registry.bind` attaches a proxy to the node that the config already loaded.
/// An earlier version used `core.create_object("node", ...)`, which creates a
/// *second*, unrelated node: every write succeeded and went nowhere. The
/// difference is invisible from the API - both return a working `Node` - and
/// only showed up when the node's Props were read back with `enum_params`.
#[cfg(feature = "audio-follow")]
mod pipewire_link {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use tracing::{debug, info, warn};

    use pipewire as pw;
    use pw::spa::param::ParamType;
    use pw::spa::pod::{Pod, Property, Value};

    use super::FilterPlan;

    /// SPA_PROP_params: SPA_PROP_START_Other (0x80000) + 1.
    const SPA_PROP_PARAMS: u32 = 0x80001;

    /// Control name for the sofa spatializer's azimuth.
    ///
    /// filter-graph prefixes a control port with the node name from
    /// `filter.graph.nodes[].name`, which the shipped config sets to `spat`.
    /// Confirmed by reading the node's Props back: they contain
    /// `spat:Azimuth`, `spat:Elevation` and `spat:Radius`.
    const CONTROL_AZIMUTH: &str = "spat:Azimuth";

    /// A message for the PipeWire thread.
    #[derive(Debug, Clone, Copy)]
    pub enum HrtfCommand {
        /// Write this azimuth, in degrees.
        Azimuth(f32),
        /// Stop the loop and end the thread.
        Quit,
    }

    /// Owns the PipeWire thread and the sender into it.
    pub struct HrtfClient {
        sender: Option<pw::channel::Sender<HrtfCommand>>,
        thread: Option<JoinHandle<()>>,
        /// Set by the thread once the node is bound, read by the caller to decide
        /// whether the UI should claim audio is being tracked.
        running: Arc<AtomicBool>,
    }

    impl HrtfClient {
        /// Starts the PipeWire thread.
        ///
        /// Returns as soon as the thread is spawned; whether the node is actually
        /// present is reported through [`is_running`]. Making this synchronous
        /// would mean blocking the caller on a server round trip that may never
        /// come back, which for a GUI is a hang on startup.
        ///
        /// [`is_running`]: HrtfClient::is_running
        pub fn start(plan: &FilterPlan) -> std::io::Result<Self> {
            let (sender, receiver) = pw::channel::channel::<HrtfCommand>();
            let running = Arc::new(AtomicBool::new(false));
            let running_thread = Arc::clone(&running);
            let node_name = plan.node_name.clone();

            let thread = std::thread::Builder::new()
                .name("rustpods-hrtf".to_string())
                .spawn(move || {
                    if let Err(e) = run_loop(receiver, running_thread, &node_name) {
                        warn!("HRTF audio stage stopped: {}", e);
                    }
                })?;

            Ok(Self {
                sender: Some(sender),
                thread: Some(thread),
                running,
            })
        }

        /// Whether the filter-chain node has been found and bound.
        pub fn is_running(&self) -> bool {
            self.running.load(Ordering::Relaxed)
        }

        /// Queues an azimuth. Never blocks and never fails loudly: the sensor loop
        /// calls this at sensor rate and must not care if audio is unavailable.
        pub fn set_azimuth(&self, azimuth_deg: f32) {
            if !azimuth_deg.is_finite() {
                return;
            }
            if let Some(sender) = &self.sender {
                let _ = sender.send(HrtfCommand::Azimuth(azimuth_deg));
            }
        }

        /// Stops the loop and joins the thread.
        pub fn shutdown(&mut self) {
            if let Some(sender) = self.sender.take() {
                let _ = sender.send(HrtfCommand::Quit);
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
            self.running.store(false, Ordering::Relaxed);
        }
    }

    impl Drop for HrtfClient {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    /// Runs the PipeWire main loop on the calling thread until `Quit`.
    fn run_loop(
        receiver: pw::channel::Receiver<HrtfCommand>,
        running: Arc<AtomicBool>,
        expected_node: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        pw::init();
        let mainloop = pw::main_loop::MainLoopRc::new(None)?;
        let context = pw::context::ContextRc::new(&mainloop, None)?;
        let core = context.connect_rc(None)?;
        let registry = core.get_registry_rc()?;

        // Bind while inside the callback: `GlobalObject` borrows the event's
        // properties, so it cannot be stashed and bound later without a
        // lifetime error, and the API documents this as the place to do it.
        let node_slot: Rc<RefCell<Option<pw::node::Node>>> = Rc::new(RefCell::new(None));
        let node_slot_cb = Rc::clone(&node_slot);
        let running_cb = Arc::clone(&running);
        let registry_cb = registry.clone();
        let expected = expected_node.to_string();
        let _global_listener = registry
            .add_listener_local()
            .global(move |global| {
                if global.type_ != pw::types::ObjectType::Node {
                    return;
                }
                let Some(props) = global.props else { return };
                if props.get("node.name") != Some(expected.as_str()) {
                    return;
                }
                let mut slot = node_slot_cb.borrow_mut();
                if slot.is_some() {
                    return;
                }
                match registry_cb.bind::<pw::node::Node, _>(global) {
                    Ok(node) => {
                        info!("Bound HRTF node '{}'", expected);
                        *slot = Some(node);
                        running_cb.store(true, Ordering::Relaxed);
                    }
                    Err(e) => warn!("Could not bind HRTF node '{}': {}", expected, e),
                }
            })
            .register();

        let ml_cb = mainloop.clone();
        let running_quit = Arc::clone(&running);
        let _recv = receiver.attach(&mainloop.loop_(), move |command| match command {
            HrtfCommand::Azimuth(deg) => {
                // The node may not be bound yet; dropping the value here is fine
                // because the schedule will send another as the head moves.
                let slot = match node_slot.try_borrow() {
                    Ok(slot) => slot,
                    Err(_) => return,
                };
                let Some(node) = slot.as_ref() else { return };
                match props_pod(CONTROL_AZIMUTH, deg) {
                    Ok(bytes) => {
                        if let Some(pod) = Pod::from_bytes(&bytes) {
                            node.set_param(ParamType::Props, 0, pod);
                            debug!("Azimuth -> {:.1}", deg);
                        }
                    }
                    Err(e) => warn!("Could not serialise azimuth: {}", e),
                }
            }
            HrtfCommand::Quit => {
                running_quit.store(false, Ordering::Relaxed);
                ml_cb.quit();
            }
        });

        mainloop.run();
        Ok(())
    }

    /// Serialises `SPA_TYPE_OBJECT_Props { SPA_PROP_params: Struct(name, value) }`.
    fn props_pod(control: &str, value: f32) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        use pw::spa::pod::Object;

        let object = Object {
            type_: pw::spa::utils::SpaTypes::ObjectParamProps.as_raw(),
            id: ParamType::Props.as_raw(),
            properties: vec![Property::new(
                SPA_PROP_PARAMS,
                Value::Struct(vec![
                    Value::String(control.to_string()),
                    Value::Float(value),
                ]),
            )],
        };
        let mut buffer = Vec::new();
        pw::spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(&mut buffer),
            &Value::Object(object),
        )?;
        Ok(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centred_by_default() {
        let pan = PanState::new();
        assert_eq!(pan.gains_mb(), (0, 0));
        assert!(pan.is_centered());
    }

    #[test]
    fn turning_right_lifts_the_right_channel() {
        let pan = PanState::new();
        pan.set_yaw(30.0);
        let (left, right) = pan.gains_mb();
        assert!(right > 0, "right should be boosted, got {}", right);
        assert!(left < 0, "left should be cut, got {}", left);
        assert!(!pan.is_centered());
    }

    #[test]
    fn turning_left_lifts_the_left_channel() {
        let pan = PanState::new();
        pan.set_yaw(-30.0);
        let (left, right) = pan.gains_mb();
        assert!(left > 0, "left should be boosted, got {}", left);
        assert!(right < 0, "right should be cut, got {}", right);
    }

    #[test]
    fn pan_is_symmetric() {
        let a = PanState::new();
        a.set_yaw(25.0);
        let b = PanState::new();
        b.set_yaw(-25.0);
        let (al, ar) = a.gains_mb();
        let (bl, br) = b.gains_mb();
        assert_eq!(al, -bl, "left gains should mirror");
        assert_eq!(ar, -br, "right gains should mirror");
    }

    #[test]
    fn full_pan_is_clamped() {
        let pan = PanState::new();
        // Beyond the full-pan angle the boost must not keep growing.
        pan.set_yaw(170.0);
        let (_, right) = pan.gains_mb();
        assert_eq!(right, MAX_BOOST_MB);
        pan.set_yaw(-170.0);
        let (left, _) = pan.gains_mb();
        assert_eq!(left, MAX_BOOST_MB);
    }

    #[test]
    fn small_turns_produce_small_shifts() {
        let pan = PanState::new();
        pan.set_yaw(5.0);
        let (left, right) = pan.gains_mb();
        assert!(
            right.abs() < MAX_BOOST_MB / 2,
            "a 5 degree turn should not approach full pan, got {}",
            right
        );
        assert!(right > 0, "but it should still move, got {}", right);
        assert!(left < 0);
    }

    #[test]
    fn centring_restores_a_balanced_mix() {
        let pan = PanState::new();
        pan.set_yaw(40.0);
        pan.centre();
        assert_eq!(pan.gains_mb(), (0, 0));
        assert!(pan.is_centered());
    }

    #[test]
    fn non_finite_input_is_ignored() {
        let pan = PanState::new();
        pan.set_yaw(20.0);
        let before = pan.gains_mb();
        pan.set_yaw(f32::NAN);
        pan.set_yaw(f32::INFINITY);
        assert_eq!(pan.gains_mb(), before, "NaN must not corrupt the gains");
    }

    // --- azimuth mapping ---------------------------------------------------

    #[test]
    fn facing_forward_puts_the_source_in_front() {
        assert_eq!(azimuth_for_yaw(0.0), 0.0);
    }

    #[test]
    fn the_sign_is_the_documented_one() {
        // Measurement (see `azimuth_for_yaw`): 90 degrees puts the source on the
        // listener's left. Turning the head right (+yaw) has to move the source
        // to the head's left, so yaw and azimuth run in opposite directions.
        assert!(
            (azimuth_for_yaw(30.0) - 330.0).abs() < 1e-4,
            "turning right should send azimuth below 360, got {}",
            azimuth_for_yaw(30.0)
        );
        assert!(
            (azimuth_for_yaw(-30.0) - 30.0).abs() < 1e-4,
            "turning left should send azimuth to 30, got {}",
            azimuth_for_yaw(-30.0)
        );
    }

    #[test]
    fn azimuth_is_always_in_range() {
        for yaw in [-720.0, -360.0, -181.0, -1.0, 0.0, 1.0, 179.0, 359.0, 360.0, 720.0] {
            let az = azimuth_for_yaw(yaw);
            assert!(
                (0.0..360.0).contains(&az),
                "yaw {} produced azimuth {} outside [0, 360)",
                yaw,
                az
            );
        }
    }

    #[test]
    fn a_full_turn_is_a_full_sweep() {
        // Half a turn has to reach the opposite side, or the pan lags the head.
        let half = azimuth_for_yaw(180.0);
        assert!(
            (half - 180.0).abs() < 1e-4,
            "180 degrees of yaw should put the source behind, got {}",
            half
        );
    }

    #[test]
    fn non_finite_yaw_maps_to_centre() {
        assert_eq!(azimuth_for_yaw(f32::NAN), 0.0);
        assert_eq!(azimuth_for_yaw(f32::INFINITY), 0.0);
    }

    // --- update scheduling --------------------------------------------------

    #[test]
    fn small_head_movements_do_not_retune() {
        let sched = AzimuthSchedule::new();
        // Well inside one step: rebuilding the convolver for this would be waste.
        assert!(!sched.should_update(1.0, 10_000));
        assert!(!sched.should_update(-2.0, 10_000));
    }

    #[test]
    fn a_real_turn_does_retune() {
        let sched = AzimuthSchedule::new();
        assert!(
            sched.should_update(20.0, 10_000),
            "20 degrees is well past the step threshold"
        );
    }

    #[test]
    fn the_wrap_point_does_not_count_as_a_huge_move() {
        let mut sched = AzimuthSchedule::new();
        // Start just below the wrap point.
        sched.commit(0.5, 0);
        assert_eq!(sched.written(), 359.5);
        // A one-degree wobble across 0 must not look like 359 degrees.
        assert!(
            !sched.should_update(0.0, 10_000),
            "crossing the wrap point is a small move, not a full turn"
        );
    }

    #[test]
    fn rebuild_rate_is_capped() {
        let mut sched = AzimuthSchedule::new();
        sched.commit(60.0, 1_000);
        // Far enough to pass the step test, but too soon after the last write.
        assert!(
            !sched.should_update(0.0, 1_000 + MIN_UPDATE_INTERVAL_MS - 1),
            "must not retune inside the interval floor"
        );
        assert!(
            sched.should_update(0.0, 1_000 + MIN_UPDATE_INTERVAL_MS),
            "must retune once the interval has passed"
        );
    }

    #[test]
    fn commit_records_what_was_written() {
        let mut sched = AzimuthSchedule::new();
        let az = sched.commit(45.0, 500);
        assert_eq!(az, sched.written());
        assert!((az - 315.0).abs() < 1e-4, "got {}", az);
        // Having just written, the same position is not due again.
        assert!(!sched.should_update(45.0, 500 + MIN_UPDATE_INTERVAL_MS));
    }

    #[test]
    fn reset_forces_a_recentre() {
        let mut sched = AzimuthSchedule::new();
        sched.commit(90.0, 0);
        assert!(sched.written() != 0.0);
        sched.reset();
        assert_eq!(sched.written(), 0.0);
        // With no prior write, coming back to centre is allowed immediately.
        assert!(sched.should_update(0.0, 1));
    }

    #[test]
    fn the_step_threshold_stays_below_the_audible_angle() {
        // The minimum audible angle in front is about 3.6 degrees; a coarse step
        // keeps the rebuild rate down but must not become audible as stair-steps.
        assert!(
            AZIMUTH_STEP_DEG >= 1.0,
            "too fine: rebuilds would dominate the CPU budget"
        );
        assert!(
            AZIMUTH_STEP_DEG <= 10.0,
            "too coarse: the step itself would be audible"
        );
    }

    #[test]
    fn azimuth_step_is_independent_of_the_amplitude_pan() {
        // The amplitude pan is a fallback with its own, coarser full-scale angle;
        // the HRTF path is accurate and must not inherit it.
        assert!(FULL_PAN_DEG > AZIMUTH_STEP_DEG * 2.0);
    }
}
