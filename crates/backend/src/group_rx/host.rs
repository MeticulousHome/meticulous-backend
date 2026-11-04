//! Everything the monitor reaches outside the process, in one trait the
//! tests replace: the clocks, files, commands, the group-frame sniffer and
//! the DHCP exchange.

use std::io::Read;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use socket2::{Domain, SockAddr, SockAddrStorage, SockFilter, Socket, Type};

use super::probe::{self, Bpf, Delivery};

/// Broadcast and multicast frames from other stations.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GroupFrames {
    pub broadcast: u64,
    pub multicast: u64,
}

pub trait GroupRxHost: Send + Sync {
    /// Seconds on `CLOCK_BOOTTIME`: steady across NTP steps and backend
    /// restarts, reset by a reboot.
    fn boot_clock(&self) -> f64;
    /// `time.time()`.
    fn wall_clock(&self) -> f64;
    fn sleep(&self, seconds: f64);
    fn read_file(&self, path: &str) -> Option<String>;
    /// The file name of a symlink's target (sysfs driver links).
    fn read_link(&self, path: &str) -> Option<String>;
    /// The output of a command that exited with 0.
    fn run(&self, argv: &[&str], timeout: f64) -> Option<String>;
    /// Start counting inbound group frames on `iface`; idempotent.
    fn start_sniffer(&self, iface: &str);
    fn stop_sniffer(&self);
    /// What the sniffer counted since the last call; `None` when it was not
    /// counting the whole time (not started, socket error, interface gone).
    fn take_frames(&self) -> Option<GroupFrames>;
    fn ipv4(&self, iface: &str) -> Option<Ipv4Addr>;
    /// Send `frame` on `iface`, then collect for `wait` seconds the DHCP
    /// replies to `xid`.
    fn exchange(
        &self,
        iface: &str,
        frame: &[u8],
        xid: u32,
        wait: f64,
    ) -> Result<Vec<Delivery>, String>;
    fn random_u32(&self) -> u32;
}

const ETH_P_ALL: u16 = 0x0003;
const ETH_P_IP: u16 = 0x0800;
/// How often the sniffer looks at its stop flag.
const READ_TIMEOUT: Duration = Duration::from_secs(1);
const REOPEN_AFTER: Duration = Duration::from_secs(5);

/// The machine.
#[derive(Default)]
pub struct SystemGroupRxHost {
    sniffer: Mutex<Option<Sniffer>>,
}

struct Sniffer {
    iface: String,
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    counting: AtomicBool,
    /// The socket failed or reopened since the last take.
    interrupted: AtomicBool,
    broadcast: AtomicU64,
    multicast: AtomicU64,
}

impl GroupRxHost for SystemGroupRxHost {
    fn boot_clock(&self) -> f64 {
        nix::time::clock_gettime(nix::time::ClockId::CLOCK_BOOTTIME)
            .map(|t| t.tv_sec() as f64 + t.tv_nsec() as f64 / 1e9)
            .unwrap_or(0.0)
    }

