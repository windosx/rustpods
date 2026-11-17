#[macro_use]
extern crate rust_i18n;

i18n!("locales", fallback = "en");

pub mod core;
pub mod system;
pub mod ui;

use clap::Parser;
use std::sync::Arc;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser, Debug)]
#[command(
    name = "rustpods",
    version,
    about = "Linux AirPods Control & macOS Parity Experience"
)]
struct Cli {
    #[arg(short, long, help = "Enable debug logging")]
    debug: bool,

    #[arg(long, help = "Check if a newer version of RustPods is available")]
    check_update: bool,

    #[arg(long, help = "Download and install the latest version of RustPods")]
    update: bool,

    #[arg(long, help = "Run in headless daemon mode without GUI")]
    no_gui: bool,

    #[arg(
        long,
        value_name = "PATH",
        help = "Also write logs to this file (useful when started from the desktop \
                entry, where stdout goes nowhere)"
    )]
    log_file: Option<String>,

    #[arg(
        long,
        value_name = "MAC",
        help = "Connect to the given AirPods and print the head-tracking stream"
    )]
    head_tracking_test: Option<String>,

    #[arg(
        long,
        help = "With --head-tracking-test: sweep candidate service ids / intervals \
                to find which one actually starts the stream"
    )]
    sweep: bool,

    #[arg(
        long,
        help = "With --head-tracking-test: replay anilsrml/airpods-posture, the \
                Linux-native implementation (handshake, drain, alternate START)"
    )]
    linux_reference: bool,

    #[arg(
        long,
        help = "With --head-tracking-test: replay the StarNumber12046/airpods-head-track \
                sequence exactly (plain handshake, feature flags, notification request, \
                START frame) with no sensor subscriptions"
    )]
    reference: bool,

    #[arg(
        long,
        help = "With --head-tracking-test: run the spatial-session test - claim spatial \
                audio in head-tracked mode (the macOS precondition) before enabling"
    )]
    spatial: bool,

    #[arg(
        long,
        help = "With --head-tracking-test: speak Apple RemoteHID on descriptor 0x800 \
                (what AirPods 3 actually uses) instead of SensorDataWX"
    )]
    remotehid: bool,

    // ZCode head-gesture answering. These are plumbing commands rather than
    // user-facing modes, so they stay out of --help: the hook entry is
    // installed by --zcode-hook-install and invokes --zcode-ask-hook itself.
    #[arg(long, hide = true, help = "ZCode PreToolUse hook: answer AskUserQuestion with head nods")]
    zcode_ask_hook: bool,

    #[arg(
        long,
        hide = true,
        help = "Install the ZCode AskUserQuestion hook into ~/.zcode/cli/config.json"
    )]
    zcode_hook_install: bool,

    #[arg(long, hide = true, help = "Remove the installed ZCode AskUserQuestion hook")]
    zcode_hook_uninstall: bool,

    #[arg(long, hide = true, help = "Codex PreToolUse hook: answer ask_question with head nods")]
    codex_ask_hook: bool,

    #[arg(
        long,
        hide = true,
        help = "Install the Codex hook into ~/.codex/hooks.json"
    )]
    codex_hook_install: bool,

    #[arg(long, hide = true, help = "Remove the installed Codex hook")]
    codex_hook_uninstall: bool,

    #[arg(
        long,
        hide = true,
        help = "Send a sample question to the running RustPods to exercise the nod-answer path"
    )]
    zcode_ask_simulate: bool,
}

/// Exclusive per-session lock, held for the lifetime of the process.
///
/// Uses `flock` on a file under `$XDG_RUNTIME_DIR`, which is the conventional
/// location for per-user runtime state and is cleared on logout. An advisory
/// lock is chosen over a pidfile because the kernel releases it when the
/// process dies, so a crash never leaves a stale lock behind - a pidfile would, and
/// checking whether that pid is still alive races with pid reuse.
///
/// Every AACP engine takes this one lock - the `--no-gui` daemon, the GUI,
/// and the head-tracking diagnostics alike - because two engines make the
/// accessory stop answering for both. The GUI-vs-GUI case is exempted by
/// checking the GApplication bus name first: a second GUI launch must keep
/// activating the running window, not fail.
struct InstanceLock {
    _file: std::fs::File,
}

impl InstanceLock {
    fn acquire() -> Result<Self, String> {
        use std::os::unix::io::AsRawFd;

        // An empty-but-set variable is as good as unset (XDG spec); falling
        // back per key keeps the lock off the filesystem root either way.
        let dir = std::env::var("XDG_RUNTIME_DIR")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| std::env::temp_dir().to_string_lossy().into_owned());
        let path = format!("{}/rustpods-session.lock", dir);

        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| format!("Could not open lock file {}: {}", path, e))?;

        // SAFETY: the fd is valid for as long as `file` is alive, and LOCK_EX on
        // a regular file needs no other setup.
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                return Err(format!(
                    "Another RustPods engine (GUI or --no-gui daemon) is already running \
                     ({} is locked); only one can use the AACP session at a time",
                    path,
                ));
            }
            return Err(format!("Could not lock {}: {}", path, err));
        }

        // Best effort: record the pid for humans looking at the file. The lock
        // itself is what enforces exclusivity, not this.
        let _ = std::fs::write(&path, format!("{}\n", std::process::id()));
        Ok(Self { _file: file })
    }
}

