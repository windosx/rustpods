//! Head-tracked audio: keeps the sound stage where it is as the wearer turns.
//!
//! ## How the stereo stage is rendered
//!
//! macOS spatial audio places the left and right channels at fixed positions in
//! front of the listener - a virtual loudspeaker pair at +/-30 degrees - and
//! rotates that stage in 3D as the head turns, nods, and tilts.
//!
//! PipeWire's `sofa` filter-chain plugin (`pipewire-module-filter-chain-sofa`
//! on Fedora) runs two independent spatializers (`spatL` and `spatR`), one per
//! input channel, each exposing `Azimuth` and `Elevation` control ports. For
//! every head pose `(yaw, pitch, roll)`, [`stage_for_pose`] transforms the two
//! fixed speaker vectors into the head's coordinate frame and [`HrtfClient`]
//! writes all four angles (`spatL:Azimuth`, `spatL:Elevation`, `spatR:Azimuth`,
//! `spatR:Elevation`) in a single `SPA_PROP_params` struct.
//!
//! The older amplitude-pan fallback and its one-axis scheduler were removed:
//! nothing called them in production, and keeping untested-against-hardware
//! fallback code around invited exactly the kind of drift that bit the
//! sign conventions. [`StageSchedule`] is the only retuning policy.
//!
//! ## Why the graph is shipped as a config file
//!
//! The filter graph is declared in `contrib/pipewire/99-rustpods-hrtf.conf`,
//! not built from Rust. What has to happen at the sensor rate is updating
//! control values, and that is a Props write on the bound capture node - never
//! a config reload, which would stall the audio far outside the budget.
//!
//! ## Packaging cost, stated plainly
//!
//! Linking libpipewire needs `libpipewire-0.3` headers and, because
//! `pipewire-sys` runs bindgen, `libclang`. That cost is why this module sits
//! behind the `audio-follow` feature and is the only place in the project that
//! touches PipeWire.

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
// Checked at compile time: below ~1.0 the rebuilds would dominate the CPU
// budget, above ~10.0 the stair-steps themselves become audible (the minimum
// audible angle in front is about 3.6 degrees).
const _: () = assert!(AZIMUTH_STEP_DEG >= 1.0 && AZIMUTH_STEP_DEG <= 10.0);

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
/// ## Why the sign matches yaw
///
/// Spatial audio keeps a source *固定的位置*: turning the head right (+yaw,
/// clockwise seen from above) turns the listener away from a fixed front source,
/// placing the source to the listener's left in the head frame. In the SOFA
/// coordinate system, azimuth is measured anti-clockwise from front (90 deg
/// is left), so a source to the left of the head has a positive azimuth (+90).
/// Therefore, turning right (+yaw) maps to positive azimuth (`azimuth = yaw`).
pub fn azimuth_for_yaw(yaw_deg: f32) -> f32 {
    if !yaw_deg.is_finite() {
        return AZIMUTH_CENTRE_DEG;
    }
    // Keep the result in [0, 360) without the drift that repeated `+ 360.0`
    // accumulates on a value that is already in range.
    let az = yaw_deg;
    az.rem_euclid(360.0)
}

/// Shortest angular distance between two angles, in degrees, in `[0, 180]`.
///
/// A plain subtraction would call a one-degree wobble across the +/-180 seam a
/// 359-degree turn and retune on every sensor sample.
fn axis_distance(a: f32, b: f32) -> f32 {
    let diff = (a - b).abs() % 360.0;
    if diff > 180.0 { 360.0 - diff } else { diff }
}

/// Rest azimuth of the virtual left speaker, in degrees (`+30`, front-left).
///
/// Standard ITU-R BS.775 stereo loudspeaker pair: +/-30 degrees from straight
/// ahead. Driving both spatializers with the same 0-degree azimuth collapses
/// the stage into a single point in front of the nose.
pub const SPEAKER_LEFT_AZIMUTH_DEG: f32 = 30.0;

/// Rest azimuth of the virtual right speaker, in degrees (`330` = `-30`, front-right).
pub const SPEAKER_RIGHT_AZIMUTH_DEG: f32 = 330.0;

