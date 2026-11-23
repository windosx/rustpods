//! Exercises the real `HrtfClient` against a live PipeWire, without Bluetooth.
//!
//! The probe proves that *a* write reaches the sofa plugin. This proves that the
//! code RustPods actually runs - the same `HrtfClient` the daemon constructs -
//! binds the node and moves it. Nothing here touches the accessory, so it is safe
//! to run while another RustPods instance owns the AACP session.
//!
//! Build: BINDGEN_EXTRA_CLANG_ARGS="-isystem /usr/lib/clang/22/include" \
//!        cargo run --example verify_hrtf_client --features audio-follow

// The crate is binary-only, so the module is included by path rather than
// linked as a library. It compiles the same source the daemon runs.
#[cfg(all(target_os = "linux", feature = "audio-follow"))]
#[path = "../src/system/audio_follow.rs"]
#[allow(dead_code, unused_imports)] // only the client half is exercised here
mod audio_follow;

#[cfg(all(target_os = "linux", feature = "audio-follow"))]
mod imp {
    use pipewire as pw;
    use pw::spa::param::ParamType;
    use pw::spa::pod::{Pod, Value};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use super::audio_follow::{FilterPlan, HrtfClient, StageAngles, StageSchedule};

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let plan = FilterPlan::default();
        let mut client = HrtfClient::start(&plan)?;
        println!("client started, waiting for '{}'", plan.node_name);

