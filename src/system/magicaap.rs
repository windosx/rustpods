use crate::core::models::MacAddress;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
#[cfg(target_os = "windows")]
use tracing::{debug, info, warn};

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
struct SharedHandle {
    device: windows::Win32::Foundation::HANDLE,
    stop_event: windows::Win32::Foundation::HANDLE,
}

#[cfg(target_os = "windows")]
unsafe impl Send for SharedHandle {}
#[cfg(target_os = "windows")]
unsafe impl Sync for SharedHandle {}

#[cfg(target_os = "windows")]
impl Drop for SharedHandle {
    fn drop(&mut self) {
        use windows::Win32::Foundation::CloseHandle;
        unsafe {
            let _ = CloseHandle(self.stop_event);
            let _ = CloseHandle(self.device);
        }
    }
}

#[cfg(target_os = "windows")]
struct MagicAapControl {
    shared: Arc<SharedHandle>,
    closed: Arc<AtomicBool>,
}

#[cfg(target_os = "windows")]
impl Drop for MagicAapControl {
    fn drop(&mut self) {
        use windows::Win32::System::IO::CancelIoEx;
        use windows::Win32::System::Threading::SetEvent;
        self.closed.store(true, Ordering::SeqCst);
        unsafe {
            let _ = SetEvent(self.shared.stop_event);
            let _ = CancelIoEx(self.shared.device, None);
        }
    }
}

#[cfg(target_os = "windows")]
struct EventHandle(windows::Win32::Foundation::HANDLE);