/// Spherical coordinates (in degrees) for the left and right sofa spatializers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StageAngles {
    /// `spatL:Azimuth` in `[0, 360)`.
    pub left_azimuth: f32,
    /// `spatL:Elevation` in `[-90, 90]`.
    pub left_elevation: f32,
    /// `spatR:Azimuth` in `[0, 360)`.
    pub right_azimuth: f32,
    /// `spatR:Elevation` in `[-90, 90]`.
    pub right_elevation: f32,
}

impl StageAngles {
    /// Rest position of the stereo pair when the head is neutral or untracked.
    pub const CENTRE: Self = Self {
        left_azimuth: SPEAKER_LEFT_AZIMUTH_DEG,
        left_elevation: 0.0,
        right_azimuth: SPEAKER_RIGHT_AZIMUTH_DEG,
        right_elevation: 0.0,
    };
}

impl Default for StageAngles {
    fn default() -> Self {
        Self::CENTRE
    }
}

/// Computes `(azimuth, elevation)` in degrees for a room-fixed horizontal
/// speaker at `speaker_offset_deg` (anti-clockwise from front: `+30` left,
/// `-30` right) under a 3-axis head pose `(yaw_deg, pitch_deg, roll_deg)`.
///
/// ## Coordinate conventions
///
/// `plugin_sofa.c` passes `[Azimuth, Elevation, Radius]` to `mysofa_s2c`:
/// * `+X` is straight ahead (`Azimuth = 0, Elevation = 0`),
/// * `+Y` is left (`Azimuth = 90, Elevation = 0`),
/// * `+Z` is up (`Elevation = +90`).
///
/// In `HeadPose`:
/// * `yaw_deg > 0` turns the head right (clockwise from above), so a fixed
///   source rotates anti-clockwise (towards the left, `+Y`) by `+yaw_deg`
///   around `+Z` — matching [`azimuth_for_yaw`] exactly when `pitch` and
///   `roll` are zero;
/// * `pitch_deg > 0` tilts the head up, so a source in front (`+X`) moves down
///   (`-Z`, negative elevation) in the head frame;
/// * `roll_deg > 0` tilts the head toward the right shoulder, lifting the left
///   ear (`+Y`) and dropping the right ear (`-Y`), so in head coordinates a
///   left-side source moves down in elevation and a right-side source moves up.
pub fn speaker_for_pose(
    speaker_offset_deg: f32,
    yaw_deg: f32,
    pitch_deg: f32,
    roll_deg: f32,
) -> (f32, f32) {
    let yaw = if yaw_deg.is_finite() { yaw_deg } else { 0.0 };
    let pitch = if pitch_deg.is_finite() {
        pitch_deg.clamp(-90.0, 90.0)
    } else {
        0.0
    };
    let roll = if roll_deg.is_finite() {
        roll_deg.clamp(-90.0, 90.0)
    } else {
        0.0
    };

    // 1. Yaw in the horizontal plane around +Z.
    let alpha = (speaker_offset_deg + yaw).to_radians();
    let (y1, x1) = alpha.sin_cos();

    // 2. Pitch around the head's lateral axis (+Y).
    let (sin_p, cos_p) = pitch.to_radians().sin_cos();
    let x2 = x1 * cos_p;
    let y2 = y1;
    let z2 = -x1 * sin_p;

    // 3. Roll around the head's forward axis (+X).
    let (sin_r, cos_r) = roll.to_radians().sin_cos();
    let x3 = x2;
    let y3 = y2 * cos_r + z2 * sin_r;
    let z3 = -y2 * sin_r + z2 * cos_r;

    let elevation = z3.clamp(-1.0, 1.0).asin().to_degrees();
    let azimuth = if x3 * x3 + y3 * y3 < 1e-12 {
        (speaker_offset_deg + yaw).rem_euclid(360.0)
    } else {
        y3.atan2(x3).to_degrees().rem_euclid(360.0)
    };
    (azimuth, elevation)
}

