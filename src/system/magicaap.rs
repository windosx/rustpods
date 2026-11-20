use crate::core::models::MacAddress;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
#[cfg(target_os = "windows")]
use tracing::{debug, info};

pub const MAGICAAP_DEVICE_GUID: &str = "{9eec98bb-3c54-45d4-a843-7900c4635e08}";
pub const MAGICAAP_SYM_LINK: &str = r"\\.\MagicAAP";
pub const MAGICAAP_ALT_LINK: &str = r"\\.\MagicPodsL2cap";
pub const MAGICAAP_PIPE: &str = r"\\.\pipe\magicaap-l2cap";
pub const RUSTPODS_L2CAP_PIPE: &str = r"\\.\pipe\rustpods-l2cap";

pub const PSM_AACP: u16 = 0x1001;
pub const AACP_RECEIVE_MTU: u16 = 2582;

const FILE_DEVICE_UNKNOWN: u32 = 0x00000022;
const METHOD_BUFFERED: u32 = 0;
const FILE_ANY_ACCESS: u32 = 0;

macro_rules! ctl_code {
    ($dev:expr, $func:expr, $method:expr, $access:expr) => {
        (($dev) << 16) | (($access) << 14) | (($func) << 2) | ($method)
    };
}

pub const IOCTL_MAGICAAP_CONNECT: u32 =
    ctl_code!(FILE_DEVICE_UNKNOWN, 0x801, METHOD_BUFFERED, FILE_ANY_ACCESS);
pub const IOCTL_MAGICAAP_DISCONNECT: u32 =
    ctl_code!(FILE_DEVICE_UNKNOWN, 0x802, METHOD_BUFFERED, FILE_ANY_ACCESS);

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct MagicAapConnectReq {
    pub bth_addr: u64,
    pub psm: u16,
    pub mtu: u16,
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy)]
struct SendHandle(windows::Win32::Foundation::HANDLE);
#[cfg(target_os = "windows")]
unsafe impl Send for SendHandle {}
#[cfg(target_os = "windows")]
unsafe impl Sync for SendHandle {}

#[cfg(target_os = "windows")]
impl SendHandle {
    pub fn raw(&self) -> windows::Win32::Foundation::HANDLE {
        self.0
    }
}

#[derive(Clone)]
pub struct MagicAapStream {
    tx_outbound: mpsc::Sender<Vec<u8>>,
    rx_inbound: Arc<Mutex<mpsc::Receiver<Vec<u8>>>>,
    closed: Arc<AtomicBool>,
}