    fn wall_clock(&self) -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }

    fn sleep(&self, seconds: f64) {
        if seconds > 0.0 {
            std::thread::sleep(Duration::from_secs_f64(seconds));
        }
    }

    fn read_file(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn read_link(&self, path: &str) -> Option<String> {
        let target = std::fs::read_link(path).ok()?;
        Some(target.file_name()?.to_string_lossy().into_owned())
    }

    fn run(&self, argv: &[&str], timeout: f64) -> Option<String> {
        let argv: Vec<String> = argv.iter().map(|a| a.to_string()).collect();
        match crate::py::run_command(&argv, Some(Duration::from_secs_f64(timeout)), &[]) {
            Ok(done) if done.returncode == 0 => Some(done.stdout),
            _ => None,
        }
    }

    fn start_sniffer(&self, iface: &str) {
        let mut sniffer = self.sniffer.lock();
        if sniffer.as_ref().is_some_and(|s| s.iface == iface) {
            return;
        }
        if let Some(old) = sniffer.take() {
            old.shared.stop.store(true, Ordering::Relaxed);
        }
        let shared = Arc::new(Shared::default());
        let thread_shared = shared.clone();
        let thread_iface = iface.to_string();
        let spawned = std::thread::Builder::new()
            .name("WifiGroupSniff".into())
            .spawn(move || sniff(&thread_iface, &thread_shared));
        match spawned {
            Ok(_) => {
                *sniffer = Some(Sniffer {
                    iface: iface.to_string(),
                    shared,
                })
            }
            Err(error) => tracing::warn!("Could not start the Wi-Fi group frame counter: {error}"),
        }
    }

    fn stop_sniffer(&self) {
        if let Some(sniffer) = self.sniffer.lock().take() {
            sniffer.shared.stop.store(true, Ordering::Relaxed);
        }
    }

    fn take_frames(&self) -> Option<GroupFrames> {
        let sniffer = self.sniffer.lock();
        let shared = &sniffer.as_ref()?.shared;
        let frames = GroupFrames {
            broadcast: shared.broadcast.swap(0, Ordering::Relaxed),
            multicast: shared.multicast.swap(0, Ordering::Relaxed),
        };
        let interrupted = shared.interrupted.swap(false, Ordering::Relaxed);
        (shared.counting.load(Ordering::Relaxed) && !interrupted).then_some(frames)
    }

    fn ipv4(&self, iface: &str) -> Option<Ipv4Addr> {
        nix::ifaddrs::getifaddrs().ok()?.find_map(|entry| {
            if entry.interface_name != iface {
                return None;
            }
            Some(entry.address?.as_sockaddr_in()?.ip())
        })
    }

    fn exchange(
        &self,
        iface: &str,
        frame: &[u8],
        xid: u32,
        wait: f64,
    ) -> Result<Vec<Delivery>, String> {
        let socket = packet_socket(iface, ETH_P_IP, &probe::DHCP_CLIENT_FILTER)
            .map_err(|e| e.to_string())?;
        socket
            .set_read_timeout(Some(Duration::from_millis(200)))
            .map_err(|e| e.to_string())?;
        socket.send(frame).map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_secs_f64(wait);
        let mut buffer = [0u8; 1600];
        let mut replies = Vec::new();
        while Instant::now() < deadline {
            match (&socket).read(&mut buffer) {
                Ok(len) => replies.extend(probe::reply_delivery(&buffer[..len], xid)),
                Err(error) if is_timeout(&error) => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(replies)
    }

    fn random_u32(&self) -> u32 {
        rand::random()
    }
}

fn is_timeout(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::Interrupted
    )
}

/// The sniffer thread: count until stopped, reopening the socket when the
/// interface goes away (a driver reload gives it a new index).
fn sniff(iface: &str, shared: &Shared) {
    let mut warned = false;
    while !shared.stop.load(Ordering::Relaxed) {
        match open_sniffer(iface) {
            Ok((socket, own)) => {
                warned = false;
                shared.counting.store(true, Ordering::Relaxed);
                if let Err(error) = count(&socket, own, shared) {
                    tracing::info!("Wi-Fi group frame counter interrupted: {error}");
                }
            }
            Err(error) if !warned => {
                warned = true;
                tracing::warn!("Could not open the Wi-Fi group frame counter: {error}");
            }
            Err(_) => {}
        }
        shared.counting.store(false, Ordering::Relaxed);
        shared.interrupted.store(true, Ordering::Relaxed);
        let reopen = Instant::now() + REOPEN_AFTER;
        while Instant::now() < reopen && !shared.stop.load(Ordering::Relaxed) {
            std::thread::sleep(READ_TIMEOUT);
        }
    }
}

fn open_sniffer(iface: &str) -> std::io::Result<(Socket, [u8; 6])> {
    let own = std::fs::read_to_string(format!("/sys/class/net/{iface}/address"))?;
    let own = hex::decode(own.trim().replace(':', ""))
        .ok()
        .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())
        .ok_or_else(|| std::io::Error::other("unreadable MAC address"))?;
    let socket = packet_socket(iface, ETH_P_ALL, &probe::GROUP_FILTER)?;
    socket.set_read_timeout(Some(READ_TIMEOUT))?;
    Ok((socket, own))
}

fn count(socket: &Socket, own: [u8; 6], shared: &Shared) -> std::io::Result<()> {
    let mut frame = [0u8; 64];
    let mut reader = socket;
    while !shared.stop.load(Ordering::Relaxed) {
        match reader.read(&mut frame) {
            // The firmware drops our own frames relayed by the AP; skip them
            // anyway, they say nothing about the group key.
            Ok(len) if len >= 12 && frame[6..12] != own => {
                if frame[..6] == [0xff; 6] {
                    shared.broadcast.fetch_add(1, Ordering::Relaxed);
                } else {
                    shared.multicast.fetch_add(1, Ordering::Relaxed);
                }
            }
            Ok(_) => {}
            Err(error) if is_timeout(&error) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// A raw packet socket on `iface` that receives `protocol` frames passing
/// `filter`. It is opened with protocol 0, which receives nothing, and the
/// filter goes on before the bind, so nothing unfiltered is ever queued.
/// The interface is not put in promiscuous mode.
fn packet_socket(iface: &str, protocol: u16, filter: &[Bpf]) -> std::io::Result<Socket> {
    let ifindex: i32 = std::fs::read_to_string(format!("/sys/class/net/{iface}/ifindex"))?
        .trim()
        .parse()
        .map_err(|_| std::io::Error::other("unreadable interface index"))?;
    let socket = Socket::new(Domain::PACKET, Type::RAW, None)?;
    let program: Vec<SockFilter> = filter
        .iter()
        .map(|&(code, jt, jf, k)| SockFilter::new(code, jt, jf, k))
        .collect();
    socket.attach_filter(&program)?;
    socket.bind(&link_address(ifindex, protocol))?;
    Ok(socket)
}

/// A `sockaddr_ll` for `bind`: socket2 has no safe constructor for one.
#[allow(unsafe_code)]
fn link_address(ifindex: i32, protocol: u16) -> SockAddr {
    let mut storage = SockAddrStorage::zeroed();
    // SAFETY: a zeroed sockaddr_storage is larger than, and aligned for, a
    // sockaddr_ll, and every bit pattern is a valid sockaddr_ll.
    let address = unsafe { storage.view_as::<nix::libc::sockaddr_ll>() };
    address.sll_family = nix::libc::AF_PACKET as u16;
    address.sll_protocol = protocol.to_be();
    address.sll_ifindex = ifindex;
    let len = std::mem::size_of::<nix::libc::sockaddr_ll>() as nix::libc::socklen_t;
    // SAFETY: the storage holds an initialised sockaddr_ll of `len` bytes.
    unsafe { SockAddr::new(storage, len) }
}