/// Computes the 3D spherical coordinates of both virtual stereo speakers
/// (`+30` left, `-30` right) for a head pose `(yaw, pitch, roll)` in degrees.
pub fn stage_for_pose(yaw_deg: f32, pitch_deg: f32, roll_deg: f32) -> StageAngles {
    let (left_azimuth, left_elevation) =
        speaker_for_pose(SPEAKER_LEFT_AZIMUTH_DEG, yaw_deg, pitch_deg, roll_deg);
    let (right_azimuth, right_elevation) =
        speaker_for_pose(-SPEAKER_LEFT_AZIMUTH_DEG, yaw_deg, pitch_deg, roll_deg);
    StageAngles {
        left_azimuth,
        left_elevation,
        right_azimuth,
        right_elevation,
    }
}

/// Decides when the 3-axis stereo stage needs retuning in PipeWire.
///
/// Every update rebuilds the two sofa convolvers, so this applies the same
/// [`AZIMUTH_STEP_DEG`] deadband and [`MIN_UPDATE_INTERVAL_MS`] rate floor as
/// [`AzimuthSchedule`], but tests all three head axes (`yaw`, `pitch`, `roll`)
/// so nodding or tilting with a steady yaw still updates the stage.
#[derive(Debug)]
pub struct StageSchedule {
    pose: [f32; 3],
    written: StageAngles,
    last_write_ms: Option<u64>,
    forced: bool,
}

impl Default for StageSchedule {
    fn default() -> Self {
        Self::new()
    }
}

impl StageSchedule {
    pub fn new() -> Self {
        Self {
            pose: [0.0; 3],
            written: StageAngles::CENTRE,
            last_write_ms: None,
            // Ensure the initial +/-30 degree stereo stage is pushed on the
            // first sample even when the head starts centred.
            forced: true,
        }
    }

    /// The stage angles most recently committed.
    pub fn written(&self) -> StageAngles {
        self.written
    }

    /// Whether any axis in `pose` (`[yaw, pitch, roll]`) has moved by at least
    /// [`AZIMUTH_STEP_DEG`] and [`MIN_UPDATE_INTERVAL_MS`] has elapsed.
    pub fn should_update(&self, pose: [f32; 3], now_ms: u64) -> bool {
        if pose.iter().any(|v| !v.is_finite()) {
            return false;
        }
        if self.forced {
            return true;
        }
        let moved = pose
            .iter()
            .zip(self.pose.iter())
            .any(|(a, b)| axis_distance(*a, *b) >= AZIMUTH_STEP_DEG);
        if !moved {
            return false;
        }
        match self.last_write_ms {
            None => true,
            Some(last) => now_ms.saturating_sub(last) >= MIN_UPDATE_INTERVAL_MS,
        }
    }

    /// Records `pose` (`[yaw, pitch, roll]`) as written and returns the
    /// resulting [`StageAngles`] for both virtual speakers.
    pub fn commit(&mut self, pose: [f32; 3], now_ms: u64) -> StageAngles {
        let stage = stage_for_pose(pose[0], pose[1], pose[2]);
        self.pose = pose;
        self.written = stage;
        self.last_write_ms = Some(now_ms);
        self.forced = false;
        stage
    }

