//! Drives the HRTF filter chain's Azimuth at runtime, and proves it landed.
//!
//! Build: BINDGEN_EXTRA_CLANG_ARGS="-isystem /usr/lib/clang/22/include" \
//!        cargo run --example probe_hrtf --features audio-follow -- [name]
//!
//! ## Why this is a probe and not just a write
//!
//! `pw_node_set_param` is fire-and-forget: the round trip never reports whether
//! the server accepted the value, so a successful call proves nothing at all. A
//! control-name experiment confirmed that: writing a non-existent control name
//! printed exactly the same success line as a real one.
//!
//! Two things therefore have to be established separately:
//!
//! 1. Which parameter the node actually exposes - read back with `enum_params`.
//! 2. That a write shows up - `enum_params` again after the write and compare.
//!
//! The node is reached with `registry.bind`, which attaches a proxy to the
//! *existing* global. Creating a new object with `create_object` instead produces
//! a second, unrelated node that accepts writes into the void.

use pipewire as pw;
use pw::spa::param::ParamType;
use pw::spa::pod::{Pod, Value};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// The control to drive. filter-graph splices sofa control ports into the node's
/// Props under this name.
const CONTROL: &str = "spat:Azimuth";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| CONTROL.to_string());
    let target_value: f32 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(90.0);
    // The capture and playback ends are different streams, and Props written to
    // one are not necessarily delivered to the same handler as the other.
    let node_name = std::env::args()
        .nth(3)
        .unwrap_or_else(|| "rustpods_hrtf_in".to_string());

    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;
    let registry = core.get_registry_rc()?;

    // Bind inside the global callback, which is what the API documents
    // ("usually this is called in callbacks for the global event"). Storing the
    // GlobalObject to bind later runs into its borrow of the event's props.
    let node_slot: Rc<RefCell<Option<pw::node::Node>>> = Rc::new(RefCell::new(None));
    let node_slot_cb = Rc::clone(&node_slot);
    let registry_cb = registry.clone();
    let _reg = registry
        .add_listener_local()
        .global(move |global| {
            if global.type_ != pw::types::ObjectType::Node {
                return;
            }
            let Some(props) = global.props else { return };
            if props.get("node.name") != Some(node_name.as_str()) {
                return;
            }
            let mut slot = node_slot_cb.borrow_mut();
            if slot.is_some() {
                return; // first match wins; the config loads one node
            }
            match registry_cb.bind::<pw::node::Node, _>(global) {
                Ok(node) => *slot = Some(node),
                Err(e) => println!("bind failed: {}", e),
            }
        })
        .register();

    sync(&core, &mainloop)?;

    let node = match node_slot.borrow_mut().take() {
        Some(node) => node,
        None => {
            println!("rustpods_hrtf_in not found; is 99-rustpods-hrtf.conf loaded?");
            return Ok(());
        }
    };
    println!("bound rustpods_hrtf_in");

    // Read the parameters back. This is the only way to learn the real names and
    // types: guessing them produced three silent no-ops in a row.
    let seen: Rc<RefCell<Vec<(u32, String)>>> = Rc::new(RefCell::new(Vec::new()));
    let seen_cb = Rc::clone(&seen);
    let listener = node.add_listener_local();
    let _params = listener
        .param(move |_seq, id, _index, _next, param| {
            let Some(pod) = param else { return };
            let mut out = seen_cb.borrow_mut();
            out.push((id.as_raw(), describe(pod)));
        })
        .register();

    node.enum_params(1, Some(ParamType::Props), 0, u32::MAX);
    sync(&core, &mainloop)?;

    let before = seen.borrow().clone();
    if before.is_empty() {
        println!("enum_params returned no Props at all");
    } else {
        println!("--- Props before ---");
        for (id, text) in &before {
            println!("[{}] {}", id, text);
        }
    }

    // Now write, then read again. A write that lands changes the readback; a
    // write that is ignored leaves it identical.
    let pod_bytes = build_props(&name, target_value);
    let pod = Pod::from_bytes(&pod_bytes).expect("pod must be well formed");
    node.set_param(ParamType::Props, 0, pod);
    sync(&core, &mainloop)?;
    // spatializer_reload runs on the data loop, so give it a moment to publish.
    std::thread::sleep(Duration::from_millis(300));

    seen.borrow_mut().clear();
    node.enum_params(2, Some(ParamType::Props), 0, u32::MAX);
    sync(&core, &mainloop)?;
    let after = seen.borrow().clone();

    println!("--- Props after writing {} = {} ---", name, target_value);
    for (id, text) in &after {
        let changed = before
            .iter()
            .find(|(bid, btext)| *bid == *id && btext != text)
            .map(|_| "  <-- CHANGED")
            .unwrap_or("");
        println!("[{}] {}{}", id, text, changed);
    }

    let any_change = after.iter().any(|(id, text)| {
        before
            .iter()
            .any(|(bid, btext)| bid == id && btext != text)
    });
    println!(
        "verdict: {}",
        if any_change {
            "the write changed the node's state"
        } else {
            "no observable change - the value was ignored"
        }
    );

    Ok(())
}

/// Runs the loop until the core acknowledges a sync, so every pending event has
/// been delivered before the caller inspects its buffers.
fn sync(core: &pw::core::CoreRc, mainloop: &pw::main_loop::MainLoopRc) -> Result<(), Box<dyn std::error::Error>> {
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

/// Serialises `SPA_TYPE_OBJECT_Props { SPA_PROP_params: Struct(name, value) }`.
fn build_props(control_name: &str, value: f32) -> Vec<u8> {
    use pw::spa::pod::Object;
    use pw::spa::pod::Property;

    /// SPA_PROP_params = SPA_PROP_START_Other (0x80000) + 1.
    const SPA_PROP_PARAMS: u32 = 0x80001;

    let object = Object {
        type_: pw::spa::utils::SpaTypes::ObjectParamProps.as_raw(),
        id: ParamType::Props.as_raw(),
        properties: vec![Property::new(
            SPA_PROP_PARAMS,
            Value::Struct(vec![
                Value::String(control_name.to_string()),
                Value::Float(value),
            ]),
        )],
    };

    let mut buffer = Vec::new();
    pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(&mut buffer),
        &Value::Object(object),
    )
    .expect("serialising the props object must succeed");
    buffer
}

/// Renders a Props pod as `PropName=value` lines so differences are visible.
fn describe(pod: &Pod) -> String {
    let value = match pw::spa::pod::deserialize::PodDeserializer::deserialize_any_from(
        pod.as_bytes(),
    ) {
        Ok((_, v)) => v,
        Err(e) => return format!("<unparsable: {:?}>", e),
    };
    let Value::Object(obj) = value else {
        return format!("{:?}", value);
    };
    let mut parts = Vec::new();
    for prop in &obj.properties {
        parts.push(format!("{}= {:?}", prop.key, prop.value));
    }
    parts.join(", ")
}
