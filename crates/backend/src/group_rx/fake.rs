//! A scripted host for the monitor's tests: an IW612 machine associated to
//! one AP, whose clock, sniffer counts, journal and DHCP answers the test
//! sets window by window.

use std::collections::HashMap;
use std::net::Ipv4Addr;

use parking_lot::Mutex;

use super::host::{GroupFrames, GroupRxHost};
use super::probe::Delivery;
use crate::sentry_reporting::{EventReport, EventSink};

pub const BSSID: &str = "82:83:c2:9e:22:71";
/// Wall clock minus boot clock: 2026-11-03.
pub const WALL_OFFSET: f64 = 1_793_700_000.0;
pub const BOOT_ID: &str = "8979104b-677d-41c4-a6e0-cd757c52c643";

/// `iw dev wlan0 link` on 10.10.0.84, 2026-11-03.
pub fn iw_link(bssid: &str) -> String {
    format!(
        "Connected to {bssid} (on wlan0)\n\tSSID: HomeNet\n\tfreq: 5180\n\tsignal: -58 dBm\n\
         \n\tbss flags:\tshort-slot-time\n\tdtim period:\t3\n\tbeacon int:\t100\n"
    )
}

#[derive(Debug, Default)]
pub struct State {
    pub boot: f64,
    pub wall_offset: f64,
    pub files: HashMap<String, String>,
    pub links: HashMap<String, String>,
    /// Command line (space-joined) to its output; missing ones fail.
    pub commands: HashMap<String, String>,
    /// wpa_supplicant's journal, `short-unix` lines.
    pub journal: Vec<String>,
    /// What `journalctl` returns for the attached log.
    pub wifi_log: String,
    pub sniffing: bool,
    pub frames: Option<GroupFrames>,
    pub rx_packets: u64,
    pub ipv4: Option<Ipv4Addr>,
    pub flagged_answers: Vec<Delivery>,
    pub plain_answers: Vec<Delivery>,
    /// The broadcast flag of every INFORM sent.
    pub informs: Vec<bool>,
    pub send_fails: bool,
    pub xid: u32,
}

pub struct FakeHost {
    pub state: Mutex<State>,
}

impl FakeHost {
    pub fn iw612() -> FakeHost {
        let mut state = State {
            boot: 1000.0,
            wall_offset: WALL_OFFSET,
            ipv4: Some(Ipv4Addr::new(10, 10, 0, 84)),
            flagged_answers: vec![Delivery::Broadcast],
            plain_answers: vec![Delivery::Unicast],
            ..State::default()
        };
        let files = [
            ("/sys/class/net/wlan0/address", "34:90:ea:ac:2c:1c\n"),
            ("/sys/class/net/wlan0/statistics/rx_packets", "0\n"),
            (
                "/proc/sys/kernel/random/boot_id",
                "8979104b-677d-41c4-a6e0-cd757c52c643\n",
            ),
            (
                "/proc/mwlan/adapter0/wlan0/info",
                "driver_name = \"wlan\"\n\
                 driver_version = SDIW612---18.99.3.p21.10-MM6X18505.p14-GPL-(FP92) \n\
                 essid=\"HomeNet\"\n",
            ),
            ("/sys/module/moal/parameters/ps_mode", "1\n"),
            ("/sys/module/moal/parameters/auto_ds", "1\n"),
        ];
        for (path, text) in files {
            state.files.insert(path.into(), text.into());
        }
        state.links.insert(
            "/sys/class/net/wlan0/device/driver/module".into(),
            "moal".into(),
        );
        state.links.insert(
            "/sys/class/net/wlan0/device/driver".into(),
            "wlan_sdio".into(),
        );
        let commands = [
            ("iw dev wlan0 link".to_string(), iw_link(BSSID)),
            ("iw dev wlan0 get power_save".into(), "Power save: on\n".into()),
            (
                "wpa_cli -i wlan0 status".into(),
                "bssid=82:83:c2:9e:22:71\nssid=HomeNet\npairwise_cipher=CCMP\n\
                 group_cipher=CCMP\nkey_mgmt=WPA2-PSK\nwpa_state=COMPLETED\n"
                    .into(),
            ),
            (
                format!("wpa_cli -i wlan0 bss {BSSID}"),
                "bssid=82:83:c2:9e:22:71\nssid=HomeNet\n\
                 ie=0007486f6d654e657430140100000fac040100000fac040100000fac020000dd0700156d00010100\n"
                    .into(),
            ),
            (
                "mlanutl wlan0 getlog".into(),
                "dot11GroupReceivedFrameCount 100\nbeaconMissedCount 2\n".into(),
            ),
        ];
        state.commands.extend(commands);
        state.wifi_log = format!(
            "{t:.6} machine wpa_supplicant[297]: wlan0: SME: Trying to authenticate with {BSSID} \
             (SSID='HomeNet' freq=5180 MHz)\n\
             {t:.6} machine kernel: wlan: Received disassociation request on wlan0\n\
             {t:.6} machine kernel: usb 1-1: unrelated\n",
            t = WALL_OFFSET + 900.0
        );
        FakeHost {
            state: Mutex::new(state),
        }
    }