        // The bind happens on the PipeWire thread, so poll rather than assume.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !client.is_running() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        if !client.is_running() {
            println!("FAILED: node was never bound");
            return Ok(());
        }
        println!("bound OK");

        // 1. Test StageSchedule + 3D stereo stage writes (Azimuth + Elevation per ear)
        let mut stage_sched = StageSchedule::new();
        let start = Instant::now();
        let mut stages_sent = 0;
        for i in 0..200u32 {
            let t = i as f32 / 200.0;
            let yaw = (t * std::f32::consts::PI).sin() * 45.0;
            let pitch = (t * 2.0 * std::f32::consts::PI).sin() * 15.0;
            let roll = (t * std::f32::consts::PI).cos() * 10.0;
            let now_ms = start.elapsed().as_millis() as u64;

            if stage_sched.should_update([yaw, pitch, roll], now_ms) {
                let stage = stage_sched.commit([yaw, pitch, roll], now_ms);
                client.set_stage(stage);
                stages_sent += 1;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        println!("sent {} stage updates through StageSchedule", stages_sent);

        // Allow data loop cross-fade to settle
        std::thread::sleep(Duration::from_millis(100));

        // Read stage back from node Props
        let stage_readback = read_stage(&plan.node_name)?;
        let stage_ok = match stage_readback {
            Some(stage) => {
                println!(
                    "stage readback: L({:.1}, {:.1}) R({:.1}, {:.1})",
                    stage.left_azimuth,
                    stage.left_elevation,
                    stage.right_azimuth,
                    stage.right_elevation
                );
                // Last commit had non-zero angles
                stage.left_azimuth.abs() > 0.1 || stage.right_azimuth.abs() > 0.1
            }
            None => {
                println!("FAILED: could not read stage controls back from node Props");
                false
            }
        };

        // 2. Test shared Azimuth write (the simpler mode)
        let test_azimuth = 37.5f32;
        println!("testing shared Azimuth write: {:.1} deg", test_azimuth);
        client.set_azimuth(test_azimuth);
        std::thread::sleep(Duration::from_millis(100));

        let az_readback = read_azimuth(&plan.node_name)?;
        let az_ok = match az_readback {
            Some(az) => {
                println!("azimuth readback: {:.1}", az);
                (az - test_azimuth).abs() < 1.0
            }
            None => {
                println!("FAILED: could not read azimuth back from node Props");
                false
            }
        };

        println!();
        println!(
            "STAGE VERIFICATION: {}",
            if stage_ok {
                "3D stereo stage controls verified"
            } else {
                "stage controls did not reach the node"
            }
        );
        println!(
            "AZIMUTH VERIFICATION: {}",
            if az_ok {
                "shared azimuth write verified"
            } else {
                "azimuth write did not reach the node"
            }
        );

        // Reset to neutral stereo stage before shutting down
        client.set_stage(StageAngles::CENTRE);
        std::thread::sleep(Duration::from_millis(100));

        client.shutdown();
        if !stage_ok || !az_ok {
            return Err("verification failed".into());
        }
        Ok(())
    }

    /// Reads all 4 stage controls back from the node's Props.
    fn read_stage(node_name: &str) -> Result<Option<StageAngles>, Box<dyn std::error::Error>> {
        pw::init();
        let mainloop = pw::main_loop::MainLoopRc::new(None)?;
        let context = pw::context::ContextRc::new(&mainloop, None)?;
        let core = context.connect_rc(None)?;
        let registry = core.get_registry_rc()?;

        let node_slot: Rc<RefCell<Option<pw::node::Node>>> = Rc::new(RefCell::new(None));
        let slot_cb = Rc::clone(&node_slot);
        let registry_cb = registry.clone();
        let expected = node_name.to_string();
        let _reg = registry
            .add_listener_local()
            .global(move |global| {
                if global.type_ != pw::types::ObjectType::Node {
                    return;
                }
                let Some(props) = global.props else { return };
                if props.get("node.name") != Some(expected.as_str()) {
                    return;
                }
                if let Ok(node) = registry_cb.bind::<pw::node::Node, _>(global) {
                    *slot_cb.borrow_mut() = Some(node);
                }
            })
            .register();
        sync(&core, &mainloop)?;

        let Some(node) = node_slot.borrow_mut().take() else {
            return Ok(None);
        };

        let found: Rc<RefCell<Option<StageAngles>>> = Rc::new(RefCell::new(None));
        let found_cb = Rc::clone(&found);
        let listener = node.add_listener_local();
        let _params = listener
            .param(move |_seq, id, _index, _next, param| {
                if id != ParamType::Props {
                    return;
                }
                let Some(pod) = param else { return };
                if let Some(v) = stage_in(pod) {
                    *found_cb.borrow_mut() = Some(v);
                }
            })
            .register();
        node.enum_params(1, Some(ParamType::Props), 0, u32::MAX);
        sync(&core, &mainloop)?;
        Ok(*found.borrow())
    }

    /// Pulls the 4 stage controls out of a Props pod.
    fn stage_in(pod: &Pod) -> Option<StageAngles> {
        let (_, value) = pw::spa::pod::deserialize::PodDeserializer::deserialize_any_from(
            pod.as_bytes(),
        )
        .ok()?;
        let Value::Object(obj) = value else { return None };
        let mut left_az = None;
        let mut left_el = None;
        let mut right_az = None;
        let mut right_el = None;
        for prop in &obj.properties {
            let Value::Struct(items) = &prop.value else { continue };
            let mut i = 0;
            while i + 1 < items.len() {
                if let (Value::String(k), Value::Float(v)) = (&items[i], &items[i + 1]) {
                    match k.as_str() {
                        "spatL:Azimuth" => left_az = Some(*v),
                        "spatL:Elevation" => left_el = Some(*v),
                        "spatR:Azimuth" => right_az = Some(*v),
                        "spatR:Elevation" => right_el = Some(*v),
                        _ => {}
                    }
                }
                i += 2;
            }
        }
        match (left_az, left_el, right_az, right_el) {
            (Some(la), Some(le), Some(ra), Some(re)) => Some(StageAngles {
                left_azimuth: la,
                left_elevation: le,
                right_azimuth: ra,
                right_elevation: re,
            }),
            _ => None,
        }
    }

    /// Reads `spatL:Azimuth` (left spatialiser) back from the node's Props.
    fn read_azimuth(node_name: &str) -> Result<Option<f32>, Box<dyn std::error::Error>> {
        pw::init();
        let mainloop = pw::main_loop::MainLoopRc::new(None)?;
        let context = pw::context::ContextRc::new(&mainloop, None)?;
        let core = context.connect_rc(None)?;
        let registry = core.get_registry_rc()?;

        let node_slot: Rc<RefCell<Option<pw::node::Node>>> = Rc::new(RefCell::new(None));
        let slot_cb = Rc::clone(&node_slot);
        let registry_cb = registry.clone();
        let wanted = node_name.to_string();
        let _reg = registry
            .add_listener_local()
            .global(move |global| {
                if global.type_ != pw::types::ObjectType::Node {
                    return;
                }
                let Some(props) = global.props else { return };
                if props.get("node.name") != Some(wanted.as_str()) {
                    return;
                }
                let mut slot = slot_cb.borrow_mut();
                if slot.is_some() {
                    return;
                }
                if let Ok(node) = registry_cb.bind::<pw::node::Node, _>(global) {
                    *slot = Some(node);
                }
            })
            .register();

        sync(&core, &mainloop)?;
        let node = match node_slot.borrow_mut().take() {
            Some(n) => n,
            None => return Ok(None),
        };

        let found: Rc<RefCell<Option<f32>>> = Rc::new(RefCell::new(None));
        let found_cb = Rc::clone(&found);
        let listener = node.add_listener_local();
        let _params = listener
            .param(move |_seq, id, _index, _next, param| {
                if id != ParamType::Props {
                    return;
                }
                let Some(pod) = param else { return };
                if let Some(v) = azimuth_in(pod) {
                    *found_cb.borrow_mut() = Some(v);
                }
            })
            .register();
        node.enum_params(1, Some(ParamType::Props), 0, u32::MAX);
        sync(&core, &mainloop)?;
        Ok(*found.borrow())
    }

    /// Pulls `spatL:Azimuth` out of a Props pod.
    fn azimuth_in(pod: &Pod) -> Option<f32> {
        let (_, value) = pw::spa::pod::deserialize::PodDeserializer::deserialize_any_from(
            pod.as_bytes(),
        )
        .ok()?;
        let Value::Object(obj) = value else { return None };
        for prop in &obj.properties {
            let Value::Struct(items) = &prop.value else { continue };
            let mut i = 0;
            while i + 1 < items.len() {
                if let (Value::String(k), Value::Float(v)) = (&items[i], &items[i + 1])
                    && k == "spatL:Azimuth" {
                        return Some(*v);
                    }
                i += 2;
            }
        }
        None
    }

    fn sync(
        core: &pw::core::CoreRc,
        mainloop: &pw::main_loop::MainLoopRc,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let pending = core.sync(0)?;
        let ml = mainloop.clone();
        let done = Rc::new(RefCell::new(false));
        let done_cb = Rc::clone(&done);
        let _listener = core
            .add_listener_local()
            .done(move |id, seq| {
                if id == pw::core::PW_ID_CORE && seq == pending {
                    *done_cb.borrow_mut() = true;
                    ml.quit();
                }
            })
            .register();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !*done.borrow() && Instant::now() < deadline {
            mainloop.run();
            if *done.borrow() {
                break;
            }
        }
        Ok(())
    }
}

#[cfg(all(target_os = "linux", feature = "audio-follow"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    imp::run()
}

#[cfg(not(all(target_os = "linux", feature = "audio-follow")))]
fn main() {
    eprintln!("verify_hrtf_client is only supported on Linux with audio-follow");
}