#[cfg(target_os = "windows")]
impl Drop for EventHandle {
    fn drop(&mut self) {
        use windows::Win32::Foundation::CloseHandle;
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[derive(Clone)]
pub struct MagicAapStream {
    tx_outbound: mpsc::Sender<Vec<u8>>,
    rx_inbound: Arc<Mutex<mpsc::Receiver<Vec<u8>>>>,
    closed: Arc<AtomicBool>,
    #[cfg(target_os = "windows")]
    _control: Arc<MagicAapControl>,
}

#[cfg(target_os = "windows")]
fn is_io_pending(err: &windows::core::Error) -> bool {
    err.code() == windows::Win32::Foundation::ERROR_IO_PENDING.to_hresult()
}

#[cfg(target_os = "windows")]
unsafe fn overlapped_ioctl(
    device: windows::Win32::Foundation::HANDLE,
    code: u32,
    in_buf: Option<*const std::ffi::c_void>,
    in_len: u32,
) -> windows::core::Result<()> {
    use windows::Win32::System::IO::{DeviceIoControl, GetOverlappedResult, OVERLAPPED};
    use windows::Win32::System::Threading::CreateEventW;

    let ev = EventHandle(unsafe { CreateEventW(None, true, false, None)? });
    let mut ov = OVERLAPPED {
        hEvent: ev.0,
        ..Default::default()
    };
    let mut bytes_returned: u32 = 0;
    let res = unsafe {
        DeviceIoControl(
            device,
            code,
            in_buf,
            in_len,
            None,
            0,
            Some(&mut bytes_returned),
            Some(&mut ov),
        )
    };
    match res {
        Ok(()) => Ok(()),
        Err(e) if is_io_pending(&e) => unsafe {
            GetOverlappedResult(device, &ov, &mut bytes_returned, true)
        },
        Err(e) => Err(e),
    }
}

#[cfg(target_os = "windows")]
pub fn is_driver_available() -> bool {
    use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
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
                FILE_FLAG_OVERLAPPED,
                None,
            );
            if let Ok(h) = handle
                && h != INVALID_HANDLE_VALUE
                && !h.is_invalid()
            {
                let _ = CloseHandle(h);
                return true;
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
            use windows::Win32::Foundation::{
                CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
            };
            use windows::Win32::Storage::FileSystem::{
                CreateFileW, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ,
                FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
            };
            use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
            use windows::Win32::System::Threading::{
                CreateEventW, ResetEvent, SetEvent, WaitForMultipleObjects, INFINITE,
            };

            info!(
                "Attempting MagicAAP / L2CAP driver bridge connection to {}...",
                addr
            );

            let candidates = [
                (windows::core::w!(r"\\.\MagicAAP"), false),
                (windows::core::w!(r"\\.\MagicPodsL2cap"), false),
                (windows::core::w!(r"\\.\pipe\magicaap-l2cap"), true),
                (windows::core::w!(r"\\.\pipe\rustpods-l2cap"), true),
            ];

            let mut opened: Option<(HANDLE, bool)> = None;
            for (path, is_pipe) in candidates {
                unsafe {
                    let handle = CreateFileW(
                        path,
                        FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
                        FILE_SHARE_READ | FILE_SHARE_WRITE,
                        None,
                        OPEN_EXISTING,
                        FILE_FLAG_OVERLAPPED,
                        None,
                    );
                    if let Ok(h) = handle
                        && h != INVALID_HANDLE_VALUE
                        && !h.is_invalid()
                    {
                        info!("Opened MagicAAP driver/pipe interface (overlapped, pipe={})", is_pipe);
                        opened = Some((h, is_pipe));
                        break;
                    }
                }
            }

            let (handle, is_pipe) = opened.ok_or_else(|| {
                "MagicAAP kernel driver or L2CAP bridge not detected. Operating in Native Driverless Mode."
                    .to_string()
            })?;

            let stop_event = match unsafe { CreateEventW(None, true, false, None) } {
                Ok(ev) => ev,
                Err(e) => {
                    unsafe {
                        let _ = CloseHandle(handle);
                    }
                    return Err(format!("Failed to create MagicAAP stop event: {}", e));
                }
            };

            let shared = Arc::new(SharedHandle {
                device: handle,
                stop_event,
            });

            // Convert MAC address to 64-bit integer
            let b = addr.0;
            let bth_addr = u64::from_be_bytes([0, 0, b[0], b[1], b[2], b[3], b[4], b[5]]);

            let req = MagicAapConnectReq {
                bth_addr,
                psm: PSM_AACP,
                mtu: AACP_RECEIVE_MTU,
            };

            // Issue IOCTL_MAGICAAP_CONNECT to create kernel L2CAP channel.
            let ioctl_res = unsafe {
                overlapped_ioctl(
                    shared.device,
                    IOCTL_MAGICAAP_CONNECT,
                    Some(&req as *const _ as *const _),
                    std::mem::size_of::<MagicAapConnectReq>() as u32,
                )
            };

            if let Err(e) = ioctl_res {
                if !is_pipe {
                    return Err(format!("MagicAAP IOCTL_MAGICAAP_CONNECT failed: {}", e));
                }
                debug!(
                    "DeviceIoControl connect ignored in pipe bridge mode: {}",
                    e
                );
            }

            info!("MagicAAP L2CAP channel established to {}", addr);

            let (tx_outbound, mut rx_outbound) = mpsc::channel::<Vec<u8>>(128);
            let (tx_inbound, rx_inbound) = mpsc::channel::<Vec<u8>>(128);
            let closed = Arc::new(AtomicBool::new(false));
            let control = Arc::new(MagicAapControl {
                shared: Arc::clone(&shared),
                closed: Arc::clone(&closed),
            });

            let shared_read = Arc::clone(&shared);
            let closed_read = Arc::clone(&closed);
            std::thread::Builder::new()
                .name("magicaap-reader".to_string())
                .spawn(move || {
                    let read_ev = match unsafe { CreateEventW(None, true, false, None) } {
                        Ok(ev) => EventHandle(ev),
                        Err(e) => {
                            warn!("MagicAAP reader failed to create event: {}", e);
                            closed_read.store(true, Ordering::SeqCst);
                            return;
                        }
                    };
                    let mut buf = vec![0u8; AACP_RECEIVE_MTU as usize];
                    while !closed_read.load(Ordering::Acquire) {
                        unsafe {
                            let _ = ResetEvent(read_ev.0);
                        }
                        let mut ov = OVERLAPPED {
                            hEvent: read_ev.0,
                            ..Default::default()
                        };
                        let mut bytes_read: u32 = 0;
                        let res = unsafe {
                            ReadFile(
                                shared_read.device,
                                Some(buf.as_mut_slice()),
                                Some(&mut bytes_read),
                                Some(&mut ov),
                            )
                        };
                        let ok = match res {
                            Ok(()) => {
                                if bytes_read == 0 {
                                    unsafe {
                                        GetOverlappedResult(
                                            shared_read.device,
                                            &ov,
                                            &mut bytes_read,
                                            false,
                                        )
                                        .is_ok()
                                    }
                                } else {
                                    true
                                }
                            }
                            Err(e) if is_io_pending(&e) => {
                                let handles = [shared_read.stop_event, read_ev.0];
                                let wait = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
                                if wait == WAIT_OBJECT_0 {
                                    unsafe {
                                        let _ = CancelIoEx(shared_read.device, Some(&ov));
                                        let mut ignored = 0u32;
                                        let _ = GetOverlappedResult(
                                            shared_read.device,
                                            &ov,
                                            &mut ignored,
                                            true,
                                        );
                                    }
                                    break;
                                }
                                unsafe {
                                    GetOverlappedResult(
                                        shared_read.device,
                                        &ov,
                                        &mut bytes_read,
                                        false,
                                    )
                                    .is_ok()
                                }
                            }
                            Err(e) => {
                                debug!("MagicAAP ReadFile error: {}", e);
                                false
                            }
                        };
                        if ok && bytes_read > 0 {
                            let packet = buf[..bytes_read as usize].to_vec();
                            if tx_inbound.blocking_send(packet).is_err() {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                    closed_read.store(true, Ordering::SeqCst);
                    unsafe {
                        let _ = SetEvent(shared_read.stop_event);
                    }
                    debug!("MagicAAP reader thread stopped");
                })
                .expect("Failed to spawn magicaap reader thread");

            let shared_write = Arc::clone(&shared);
            let closed_write = Arc::clone(&closed);
            std::thread::Builder::new()
                .name("magicaap-writer".to_string())
                .spawn(move || {
                    let write_ev = match unsafe { CreateEventW(None, true, false, None) } {
                        Ok(ev) => Some(EventHandle(ev)),
                        Err(e) => {
                            warn!("MagicAAP writer failed to create event: {}", e);
                            None
                        }
                    };
                    if let Some(write_ev) = write_ev {
                        while !closed_write.load(Ordering::Acquire) {
                            let packet = match rx_outbound.blocking_recv() {
                                Some(p) => p,
                                None => break,
                            };
                            unsafe {
                                let _ = ResetEvent(write_ev.0);
                            }
                            let mut ov = OVERLAPPED {
                                hEvent: write_ev.0,
                                ..Default::default()
                            };
                            let mut bytes_written: u32 = 0;
                            let res = unsafe {
                                WriteFile(
                                    shared_write.device,
                                    Some(&packet),
                                    Some(&mut bytes_written),
                                    Some(&mut ov),
                                )
                            };
                            let ok = match res {
                                Ok(()) => true,
                                Err(e) if is_io_pending(&e) => {
                                    let handles = [shared_write.stop_event, write_ev.0];
                                    let wait =
                                        unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
                                    if wait == WAIT_OBJECT_0 {
                                        unsafe {
                                            let _ = CancelIoEx(shared_write.device, Some(&ov));
                                            let mut ignored = 0u32;
                                            let _ = GetOverlappedResult(
                                                shared_write.device,
                                                &ov,
                                                &mut ignored,
                                                true,
                                            );
                                        }
                                        break;
                                    }
                                    unsafe {
                                        GetOverlappedResult(
                                            shared_write.device,
                                            &ov,
                                            &mut bytes_written,
                                            false,
                                        )
                                        .is_ok()
                                    }
                                }
                                Err(e) => {
                                    debug!("MagicAAP WriteFile error: {}", e);
                                    false
                                }
                            };
                            if !ok {
                                break;
                            }
                        }
                    }
                    closed_write.store(true, Ordering::SeqCst);
                    unsafe {
                        let _ = SetEvent(shared_write.stop_event);
                        if !is_pipe {
                            let _ = overlapped_ioctl(
                                shared_write.device,
                                IOCTL_MAGICAAP_DISCONNECT,
                                None,
                                0,
                            );
                        }
                    }
                    debug!("MagicAAP writer thread stopped");
                })
                .expect("Failed to spawn magicaap writer thread");

            Ok(Self {
                tx_outbound,
                rx_inbound: Arc::new(Mutex::new(rx_inbound)),
                closed,
                _control: control,
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