    pub fn now(&self) -> f64 {
        self.state.lock().boot
    }

    /// One window passes with these frames counted.
    pub fn window(&self, broadcast: u64, multicast: u64, unicast: u64) {
        let mut state = self.state.lock();
        state.boot += super::WINDOW;
        state.frames = state.sniffing.then_some(GroupFrames {
            broadcast,
            multicast,
        });
        state.rx_packets += broadcast + multicast + unicast;
        let rx = state.rx_packets.to_string();
        state
            .files
            .insert("/sys/class/net/wlan0/statistics/rx_packets".into(), rx);
    }

    /// wpa_supplicant logs a group rekey now.
    pub fn rekey(&self) {
        self.log("WPA: Group rekeying completed with 82:83:c2:9e:22:71 [GTK=CCMP]");
    }

    pub fn log(&self, message: &str) {
        let mut state = self.state.lock();
        let line = format!(
            "{:.6} meticulousMachine-000000 wpa_supplicant[297]: wlan0: {message}",
            state.boot + state.wall_offset
        );
        state.journal.push(line);
    }

    pub fn associate(&self, bssid: Option<&str>) {
        let output = bssid.map_or_else(|| "Not connected.\n".to_string(), iw_link);
        self.state
            .lock()
            .commands
            .insert("iw dev wlan0 link".into(), output);
    }

    pub fn answers(&self, flagged: Vec<Delivery>, plain: Vec<Delivery>) {
        let mut state = self.state.lock();
        state.flagged_answers = flagged;
        state.plain_answers = plain;
    }

    pub fn informs(&self) -> Vec<bool> {
        self.state.lock().informs.clone()
    }
}

impl GroupRxHost for FakeHost {
    fn boot_clock(&self) -> f64 {
        self.state.lock().boot
    }

    fn wall_clock(&self) -> f64 {
        let state = self.state.lock();
        state.boot + state.wall_offset
    }

    fn sleep(&self, seconds: f64) {
        self.state.lock().boot += seconds.max(0.0);
    }

    fn read_file(&self, path: &str) -> Option<String> {
        self.state.lock().files.get(path).cloned()
    }

    fn read_link(&self, path: &str) -> Option<String> {
        self.state.lock().links.get(path).cloned()
    }

    fn run(&self, argv: &[&str], _timeout: f64) -> Option<String> {
        let state = self.state.lock();
        if argv.first() == Some(&"journalctl") {
            if argv.contains(&"_TRANSPORT=kernel") {
                return Some(state.wifi_log.clone());
            }
            return Some(state.journal.join("\n"));
        }
        state.commands.get(&argv.join(" ")).cloned()
    }

    fn start_sniffer(&self, _iface: &str) {
        self.state.lock().sniffing = true;
    }

    fn stop_sniffer(&self) {
        self.state.lock().sniffing = false;
    }

    fn take_frames(&self) -> Option<GroupFrames> {
        self.state.lock().frames.take()
    }

    fn ipv4(&self, _iface: &str) -> Option<Ipv4Addr> {
        self.state.lock().ipv4
    }

    fn exchange(
        &self,
        _iface: &str,
        frame: &[u8],
        xid: u32,
        _wait: f64,
    ) -> Result<Vec<Delivery>, String> {
        let mut state = self.state.lock();
        if state.send_fails {
            return Err("Network is down".into());
        }
        // The BOOTP flags field of the frame the monitor built.
        let broadcast = frame[14 + 20 + 8 + 10] & 0x80 != 0;
        assert_eq!(frame[14 + 20 + 8 + 4..14 + 20 + 8 + 8], xid.to_be_bytes());
        state.informs.push(broadcast);
        Ok(if broadcast {
            state.flagged_answers.clone()
        } else {
            state.plain_answers.clone()
        })
    }

    fn random_u32(&self) -> u32 {
        let mut state = self.state.lock();
        state.xid += 1;
        state.xid
    }
}

/// The events a test would have sent to Sentry.
#[derive(Default)]
pub struct RecordingSink {
    pub events: Mutex<Vec<EventReport>>,
}

impl EventSink for RecordingSink {
    fn send(&self, report: EventReport) {
        self.events.lock().push(report);
    }
}
