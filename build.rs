fn main() {
    #[cfg(target_os = "linux")]
    ensure_xkbcommon_x11_symlink();
}

#[cfg(target_os = "linux")]
fn ensure_xkbcommon_x11_symlink() {
    use std::path::Path;

    let candidates = [
        "/usr/lib64/libxkbcommon-x11.so.0",
        "/usr/lib/x86_64-linux-gnu/libxkbcommon-x11.so.0",
        "/usr/lib/libxkbcommon-x11.so.0",
    ];
    let dev_symlinks = [
        "/usr/lib64/libxkbcommon-x11.so",
        "/usr/lib/x86_64-linux-gnu/libxkbcommon-x11.so",
        "/usr/lib/libxkbcommon-x11.so",
    ];

    if dev_symlinks.iter().any(|p| Path::new(p).exists()) {
        return;
    }

    if let Some(target) = candidates.iter().find(|p| Path::new(p).exists()) {
        let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
        let link = out_dir.join("libxkbcommon-x11.so");
        let _ = std::fs::remove_file(&link);
        if std::os::unix::fs::symlink(target, &link).is_ok() {
            println!("cargo:rustc-link-search=native={}", out_dir.display());
        }
    }
}