#[cfg(target_os = "windows")]
pub fn is_driver_available() -> bool {
    use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_EXISTING,
    };

    let candidates = [
        windows::core::w!(r"\\.\MagicAAP"),
        windows::core::w!(r"\\.\MagicPodsL2cap"),
        windows::core::w!(r"\\.\pipe\magicaap-l2cap"),
        windows::core::w!(r"\\.\pipe\rustpods-l2cap"),
    ];

    for path in candidates {
        unsafe {
            let handle = CreateFileW(
                path,
                FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            );
            if let Ok(h) = handle {
                if h != INVALID_HANDLE_VALUE && !h.is_invalid() {
                    let _ = CloseHandle(h);
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(not(target_os = "windows"))]
pub fn is_driver_available() -> bool {
    false
}

impl MagicAapStream {
    pub async fn connect(addr: MacAddress) -> Result<Self, String> {
        #[cfg(target_os = "windows")]
        {
            use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
            use windows::Win32::Storage::FileSystem::{
                CreateFileW, ReadFile, WriteFile, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
                FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
            };
            use windows::Win32::System::IO::DeviceIoControl;

            info!(
                "Attempting MagicAAP / L2CAP driver bridge connection to {}...",
                addr
            );

            let candidates = [
                windows::core::w!(r"\\.\MagicAAP"),
                windows::core::w!(r"\\.\MagicPodsL2cap"),
                windows::core::w!(r"\\.\pipe\magicaap-l2cap"),
                windows::core::w!(r"\\.\pipe\rustpods-l2cap"),
            ];

            let mut device_handle: Option<HANDLE> = None;
            for path in candidates {
                unsafe {
                    let handle = CreateFileW(
                        path,
                        FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
                        FILE_SHARE_READ | FILE_SHARE_WRITE,
                        None,
                        OPEN_EXISTING,
                        windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(0),
                        None,
                    );
                    if let Ok(h) = handle {
                        if h != INVALID_HANDLE_VALUE && !h.is_invalid() {
                            info!("Opened MagicAAP driver/pipe interface");
                            device_handle = Some(h);
                            break;
                        }
                    }
                }
            }

            let handle = device_handle.ok_or_else(|| {
                "MagicAAP kernel driver or L2CAP bridge not detected. Operating in Native Driverless Mode."
                    .to_string()
            })?;

            // Convert MAC address to 64-bit integer
            let b = addr.0;
            let bth_addr = u64::from_be_bytes([0, 0, b[0], b[1], b[2], b[3], b[4], b[5]]);

            let req = MagicAapConnectReq {
                bth_addr,
                psm: PSM_AACP,
                mtu: AACP_RECEIVE_MTU,
            };

            // Issue IOCTL_MAGICAAP_CONNECT to create kernel L2CAP channel
            let mut bytes_returned: u32 = 0;
            let ioctl_ok = unsafe {
                DeviceIoControl(
                    handle,
                    IOCTL_MAGICAAP_CONNECT,
                    Some(&req as *const _ as *const _),
                    std::mem::size_of::<MagicAapConnectReq>() as u32,
                    None,
                    0,
                    Some(&mut bytes_returned),
                    None,
                )
            };

            if let Err(e) = ioctl_ok {
                debug!(
                    "DeviceIoControl connect returned error (may be pipe mode): {}",
                    e
                );
            }

            info!("MagicAAP L2CAP channel established to {}", addr);

            let (tx_outbound, mut rx_outbound) = mpsc::channel::<Vec<u8>>(128);
            let (tx_inbound, rx_inbound) = mpsc::channel::<Vec<u8>>(128);
            let closed = Arc::new(AtomicBool::new(false));

            let handle_read = SendHandle(handle);
            let closed_read = Arc::clone(&closed);
            std::thread::Builder::new()
                .name("magicaap-reader".to_string())
                .spawn(move || {
                    let mut buf = vec![0u8; AACP_RECEIVE_MTU as usize];
                    while !closed_read.load(Ordering::Relaxed) {
                        let mut bytes_read: u32 = 0;
                        let ok = unsafe {
                            ReadFile(
                                handle_read.raw(),
                                Some(buf.as_mut_slice()),
                                Some(&mut bytes_read),
                                None,
                            )
                        };
                        if ok.is_ok() && bytes_read > 0 {
                            let packet = buf[..bytes_read as usize].to_vec();
                            if tx_inbound.blocking_send(packet).is_err() {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                    closed_read.store(true, Ordering::Relaxed);
                    debug!("MagicAAP reader thread stopped");
                })
                .expect("Failed to spawn magicaap reader thread");

            let handle_write = SendHandle(handle);
            let closed_write = Arc::clone(&closed);
            std::thread::Builder::new()
                .name("magicaap-writer".to_string())
                .spawn(move || {
                    while !closed_write.load(Ordering::Relaxed) {
                        let packet = match rx_outbound.blocking_recv() {
                            Some(p) => p,
                            None => break,
                        };
                        let mut bytes_written: u32 = 0;
                        let ok = unsafe {
                            WriteFile(
                                handle_write.raw(),
                                Some(&packet),
                                Some(&mut bytes_written),
                                None,
                            )
                        };
                        if ok.is_err() {
                            break;
                        }
                    }
                    closed_write.store(true, Ordering::Relaxed);
                    unsafe {
                        let mut ret: u32 = 0;
                        let _ = DeviceIoControl(
                            handle_write.raw(),
                            IOCTL_MAGICAAP_DISCONNECT,
                            None,
                            0,
                            None,
                            0,
                            Some(&mut ret),
                            None,
                        );
                        let _ = CloseHandle(handle_write.raw());
                    }
                    debug!("MagicAAP writer thread stopped");
                })
                .expect("Failed to spawn magicaap writer thread");

            Ok(Self {
                tx_outbound,
                rx_inbound: Arc::new(Mutex::new(rx_inbound)),
                closed,
            })
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = addr;
            Err("MagicAAP driver stream is only available on Windows".to_string())
        }
    }

    pub async fn send(&self, packet: &[u8]) -> Result<(), std::io::Error> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "MagicAAP stream closed",
            ));
        }
        self.tx_outbound
            .send(packet.to_vec())
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e.to_string()))
    }

    pub async fn recv(&self, buf: &mut [u8]) -> Result<usize, std::io::Error> {
        let mut rx = self.rx_inbound.lock().await;
        match rx.recv().await {
            Some(packet) => {
                let len = packet.len().min(buf.len());
                buf[..len].copy_from_slice(&packet[..len]);
                Ok(len)
            }
            None => {
                self.closed.store(true, Ordering::Relaxed);
                Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "MagicAAP connection closed",
                ))
            }
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }
}