    /// Resets the schedule to the centred +/-30 degree stereo stage and forces
    /// the next update through.
    pub fn reset(&mut self) {
        self.pose = [0.0; 3];
        self.written = StageAngles::CENTRE;
        self.last_write_ms = None;
        self.forced = true;
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

#[cfg(all(target_os = "linux", feature = "audio-follow"))]
pub use pipewire_link::{HrtfClient, HrtfCommand};

/// The PipeWire side: binds the filter-chain node and writes stage angles to it.
///
/// ## Why a thread and a channel
///
/// PipeWire's `MainLoop` is reference counted and not `Send`, so it cannot be
/// shared with the sensor loop. The standard arrangement from the crate's own
/// docs applies: the loop owns a thread, and a [`pw::channel`] carries commands
/// into it. The sensor loop never blocks on PipeWire; it just sends a struct.
///
/// ## Why the node is bound, not created
///
/// `registry.bind` attaches a proxy to the node that the config already loaded.
/// An earlier version used `core.create_object("node", ...)`, which creates a
/// *second*, unrelated node: every write succeeded and went nowhere. The
/// difference is invisible from the API - both return a working `Node` - and
/// only showed up when the node's Props were read back with `enum_params`.
#[cfg(all(target_os = "linux", feature = "audio-follow"))]
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

    use super::{FilterPlan, StageAngles};

    /// SPA_PROP_params: SPA_PROP_START_Other (0x80000) + 1.
    const SPA_PROP_PARAMS: u32 = 0x80001;

    /// Control names for the two sofa spatializers' azimuth.
    const CONTROL_AZIMUTH: [&str; 2] = ["spatL:Azimuth", "spatR:Azimuth"];

    /// Control names for the full 3D stereo stage (azimuth + elevation per ear).
    const CONTROL_STAGE: [&str; 4] = [
        "spatL:Azimuth",
        "spatL:Elevation",
        "spatR:Azimuth",
        "spatR:Elevation",
    ];

    /// A message for the PipeWire thread.
    #[derive(Debug, Clone, Copy)]
    pub enum HrtfCommand {
        /// Write a shared azimuth to both spatializers, in degrees.
        Azimuth(f32),
        /// Write distinct 3D spherical coordinates to the left and right
        /// spatializers (`spatL` and `spatR`).
        Stage(StageAngles),
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
                    if let Err(e) = run_loop(receiver, running_thread, node_name) {
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

        /// Queues a shared azimuth for both spatializers.
        pub fn set_azimuth(&self, azimuth_deg: f32) {
            if !azimuth_deg.is_finite() {
                return;
            }
            if let Some(sender) = &self.sender {
                let _ = sender.send(HrtfCommand::Azimuth(azimuth_deg));
            }
        }

        /// Queues 3D spherical coordinates for the left and right virtual
        /// speakers (`spatL` and `spatR`). Never blocks.
        pub fn set_stage(&self, stage: StageAngles) {
            if ![
                stage.left_azimuth,
                stage.left_elevation,
                stage.right_azimuth,
                stage.right_elevation,
            ]
            .iter()
            .all(|v| v.is_finite())
            {
                return;
            }
            if let Some(sender) = &self.sender {
                let _ = sender.send(HrtfCommand::Stage(stage));
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
        expected_node: String,
    ) -> Result<(), Box<dyn std::error::Error>> {
        pw::init();
        let mainloop = pw::main_loop::MainLoopRc::new(None)?;
        let context = pw::context::ContextRc::new(&mainloop, None)?;
        let core = context.connect_rc(None)?;
        let registry = core.get_registry_rc()?;

        // Routing: while head tracking is live, applications must play into the
        // filter chain, or the chain sits idle and nothing is audible. WirePlumber
        // stores the default sink in the `default` metadata object under
        // `default.audio.sink` as a `{ "name": "..." }` JSON string, so writing
        // there is what the mixer UI does.
        //
        // Ordering: the HRTF node and the metadata object arrive as independent
        // registry events, in no fixed order, and a metadata property read is
        // itself asynchronous - the value only arrives as an event. So neither
        // side may act at bind time alone: whichever arrives last triggers the
        // switch, and the first observed metadata value before that switch is
        // remembered as the thing to restore on Quit.
        let default_meta: Rc<RefCell<Option<pw::metadata::Metadata>>> =
            Rc::new(RefCell::new(None));
        let current_sink: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let saved_sink: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let node_slot: Rc<RefCell<Option<pw::node::Node>>> = Rc::new(RefCell::new(None));
        let routed: Rc<std::cell::Cell<bool>> = Rc::new(std::cell::Cell::new(false));

        // Switches the default sink once, when every input is available.
        let ensure_route = {
            let meta = Rc::clone(&default_meta);
            let current = Rc::clone(&current_sink);
            let saved = Rc::clone(&saved_sink);
            let node = Rc::clone(&node_slot);
            let done = Rc::clone(&routed);
            let name = expected_node.clone();
            move || {
                if done.get() {
                    return;
                }
                let m = meta.borrow();
                let Some(m) = m.as_ref() else { return };
                if node.borrow().is_none() {
                    return;
                }
                let cur = match current.borrow().as_ref() {
                    Some(c) => c.clone(),
                    // No value observed yet: the property event has not fired.
                    None => return,
                };
                *saved.borrow_mut() = Some(cur);
                let ours = format!("{{ \"name\": \"{}\" }}", name);
                done.set(true); // before the write: the echo event must not re-enter
                m.set_property(0, "default.audio.sink", Some("Spa:Json"), Some(&ours));
                info!(
                    "Default sink -> '{}'; will restore {:?} on stop",
                    name,
                    saved.borrow()
                );
            }
        };

        // Watch the metadata's default-sink value. The first event seen before
        // we switch is the user's own choice; later events keep `current` fresh
        // so Quit can tell "still ours" from "the user moved it".
        let attach_meta_listener = {
            let current = Rc::clone(&current_sink);
            let saved = Rc::clone(&saved_sink);
            let done = Rc::clone(&routed);
            let ensure = ensure_route.clone();
            move |m: &pw::metadata::Metadata| -> pw::metadata::MetadataListener {
                let done = Rc::clone(&done);
                let saved = Rc::clone(&saved);
                let current = Rc::clone(&current);
                let ensure = ensure.clone();
                m.add_listener_local()
                    .property(move |_s, key, _ty, value| {
                        if key != Some("default.audio.sink") {
                            return 0;
                        }
                        let value = value.map(|v| v.to_string());
                        if !done.get() {
                            // Pre-switch: remember the original exactly once.
                            let mut saved = saved.borrow_mut();
                            if saved.is_none() {
                                *saved = value.clone();
                            }
                        }
                        *current.borrow_mut() = value;
                        ensure();
                        0
                    })
                    .register()
            }
        };

        // Bind while inside the callback: `GlobalObject` borrows the event's
        // properties, so it cannot be stashed and bound later without a
        // lifetime error, and the API documents this as the place to do it.
        let node_slot_cb = Rc::clone(&node_slot);
        let running_cb = Arc::clone(&running);
        let meta_slot = Rc::clone(&default_meta);
        let registry_cb = registry.clone();
        let expected_cb = expected_node.clone();
        let ensure_on_node = ensure_route.clone();
        let _global_listener = registry
            .add_listener_local()
            .global(move |global| {
                // Pick up the default metadata object when it appears; binding
                // it into the slot is all the closure does. The listener is
                // attached after `.register()` below, once the slot is filled.
                if global.type_ == pw::types::ObjectType::Metadata {
                    if let Some(props) = global.props
                        && props.get("metadata.name") == Some("default")
                            && meta_slot.borrow().is_none()
                        {
                            match registry_cb.bind::<pw::metadata::Metadata, _>(global) {
                                Ok(m) => {
                                    *meta_slot.borrow_mut() = Some(m);
                                }
                                Err(e) => warn!("Could not bind default metadata: {}", e),
                            }
                        }
                    return;
                }
                if global.type_ != pw::types::ObjectType::Node {
                    return;
                }
                let Some(props) = global.props else { return };
                if props.get("node.name") != Some(expected_cb.as_str()) {
                    return;
                }
                let mut slot = node_slot_cb.borrow_mut();
                if slot.is_some() {
                    return;
                }
                match registry_cb.bind::<pw::node::Node, _>(global) {
                    Ok(node) => {
                        info!("Bound HRTF node '{}'", expected_cb);
                        *slot = Some(node);
                        running_cb.store(true, Ordering::Relaxed);
                        ensure_on_node();
                    }
                    Err(e) => warn!("Could not bind HRTF node '{}': {}", expected_cb, e),
                }
            })
            .register();

        // The metadata listener is attached lazily from the command handler
        // below: registry globals arrive asynchronously, so a one-shot pump
        // after `.register()` races the node bind. Azimuth commands arrive
        // continuously while tracking is live, which makes them a heartbeat -
        // each one first completes pending setup work, then writes.

        let meta_listener: Rc<RefCell<Option<pw::metadata::MetadataListener>>> =
            Rc::new(RefCell::new(None));
        let attach_fn: Rc<dyn Fn(&pw::metadata::Metadata) -> pw::metadata::MetadataListener> =
            Rc::new(attach_meta_listener);
        let meta_listener_cb = Rc::clone(&meta_listener);
        let attach_fn_cb = Rc::clone(&attach_fn);
        let expected_quit = expected_node.clone();
        let ml_cb = mainloop.clone();
        let running_quit = Arc::clone(&running);
        let meta_quit = Rc::clone(&default_meta);
        let current_quit = Rc::clone(&current_sink);
        let saved_quit = Rc::clone(&saved_sink);
        let _recv = receiver.attach(mainloop.loop_(), move |command| match command {
            HrtfCommand::Azimuth(deg) => {
                // Heartbeat: complete pending setup (metadata listener + route)
                // before the write. See the note above the command handler.
                if let Some(m) = default_meta.borrow().as_ref() {
                    if meta_listener_cb.borrow().is_none() {
                        *meta_listener_cb.borrow_mut() = Some(attach_fn_cb(m));
                    }
                    ensure_route();
                }
                // The node may not be bound yet; dropping the value here is fine
                // because the schedule will send another as the head moves.
                let slot = match node_slot.try_borrow() {
                    Ok(slot) => slot,
                    Err(_) => return,
                };
                let Some(node) = slot.as_ref() else { return };
                let pairs = [(CONTROL_AZIMUTH[0], deg), (CONTROL_AZIMUTH[1], deg)];
                match props_pod(&pairs) {
                    Ok(bytes) => {
                        if let Some(pod) = Pod::from_bytes(&bytes) {
                            node.set_param(ParamType::Props, 0, pod);
                            debug!("Azimuth -> {:.1}", deg);
                        }
                    }
                    Err(e) => warn!("Could not serialise azimuth: {}", e),
                }
            }
            HrtfCommand::Stage(stage) => {
                if let Some(m) = default_meta.borrow().as_ref() {
                    if meta_listener_cb.borrow().is_none() {
                        *meta_listener_cb.borrow_mut() = Some(attach_fn_cb(m));
                    }
                    ensure_route();
                }
                let slot = match node_slot.try_borrow() {
                    Ok(slot) => slot,
                    Err(_) => return,
                };
                let Some(node) = slot.as_ref() else { return };
                let pairs = [
                    (CONTROL_STAGE[0], stage.left_azimuth),
                    (CONTROL_STAGE[1], stage.left_elevation),
                    (CONTROL_STAGE[2], stage.right_azimuth),
                    (CONTROL_STAGE[3], stage.right_elevation),
                ];
                match props_pod(&pairs) {
                    Ok(bytes) => {
                        if let Some(pod) = Pod::from_bytes(&bytes) {
                            node.set_param(ParamType::Props, 0, pod);
                            debug!(
                                "Stage -> L({:.1}, {:.1}) R({:.1}, {:.1})",
                                stage.left_azimuth,
                                stage.left_elevation,
                                stage.right_azimuth,
                                stage.right_elevation
                            );
                        }
                    }
                    Err(e) => warn!("Could not serialise stage angles: {}", e),
                }
            }
            HrtfCommand::Quit => {
                // Hand the default sink back before leaving - but only when it
                // is still ours. If the user switched sinks by hand while
                // tracking was live, their latest choice wins.
                if let Some(m) = meta_quit.borrow().as_ref() {
                    let ours = format!("{{ \"name\": \"{}\" }}", expected_quit);
                    if current_quit.borrow().as_deref() == Some(ours.as_str()) {
                        match saved_quit.borrow().as_deref() {
                            Some(json) if !json.is_empty() => {
                                m.set_property(
                                    0,
                                    "default.audio.sink",
                                    Some("Spa:Json"),
                                    Some(json),
                                );
                                info!("Restored default sink to {}", json);
                            }
                            _ => {
                                m.set_property(0, "default.audio.sink", Some("Spa:Json"), None);
                                info!("Cleared default sink override");
                            }
                        }
                    } else {
                        info!("Default sink changed elsewhere; leaving it alone");
                    }
                }
                running_quit.store(false, Ordering::Relaxed);
                ml_cb.quit();
            }
        });

        mainloop.run();
        drop(meta_listener);
        drop(attach_fn);
        Ok(())
    }

    /// Serialises `SPA_TYPE_OBJECT_Props { SPA_PROP_params: Struct(name, value, ...) }`
    /// for every `(name, value)` pair in `pairs`.
    fn props_pod(pairs: &[(&str, f32)]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        use pw::spa::pod::Object;

        let object = Object {
            type_: pw::spa::utils::SpaTypes::ObjectParamProps.as_raw(),
            id: ParamType::Props.as_raw(),
            properties: vec![Property::new(
                SPA_PROP_PARAMS,
                Value::Struct(
                    pairs
                        .iter()
                        .flat_map(|(name, value)| {
                            [Value::String((*name).to_string()), Value::Float(*value)]
                        })
                        .collect(),
                ),
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

    // --- azimuth mapping ---------------------------------------------------

    #[test]
    fn facing_forward_puts_the_source_in_front() {
        assert_eq!(azimuth_for_yaw(0.0), 0.0);
    }

    #[test]
    fn the_sign_is_the_documented_one() {
        // Measurement (see `azimuth_for_yaw`): 90 degrees puts the source on the
        // listener's left. Turning the head right (+yaw) has to move the source
        // to the head's left, so yaw and azimuth run in the same direction.
        assert!(
            (azimuth_for_yaw(30.0) - 30.0).abs() < 1e-4,
            "turning right should send azimuth to 30 (left of head), got {}",
            azimuth_for_yaw(30.0)
        );
        assert!(
            (azimuth_for_yaw(-30.0) - 330.0).abs() < 1e-4,
            "turning left should send azimuth to 330 (right of head), got {}",
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

    // --- 3D stereo stage (L/R +/-30° + yaw/pitch/roll) ---------------------

    #[test]
    fn neutral_pose_places_speakers_at_plus_and_minus_30_degrees() {
        let stage = stage_for_pose(0.0, 0.0, 0.0);
        assert!(
            (stage.left_azimuth - 30.0).abs() < 1e-4,
            "left speaker should sit at +30 deg, got {}",
            stage.left_azimuth
        );
        assert!(
            (stage.right_azimuth - 330.0).abs() < 1e-4,
            "right speaker should sit at 330 (-30) deg, got {}",
            stage.right_azimuth
        );
        assert!(stage.left_elevation.abs() < 1e-4);
        assert!(stage.right_elevation.abs() < 1e-4);
    }

    #[test]
    fn pure_yaw_matches_the_verified_azimuth_convention() {
        // With pitch and roll at zero, a 0-degree centre speaker must match
        // `azimuth_for_yaw` bit-for-bit, and the +/-30 speakers must shift by
        // the exact same +yaw offset.
        for yaw in [-90.0, -30.0, 0.0, 30.0, 90.0, 180.0] {
            let (centre_az, centre_el) = speaker_for_pose(0.0, yaw, 0.0, 0.0);
            assert!(
                angle_between(centre_az, azimuth_for_yaw(yaw)) < 1e-3,
                "yaw {}: speaker_for_pose(0) gave {}, azimuth_for_yaw gave {}",
                yaw,
                centre_az,
                azimuth_for_yaw(yaw)
            );
            assert!(centre_el.abs() < 1e-4);

            let stage = stage_for_pose(yaw, 0.0, 0.0);
            assert!(
                angle_between(stage.left_azimuth, (30.0 + yaw).rem_euclid(360.0)) < 1e-3
            );
            assert!(
                angle_between(stage.right_azimuth, (-30.0 + yaw).rem_euclid(360.0)) < 1e-3
            );
        }
    }

    #[test]
    fn looking_up_moves_the_front_stage_down_in_elevation() {
        // Positive pitch is looking up; a fixed speaker pair in front of the
        // listener must move to negative elevation in the head frame.
        let stage = stage_for_pose(0.0, 20.0, 0.0);
        assert!(
            stage.left_elevation < -15.0 && stage.right_elevation < -15.0,
            "expected negative elevation when looking up, got L={} R={}",
            stage.left_elevation,
            stage.right_elevation
        );
        assert!(
            (stage.left_elevation - stage.right_elevation).abs() < 1e-4,
            "symmetric pitch must move both speakers equally"
        );
    }

    #[test]
    fn tilting_head_right_splits_speaker_elevations_antisymmetrically() {
        // Positive roll tilts the head toward the right shoulder (left ear up,
        // right ear down), so in head coordinates the left speaker drops and
        // the right speaker rises by the same magnitude.
        let stage = stage_for_pose(0.0, 0.0, 20.0);
        assert!(
            stage.left_elevation < -5.0 && stage.right_elevation > 5.0,
            "expected L<0 and R>0 on right roll, got L={} R={}",
            stage.left_elevation,
            stage.right_elevation
        );
        assert!((stage.left_elevation + stage.right_elevation).abs() < 1e-4);
    }

    #[test]
    fn non_finite_pose_inputs_fall_back_to_neutral_axis() {
        let stage = stage_for_pose(f32::NAN, f32::INFINITY, f32::NEG_INFINITY);
        assert!((stage.left_azimuth - 30.0).abs() < 1e-4);
        assert!((stage.right_azimuth - 330.0).abs() < 1e-4);
        assert!(stage.left_elevation.abs() < 1e-4);
        assert!(stage.right_elevation.abs() < 1e-4);
    }

    // --- three-axis scheduling ----------------------------------------------

    #[test]
    fn stage_schedule_starts_forced_so_initial_stereo_pair_is_pushed() {
        let sched = StageSchedule::new();
        assert!(sched.should_update([0.0, 0.0, 0.0], 1_000));
    }

    #[test]
    fn nodding_or_tilting_alone_triggers_a_stage_update() {
        let mut sched = StageSchedule::new();
        sched.commit([0.0, 0.0, 0.0], 1_000);
        assert!(sched.should_update([0.0, 15.0, 0.0], 1_100));
        assert!(sched.should_update([0.0, 0.0, 15.0], 1_100));
    }

    #[test]
    fn sub_step_jitter_on_every_axis_is_ignored() {
        let mut sched = StageSchedule::new();
        sched.commit([10.0, 10.0, 10.0], 1_000);
        assert!(!sched.should_update([12.0, 12.0, 12.0], 1_100));
    }

    #[test]
    fn stage_schedule_respects_the_rebuild_rate_floor() {
        let mut sched = StageSchedule::new();
        sched.commit([0.0, 0.0, 0.0], 1_000);
        assert!(!sched.should_update(
            [45.0, 0.0, 0.0],
            1_000 + MIN_UPDATE_INTERVAL_MS - 1
        ));
        assert!(sched.should_update(
            [45.0, 0.0, 0.0],
            1_000 + MIN_UPDATE_INTERVAL_MS
        ));
    }

    #[test]
    fn stage_schedule_wraps_cleanly_across_plus_minus_180() {
        let mut sched = StageSchedule::new();
        sched.commit([179.0, 0.0, 0.0], 1_000);
        // 2 degrees across the +/-180 seam is inside the 5-degree step.
        assert!(!sched.should_update([-179.0, 0.0, 0.0], 1_100));
        // A 15-degree turn across the seam must trigger an update.
        assert!(sched.should_update([-166.0, 0.0, 0.0], 1_100));
    }

    #[test]
    fn axis_distance_takes_the_short_way_round() {
        assert!(axis_distance(179.8, -179.8) < 0.5);
        assert!((axis_distance(0.0, 90.0) - 90.0).abs() < 1e-4);
        assert!((axis_distance(-90.0, 90.0) - 180.0).abs() < 1e-4);
    }

    #[test]
    fn stage_reset_restores_centre_and_forces_next_write() {
        let mut sched = StageSchedule::new();
        sched.commit([40.0, 15.0, -10.0], 1_000);
        sched.reset();
        assert_eq!(sched.written(), StageAngles::CENTRE);
        assert!(sched.should_update([0.0, 0.0, 0.0], 1_010));
    }

    #[test]
    fn non_finite_pose_is_never_scheduled() {
        let sched = StageSchedule::new();
        assert!(!sched.should_update([f32::NAN, 0.0, 0.0], 1_000));
        assert!(!sched.should_update([0.0, f32::INFINITY, 0.0], 1_000));
    }
}