/// Whether a RustPods GUI already owns its application bus name on the
/// session bus.
fn gui_already_running() -> bool {
    use zbus::blocking::Proxy;

    let Ok(conn) = zbus::blocking::Connection::session() else {
        return false;
    };
    let Ok(proxy) = Proxy::new(
        &conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    ) else {
        return false;
    };
    proxy
        .call::<_, _, bool>(
            "NameHasOwner",
            &("io.github.rustpods.RustPods".to_string(),),
        )
        .unwrap_or(false)
}

/// Asks an already-running RustPods GUI instance to bring its window forward.
fn activate_running_gui() -> bool {
    use zbus::blocking::Proxy;

    let Ok(conn) = zbus::blocking::Connection::session() else {
        return false;
    };
    let Ok(proxy) = Proxy::new(
        &conn,
        "io.github.rustpods.RustPods",
        "/io/github/rustpods/RustPods",
        "io.github.rustpods.RustPods",
    ) else {
        return false;
    };
    proxy.call::<_, _, ()>("Activate", &()).is_ok()
}

fn main() {
    let cli = Cli::parse();

    let log_level = if cli.debug {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };

    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(log_level.into())
        .from_env_lossy();

    // A file layer matters because the GUI is normally launched from the desktop
    // entry: stdout goes nowhere there, and `journalctl` sees nothing because the
    // process does not log through syslog.
    let log_file = cli.log_file.clone().map(|path| {
        use std::fs::OpenOptions;
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => Some(file),
            Err(e) => {
                eprintln!("could not open log file {}: {}", path, e);
                None
            }
        }
    });

    // Console logs go to stderr: `--zcode-ask-hook` prints its answer as JSON
    // on stdout, and a stray log line there could break the hook's parsing.
    match log_file.flatten() {
        Some(file) => {
            let writer = std::sync::Mutex::new(file);
            tracing_subscriber::registry()
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_ansi(false)
                        .with_writer(writer),
                )
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_ansi(false)
                        .with_writer(std::io::stderr),
                )
                .with(filter)
                .init();
        }
        None => {
            tracing_subscriber::registry()
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_ansi(false)
                        .with_writer(std::io::stderr),
                )
                .with(filter)
                .init();
        }
    }

    info!("Starting RustPods v{}...", env!("CARGO_PKG_VERSION"));

    // Spawn multi-threaded Tokio runtime for background Bluetooth & MPRIS
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to initialize Tokio runtime");

    let _guard = rt.enter();

    if cli.check_update {
        rt.block_on(async {
            println!("Checking for updates...");
            match system::updater::check_for_update().await {
                Ok(Some((latest, url, _))) => {
                    println!(
                        "A new version of RustPods is available: v{} (current: v{})",
                        latest,
                        env!("CARGO_PKG_VERSION")
                    );
                    println!("Release URL: {}", url);
                    if system::updater::is_package_managed() {
                        println!("Note: This installation appears to be managed by a system package manager (e.g. pacman, dpkg, rpm). Please update using your package manager.");
                    } else {
                        println!("Run 'rustpods --update' to download and install this update.");
                    }
                }
                Ok(None) => {
                    println!(
                        "RustPods is already up to date (v{}).",
                        env!("CARGO_PKG_VERSION")
                    );
                }
                Err(e) => {
                    eprintln!("Error checking for updates: {}", e);
                    std::process::exit(1);
                }
            }
        });
        return;
    }

    if cli.update {
        let is_pkg = system::updater::is_package_managed();
        if is_pkg {
            eprintln!("Error: This binary is located in a system package directory (/usr/bin).");
            eprintln!("To prevent package database corruption, please update using your package manager (e.g. yay, apt, dnf).");
            std::process::exit(1);
        }
        println!("Triggering RustPods in-place update...");
        let status = std::process::Command::new("bash")
            .arg("-c")
            .arg("curl -fsSL https://raw.githubusercontent.com/windosx/rustpods/master/contrib/scripts/update.sh | bash")
            .status();
        match status {
            Ok(s) if s.success() => {
                println!("Update complete.");
                return;
            }
            Ok(s) => {
                eprintln!("Update exited with status: {:?}", s.code());
                std::process::exit(s.code().unwrap_or(1));
            }
            Err(e) => {
                eprintln!("Failed to run update script: {}", e);
                std::process::exit(1);
            }
        }
    }

    if let Some(target) = cli.head_tracking_test.clone() {
        // The diagnostic drives the same AACP link as a running GUI or
        // daemon, so it takes the same exclusive lock instead of silently
        // yanking the session out from under one.
        let _instance_lock = match InstanceLock::acquire() {
            Ok(lock) => lock,
            Err(reason) => {
                error!("{}", reason);
                std::process::exit(1);
            }
        };
        info!("Head-tracking diagnostic against {}", target);
        let sweep = cli.sweep;
        let reference = cli.reference;
        let linux_reference = cli.linux_reference;
        let spatial = cli.spatial;
        let remotehid = cli.remotehid;
        rt.block_on(async move {
            if remotehid {
                run_remotehid_test(&target).await;
            } else if spatial {
                run_spatial_session_test(&target).await;
            } else if linux_reference {
                run_linux_reference_sequence(&target).await;
            } else if reference {
                run_reference_sequence(&target).await;
            } else if sweep {
                run_head_tracking_sweep(&target).await;
            } else {
                run_head_tracking_test(&target).await;
            }
        });
        return;
    }

    // Agent hooks plumbing. These run standalone (no GTK, no Bluetooth, no instance
    // lock) because the hook fires while ZCode / Codex is already talking to the real
    // daemon over the socket.
    if cli.zcode_ask_hook || cli.codex_ask_hook {
        let code = rt.block_on(system::zcode_ask::run_hook_mode());
        std::process::exit(code);
    }
    if cli.zcode_hook_install {
        match system::zcode_ask::install_hook() {
            Ok(()) => info!("ZCode hook installed"),
            Err(e) => {
                error!("Installing the ZCode hook failed: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }
    if cli.zcode_hook_uninstall {
        match system::zcode_ask::uninstall_hook() {
            Ok(()) => info!("ZCode hook removed"),
            Err(e) => {
                error!("Removing the ZCode hook failed: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }
    if cli.codex_hook_install {
        match system::codex_ask::install_hook() {
            Ok(()) => info!("Codex hook installed"),
            Err(e) => {
                error!("Installing the Codex hook failed: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }
    if cli.codex_hook_uninstall {
        match system::codex_ask::uninstall_hook() {
            Ok(()) => info!("Codex hook removed"),
            Err(e) => {
                error!("Removing the Codex hook failed: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }
    if cli.zcode_ask_simulate {
        let code = rt.block_on(system::zcode_ask::run_simulate());
        std::process::exit(code);
    }

    if cli.no_gui {
        // The GUI gets single-instance behaviour from GApplication, which owns a
        // well-known bus name. The daemon has no such name, so it takes an
        // exclusive lock instead - the usual approach for background services.
        // Only one process may hold the AACP session anyway: a second one makes
        // the accessory stop answering for both.
        let _instance_lock = match InstanceLock::acquire() {
            Ok(lock) => lock,
            Err(reason) => {
                error!("{}", reason);
                std::process::exit(1);
            }
        };

        info!("Running in headless daemon mode");
        rt.block_on(async {
            let config = Arc::new(std::sync::RwLock::new(system::config::load_config()));
            let initial_locale = config.read().unwrap().language.resolved_locale();
            rust_i18n::set_locale(initial_locale);
            let device_state = Arc::new(tokio::sync::RwLock::new(None));
            let aacp_manager = Arc::new(core::aacp::AacpManager::new());
            let mpris_controller = Arc::new(system::mpris::MprisController::new());
            let (ui_tx, mut ui_rx) = tokio::sync::mpsc::unbounded_channel();

            // Graceful-shutdown signal for the tray's quit item: returning
            // from here (rather than std::process::exit) lets the runtime drop
            // every task, whose Drops run - most importantly the HRTF client
            // restoring the default audio sink.
            let (quit_tx_raw, quit_rx) = tokio::sync::oneshot::channel::<()>();
            // A oneshot sender is consumed by `send`; the tray callback may
            // fire more than once, so keep it in a take-cell behind a mutex
            // (the callback is a plain `Fn`).
            let quit_tx = std::sync::Mutex::new(Some(quit_tx_raw));

            // Spawn tray in daemon mode
            let tray_state = device_state.clone();
            let aacp_noise = aacp_manager.clone();
            let tray_dev_noise = device_state.clone();
            let tray_tx_noise = ui_tx.clone();
            let aacp_ear = aacp_manager.clone();
            let tray_config = config.clone();
            let tray_dev_ear = device_state.clone();
            let tray_tx_ear = ui_tx.clone();
            let tray = system::tray::RustPodsTray {
                device_state: tray_state,
                on_open_window: Arc::new(|| {}),
                on_set_noise_mode: Arc::new(move |mode| {
                    let aacp = aacp_noise.clone();
                    let dev = tray_dev_noise.clone();
                    let tx = tray_tx_noise.clone();
                    tokio::spawn(async move {
                        system::tray::apply_noise_mode(&aacp, &dev, &tx, mode).await;
                    });
                }),
                on_toggle_ear_detection: Arc::new(move |enabled| {
                    let aacp = aacp_ear.clone();
                    // Persist like the GUI tray does: the daemon's auto-pause
                    // gate reads the config, so a tray-only toggle used to
                    // leave the pause behaviour untouched.
                    let cfg = tray_config.clone();
                    let dev = tray_dev_ear.clone();
                    let tx = tray_tx_ear.clone();
                    tokio::spawn(async move {
                        system::tray::apply_ear_detection_toggle(&aacp, &cfg, &dev, &tx, enabled)
                            .await;
                    });
                }),
                on_quit: Arc::new(move || {
                    // Take the sender out so repeat clicks are no-ops.
                    if let Some(tx) = quit_tx.lock().ok().and_then(|mut slot| slot.take()) {
                        let _ = tx.send(());
                    }
                }),
            };
            tokio::spawn(async move {
                use ksni::TrayMethods;
                let _ = tray.spawn().await;
            });

            // Consume UI events in background
            tokio::spawn(async move {
                while ui_rx.recv().await.is_some() {}
            });

            // Serve ZCode question hooks in the daemon too: whichever process
            // wins the socket answers the hook, and both run the same engine.
            tokio::spawn(system::zcode_ask::run_socket_server());

            // Head-tracked audio, once per process (see run_audio_follow).
            tokio::spawn(ui::run_audio_follow(aacp_manager.head_pose()));

            // Head-tracked mouse controller.
            tokio::spawn(system::head_mouse::run_head_mouse_controller(
                aacp_manager.head_pose(),
                config.clone(),
            ));

            tokio::spawn(async move {
                ui::start_bluetooth_daemon(
                    aacp_manager,
                    device_state,
                    mpris_controller,
                    ui_tx,
                    config,
                )
                .await;
            });

            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = quit_rx => {
                    info!("Shutdown requested from the tray");
                }
            }
            info!("Daemon shutting down");
        });
    } else {
        // The GUI is an AACP engine just like the daemon, so it takes the
        // same exclusive lock. A second GUI launch is still fine: when the
        // single-instance D-Bus name is already owned, we ask that instance to
        // present its window and exit cleanly.
        let _instance_lock = match InstanceLock::acquire() {
            Ok(lock) => lock,
            Err(_) if gui_already_running() => {
                if activate_running_gui() {
                    info!("Activated existing RustPods GUI instance");
                    return;
                }
                error!("Existing RustPods GUI instance did not respond to Activate");
                std::process::exit(1);
            }
            Err(reason) => {
                error!("{}", reason);
                std::process::exit(1);
            }
        };

        // Run GPUI application
        ui::run_app(rt.handle().clone());
    }
}

/// Connects to the given AirPods and reports what the head-tracking stream
/// actually delivers, so the protocol can be validated against a real device
/// without the GUI.
/// Exercises the spatial-session layer end to end.
///
/// macOS will not send the enable until an application claims spatial audio in
/// head-tracked mode, so this runs the same order: claim first, then let the
/// enable path decide whether it is allowed to fire.
///
/// What this proves and what it does not: it shows the host-side precondition is
/// modelled and enforced. Whether the accessory starts streaming in response is
/// the open question the previous sweep could not settle either, so the test
/// reports sample counts rather than asserting success.
async fn run_spatial_session_test(target: &str) {
    use crate::core::spatial::SpatialMode;
    use std::time::Duration;

    let Some(aacp) = prepare_connection(target).await else {
        return;
    };

    tracing::info!("Connecting AACP...");
    if let Err(e) = aacp.connect(addr_of(target)).await {
        tracing::error!("AACP connect failed: {}", e);
        return;
    }

    // Step 1: the claim macOS makes before it will send anything.
    tracing::info!("Claiming spatial audio in head-tracked mode (the macOS precondition)");
    let permitted = aacp
        .claim_spatial_audio("io.github.rustpods.RustPods", SpatialMode::HeadTracked, true)
        .await;
    tracing::info!("Session permits head tracking: {}", permitted);

    if let Some((app, mode, tracking)) = aacp.spatial_snapshot().await {
        tracing::info!("Current claim: {} mode={:?} head_tracking={}", app, mode, tracking);
    }

    // Step 2: let the gated enable path run.
    match aacp.start_head_tracking().await {
        Ok(()) => tracing::info!("start_head_tracking accepted the claim"),
        Err(e) => tracing::warn!("start_head_tracking refused: {}", e),
    }

    for second in 1..=20 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let samples = aacp.head_samples_seen().await;
        let rate = aacp.head_tracking_rate_hz().await;
        tracing::info!(
            "t={:>2}s  samples={:<5} rate={}",
            second,
            samples,
            rate.map(|r| format!("{:.1} Hz", r))
                .unwrap_or_else(|| "—".to_string())
        );
        if samples > 0 {
            tracing::info!(">>> Stream started with a spatial claim in place");
            break;
        }
    }

    let samples = aacp.head_samples_seen().await;
    if samples == 0 {
        tracing::warn!("No samples arrived despite the claim.");
        tracing::warn!("The host side of the chain is in place; the accessory still");
        tracing::warn!("does not answer 0x800, which means the missing piece is not");
        tracing::warn!("only this state machine - cycle the buds through the case and retry.");
    }

    // Step 3: releasing must stop the retries.
    aacp.release_spatial_audio("io.github.rustpods.RustPods").await;
    tracing::info!("Spatial claim released; retries should have stopped");
}

/// Speaks Apple RemoteHID on descriptor `0x800`.
///
/// AirPods 3 does not implement `SensorDataWX` (`0x100000`) at all; motion data
/// arrives through this channel instead. See `core::aacp::remotehid` for the
/// frame layout and for what is and is not verified on this hardware.
/// Runs the RemoteHID stream, recovering the accessory on its own when needed.
///
/// Two hardware quirks make this more than a straight-line sequence:
///
/// * AirPods 3 can stop announcing its sensors while both buds are in ear. Only a
///   full Bluetooth disconnect clears that, and the reconnect has to wait —
///   reconnecting at once leaves the unit stuck. [`StreamSupervisor`] decides when
///   to cycle and `RECOVERY_GAP` sets how long to pause.
/// * The stream is cancelled whenever the accessory re-announces its devices, so
///   the interval write is repeated on every fresh announcement.
///
/// Everything here runs unattended: the earbuds never have to be touched.
async fn run_remotehid_test(target: &str) {
    use crate::core::aacp::remotehid;
    use crate::core::aacp::session::{Recovery, StreamState, StreamSupervisor};
    use std::time::Duration;

    const MAX_ATTEMPTS: u32 = 4;

    let Some(aacp) = prepare_connection(target).await else {
        return;
    };

    let mut supervisor = StreamSupervisor::new();
    let mut generation = remotehid::Generation::default();

    // One connection, watched patiently. Do not disconnect: cycling the link was
    // tried and it is destructive, pushing the accessory into a state where it
    // refuses paging and only a physical case cycle brings it back.
    for attempt in 1..=MAX_ATTEMPTS {
        supervisor.begin_attempt();

        tracing::info!("Connecting AACP (attempt {}/{})", attempt, MAX_ATTEMPTS);
        if let Err(e) = aacp.connect_without_handshake(addr_of(target)).await {
            tracing::error!("AACP connect failed: {}", e);
            continue;
        }

        // The reference client sends exactly these three packets before anything
        // else; extra handshake steps change what the accessory will announce.
        for packet in [
            vec![0x00u8, 0x00, 0x04, 0x00, 0x01, 0x00, 0x02, 0x00, 0, 0, 0, 0, 0, 0, 0, 0],
            vec![0x04, 0x00, 0x04, 0x00, 0x4D, 0x00, 0xD7, 0x00, 0, 0, 0, 0, 0, 0],
            vec![0x04, 0x00, 0x04, 0x00, 0x0F, 0x00, 0xFF, 0xFF, 0xFF, 0xFF],
        ] {
            let _ = aacp.send_raw(packet).await;
            tokio::time::sleep(Duration::from_millis(150)).await;
        }

        let mut frames = aacp.subscribe_rtbuddy_frames();
        let mut registry = remotehid::DeviceRegistry::new();

        // Collect the announcement burst. It only contains every service when the
        // receive MTU is 2582 (handled before connect), so a short list here is
        // itself a signal that something is off.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let Ok(Ok(frame)) = tokio::time::timeout(remaining, frames.recv()).await else {
                break;
            };
            remotehid::record_device_connect(&mut registry, &frame);
        }

        let announced: Vec<String> = registry.all().map(|d| format!("{}={}", d.service, d.device)).collect();
        tracing::info!("Announced {} services: {:?}", announced.len(), announced);

        match supervisor.record_announcement(&registry, &mut generation) {
            Some(packet) if !packet.is_empty() => {
                tracing::info!(
                    "Writing the sample interval: {:02X?}",
                    &packet[..packet.len().min(26)]
                );
                if let Err(e) = aacp.send_raw(packet).await {
                    tracing::error!("SetReport failed: {}", e);
                    continue;
                }
            }
            Some(_) => tracing::debug!("Interval already written for this device set"),
            None if !announced.is_empty() => {
                // Some services but not the motion one: the accessory is still
                // working through its list. Keep this connection; reconnecting
                // would restart that progress from zero.
                match supervisor.after_partial(announced.len()) {
                    Recovery::KeepWaiting => {
                        tracing::info!(
                            "{} services so far, waiting for the motion sensor",
                            announced.len()
                        );
                        // Give it time to finish announcing on this connection.
                        let extra = tokio::time::Instant::now() + Duration::from_secs(8);
                        while tokio::time::Instant::now() < extra {
                            let remaining =
                                extra.saturating_duration_since(tokio::time::Instant::now());
                            let Ok(Ok(frame)) = tokio::time::timeout(remaining, frames.recv()).await
                            else {
                                continue;
                            };
                            remotehid::record_device_connect(&mut registry, &frame);
                        }
                        let now: Vec<String> =
                            registry.all().map(|d| d.service.clone()).collect();
                        tracing::info!("After waiting: {} services {:?}", now.len(), now);
                        if let Some(packet) =
                            supervisor.record_announcement(&registry, &mut generation)
                            && !packet.is_empty() {
                                let _ = aacp.send_raw(packet).await;
                            }
                    }
                    Recovery::CycleLink | Recovery::WaitForWearer => continue,
                }
            }
            None => {
                // Nothing announced. A unit in its case powers the sensors down,
                // so only cycle the link when the buds are actually being worn.
                // A unit in its case powers the sensors down, so cycling the
                // link there would be pointless; the ear state comes from the
                // last ear-detection notification the manager cached.
                let in_ear = aacp.last_ear_status().await.map(|(l, r)| {
                    l.is_in_ear() || r.is_in_ear()
                }).unwrap_or(true);
                match supervisor.after_silence(in_ear) {
                    Recovery::CycleLink => continue,
                    Recovery::KeepWaiting => {
                        // Announcements can take several seconds on a cold
                        // accessory. Watch this same connection instead of
                        // reconnecting, which restarts its init from zero.
                        tracing::info!("No announcements yet; watching this link for 10 s");
                        let until = tokio::time::Instant::now() + Duration::from_secs(10);
                        while tokio::time::Instant::now() < until {
                            let remaining = until.saturating_duration_since(tokio::time::Instant::now());
                            let Ok(Ok(frame)) = tokio::time::timeout(remaining, frames.recv()).await
                            else {
                                continue;
                            };
                            remotehid::record_device_connect(&mut registry, &frame);
                        }
                        let names: Vec<String> = registry.all().map(|d| d.service.clone()).collect();
                        tracing::info!("After waiting on this link: {} services {:?}", names.len(), names);
                        if let Some(packet) = supervisor.record_announcement(&registry, &mut generation) {
                            if !packet.is_empty() {
                                tracing::info!("Writing the sample interval after wait");
                                let _ = aacp.send_raw(packet).await;
                            }
                        } else {
                            continue;
                        }
                    }
                    Recovery::WaitForWearer => {
                        tracing::info!(
                            "Buds appear to be out of ear; the sensors stay off until worn"
                        );
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                }
            }
        }

        // Samples should start within a second. Watch for a few seconds so a
        // silent failure is still caught before declaring success.
        for second in 1..=10 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let seen = aacp.head_samples_seen().await;
            if seen > 0 {
                let rate = aacp.head_tracking_rate_hz().await;
                tracing::info!(
                    ">>> Streaming: {} samples, {}",
                    seen,
                    rate.map(|r| format!("{:.0} Hz", r))
                        .unwrap_or_else(|| "rate pending".into())
                );
                if second >= 3 {
                    tracing::info!("Head tracking verified; leaving the stream running");
                    let _ = aacp.stop_head_tracking().await;
                    return;
                }
            } else if second == 10 {
                tracing::warn!("Interval written but no samples arrived");
            }
        }

        if matches!(supervisor.state(), StreamState::Streaming { .. }) {
            tracing::warn!("Stream did not produce samples; retrying with a link reset");
        }
    }

    tracing::error!(
        "Gave up after {} attempts. If the buds are in the case the sensors are \
         powered down; otherwise the accessory is stuck and needs a manual reset.",
        MAX_ATTEMPTS
    );
}



async fn run_head_tracking_test(target: &str) {
    use std::time::Duration;

    let aacp = match prepare_connection(target).await {
        Some(aacp) => aacp,
        None => return,
    };

    // Everything the accessory sends on the RTBuddy descriptor, unfiltered:
    // this is how the service-id/cookie reply format gets established.
    let mut replies = aacp.subscribe_rtbuddy_frames();
    tokio::spawn(async move {
        while let Ok(frame) = replies.recv().await {
            tracing::info!("RTBuddy reply [{}]: {:02X?}", frame.len(), frame);
        }
    });

    let mut poses = aacp.head_pose();
    let aacp_rate = aacp.clone();
    let pose_task = tokio::spawn(async move {
        while poses.changed().await.is_ok() {
            if let Some(pose) = *poses.borrow_and_update() {
                tracing::debug!(
                    "pose pitch={:.1}° yaw={:.1}° roll={:.1}°",
                    pose.pitch, pose.yaw, pose.roll
                );
            }
        }
    });
    let mut gestures = aacp.subscribe_gestures();
    let gesture_task = tokio::spawn(async move {
        while let Ok(gesture) = gestures.recv().await {
            tracing::info!(">>> GESTURE: {:?}", gesture);
        }
    });

    tracing::info!("Connecting AACP...");
    if let Err(e) = aacp.connect(addr_of(target)).await {
        tracing::error!("AACP connect failed: {}", e);
        return;
    }

    // The handshake already subscribed to the sensor services; now do the
    // cookie + 14-byte enable step that actually arms the stream.
    tokio::time::sleep(Duration::from_millis(600)).await;
    match aacp.start_head_tracking().await {
        Ok(()) => tracing::info!("start_head_tracking returned Ok"),
        Err(e) => tracing::warn!("start_head_tracking failed: {}", e),
    }

    for second in 1..=15 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let active = aacp_rate.is_head_tracking_active().await;
        let rate = aacp_rate.head_tracking_rate_hz().await;
        tracing::info!(
            "t={:>2}s  streaming={:<5} rate={}",
            second,
            active,
            rate.map(|r| format!("{:.1} Hz", r))
                .unwrap_or_else(|| "—".to_string())
        );
    }

    tracing::info!("Stopping head tracking...");
    let _ = aacp.stop_head_tracking().await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    gesture_task.abort();
    pose_task.abort();
    tracing::info!("Diagnostic complete");
}

/// Replays `anilsrml/airpods-posture`, the only Linux-native AirPods head
/// tracking implementation found: same L2CAP PSM, same kernel socket type.
///
/// Its whole sequence is just two writes — the plain 16-byte handshake, a
/// 1.5 s drain of the opening packets, then the alternate START frame
/// (service 16, seq 115). No sensor subscriptions, no HID enable.
async fn run_linux_reference_sequence(target: &str) {
    use std::time::Duration;

    let Some(aacp) = prepare_connection(target).await else {
        return;
    };

    if let Err(e) = aacp.connect_without_handshake(addr_of(target)).await {
        tracing::error!("AACP connect failed: {}", e);
        return;
    }

    // The shell implementation opens the socket, sends the handshake
    // immediately, then drains for 1.5 s before the START write.
    let handshake: [u8; 16] = [
        0x00, 0x00, 0x04, 0x00, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00,
    ];
    tracing::info!("Sending handshake: {:02X?}", handshake);
    let _ = aacp.send_raw(handshake.to_vec()).await;

    tracing::info!("Draining opening packets for 1.5s...");
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let start = core::aacp::rtbuddy::make_alternate_start_head_tracking(
        core::aacp::rtbuddy::INTERVAL_25HZ_US,
    );
    tracing::info!("Sending alternate START ({}B): {:02X?}", start.len(), start);
    let _ = aacp.send_raw(start).await;

    for second in 1..=25 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let seen = aacp.head_samples_seen().await;
        let rate = aacp.head_tracking_rate_hz().await;
        tracing::info!(
            "t={:>2}s  samples={:<5} rate={}",
            second,
            seen,
            rate.map(|r| format!("{:.1} Hz", r))
                .unwrap_or_else(|| "—".to_string())
        );
        if seen > 0 {
            info_once(&aacp).await;
        }
    }

    let _ = aacp.stop_head_tracking().await;
    tracing::info!("Linux reference sequence test complete");
}

/// Logs the "stream is flowing" line only the first time samples appear.
async fn info_once(aacp: &Arc<core::aacp::AacpManager>) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static ANNOUNCED: AtomicBool = AtomicBool::new(false);
    if !ANNOUNCED.swap(true, Ordering::Relaxed) {
        let seen = aacp.head_samples_seen().await;
        tracing::info!(">>> STREAM IS FLOWING ({} samples)", seen);
    }
}

/// Replays the sequence from `StarNumber12046/airpods-head-track`, a project
/// that reports working head tracking on AirPods 3 (and Pro/Max) by sending only
/// four things: the plain 16-byte handshake, feature flags, a notification
/// request, and the START frame.
///
/// Notably it does **not** subscribe to the sensor services and does **not**
/// send a 14-byte HID enable, which is what our own enable path had been trying.
/// If this produces samples on our hardware, the extra steps were the problem.
async fn run_reference_sequence(target: &str) {
    use std::time::Duration;

    let Some(aacp) = prepare_connection(target).await else {
        return;
    };

    // Connect with no handshake so every byte on the wire is ours to account for.
    tracing::info!("Opening AACP socket...");
    if let Err(e) = aacp.connect_without_handshake(addr_of(target)).await {
        tracing::error!("AACP connect failed: {}", e);
        return;
    }

    // The reference implementation drains stale driver bytes and waits 500 ms
    // before speaking; keep the settle window, it also matches what the
    // accessory expects after L2CAP comes up.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let handshake: [u8; 16] = [
        0x00, 0x00, 0x04, 0x00, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00,
    ];
    tracing::info!("1/4 handshake: {:02X?}", handshake);
    let _ = aacp.send_raw(handshake.to_vec()).await;

    // The reference waits for the handshake ACK before continuing.
    tokio::time::sleep(Duration::from_millis(400)).await;

    let feature_flags: [u8; 10] = [
        0x04, 0x00, 0x04, 0x00, 0x4D, 0x00, 0xD7, 0x00, 0x00, 0x00,
    ];
    tracing::info!("2/4 feature flags: {:02X?}", feature_flags);
    let _ = aacp.send_raw(feature_flags.to_vec()).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let notif_req: [u8; 10] = [0x04, 0x00, 0x04, 0x00, 0x0F, 0x00, 0xFF, 0xFF, 0xFF, 0xFF];
    tracing::info!("3/4 notification request: {:02X?}", notif_req);
    let _ = aacp.send_raw(notif_req.to_vec()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let start = core::aacp::rtbuddy::make_head_tracking_start(core::aacp::rtbuddy::INTERVAL_25HZ_US);
    tracing::info!("4/4 START frame: {:02X?}", start);
    let _ = aacp.send_raw(start).await;

    for second in 1..=20 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let rate = aacp.head_tracking_rate_hz().await;
        let seen = aacp.head_samples_seen().await;
        tracing::info!(
            "t={:>2}s  samples={:<4} rate={}",
            second,
            seen,
            rate.map(|r| format!("{:.1} Hz", r))
                .unwrap_or_else(|| "—".to_string())
        );
        if seen > 0 {
            tracing::info!(">>> STREAM IS FLOWING after {} samples", seen);
        }
    }

    let _ = aacp.stop_head_tracking().await;
    tracing::info!("Reference-sequence test complete");
}

/// Tries every plausible (service id, interval) combination because the
/// service id that actually streams `SensorV2` could not be confirmed from the
/// captures: LibrePods uses 14 at 40 ms, the macOS capture shows ~20 ms, and the
/// accessory's own PRXC table lists 20 000 µs.
async fn run_head_tracking_sweep(target: &str) {
    use std::time::Duration;

    const SERVICES: [u8; 6] = [14, 16, 6, 7, 8, 9];
    const INTERVALS_US: [u32; 3] = [20_000, 40_000, 10_000];
    const LISTEN: Duration = Duration::from_secs(3);

    let Some(aacp) = prepare_connection(target).await else {
        return;
    };

    tracing::info!("Connecting AACP (full handshake)...");
    if let Err(e) = aacp.connect(addr_of(target)).await {
        tracing::error!("AACP connect failed: {}", e);
        return;
    }
    // Let the handshake settle before probing.
    tokio::time::sleep(Duration::from_millis(800)).await;

    let mut found = Vec::new();

    for service in SERVICES {
        for interval in INTERVALS_US {
            let frame = core::aacp::rtbuddy::make_head_tracking_start_for(service, interval);
            tracing::info!(
                "--- probe service={} interval={}us ...",
                service,
                interval
            );
            if let Err(e) = aacp.send_raw(frame).await {
                tracing::warn!("send failed: {}", e);
                continue;
            }

            // Poll instead of waiting the full window so a hit is found fast.
            let deadline = std::time::Instant::now() + LISTEN;
            let mut hit: Option<f32> = None;
            while std::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(250)).await;
                if let Some(rate) = aacp.head_tracking_rate_hz().await {
                    hit = Some(rate);
                    break;
                }
            }

            match hit {
                Some(rate) => {
                    tracing::info!(
                        ">>> STREAM STARTED with service={} interval={}us ({:.1} Hz)",
                        service,
                        interval,
                        rate
                    );
                    found.push((service, interval, rate));
                    // Stop and move on so each combination is measured cleanly.
                    let _ = aacp.stop_head_tracking().await;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                None => tracing::info!("    no stream"),
            }
        }
    }

    if found.is_empty() {
        tracing::warn!("No (service, interval) combination started the stream");
    } else {
        for (service, interval, rate) in &found {
            tracing::info!(
                "RESULT service={} interval={}us rate={:.1} Hz",
                service,
                interval,
                rate
            );
        }
    }

    let _ = aacp.stop_head_tracking().await;
    tracing::info!("Sweep complete");
}

fn addr_of(target: &str) -> bluer::Address {
    target.parse().expect("address validated by prepare_connection")
}

/// Ensures the device is attached and builds a manager with logging attached.
async fn prepare_connection(target: &str) -> Option<Arc<core::aacp::AacpManager>> {
    use std::time::Duration;

    let addr: bluer::Address = match target.parse() {
        Ok(addr) => addr,
        Err(e) => {
            tracing::error!("Invalid MAC address '{}': {}", target, e);
            return None;
        }
    };

    tracing::info!("Ensuring {} is connected...", addr);
    // Through BlueZ instead of a `bluetoothctl` shell-out: the API resolves
    // when the link is actually up, so the fixed 3 s sleep the shell version
    // needed is replaced by a short profile-settle wait.
    match crate::system::bluetooth::BluetoothManager::new().await {
        Ok(bm) => {
            if let Err(e) = bm.ensure_connected(addr).await {
                tracing::warn!("Could not connect to {}: {} (continuing anyway)", addr, e);
            }
        }
        Err(e) => tracing::warn!("BlueZ unavailable for the diagnostic connect: {}", e),
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let aacp = Arc::new(core::aacp::AacpManager::new());

    let mut events = aacp.subscribe();
    tokio::spawn(async move {
        while let Ok(event) = events.recv().await {
            match event {
                core::aacp::events::AacpEvent::Disconnected => {
                    tracing::warn!("Disconnected");
                    break;
                }
                core::aacp::events::AacpEvent::HeadSensor(_) => {}
                other => tracing::info!("AACP event: {:?}", other),
            }
        }
    });

    Some(aacp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// `contrib/scripts/update.sh` reads the installed version from
    /// `rustpods --version`; without this flag its up-to-date short-circuit is
    /// dead code and every update re-downloads the release.
    #[test]
    fn cli_exposes_version_for_the_updater() {
        let command = Cli::command();
        assert_eq!(command.get_version(), Some(env!("CARGO_PKG_VERSION")));
    }
}
