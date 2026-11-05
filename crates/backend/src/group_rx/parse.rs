//! Parsers for what the monitor reads off the host: `iw`, `wpa_cli`,
//! `ethtool`, the NXP driver's `/proc/mwlan` and `mlanutl`, and journal
//! lines. None of them keeps a network name: the lines and information
//! elements that carry one are skipped.

use std::collections::BTreeMap;

/// `iw dev wlan0 link` while associated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IwLink {
    pub bssid: String,
    pub freq: Option<u32>,
    pub signal: Option<i32>,
    pub dtim_period: Option<u32>,
    pub beacon_interval: Option<u32>,
}

/// `None` when not associated ("Not connected.").
pub fn iw_link(text: &str) -> Option<IwLink> {
    let mut lines = text.lines();
    let bssid = lines
        .next()?
        .trim()
        .strip_prefix("Connected to ")?
        .split_whitespace()
        .next()?
        .to_ascii_lowercase();
    if !is_mac(&bssid) {
        return None;
    }
    let mut link = IwLink {
        bssid,
        ..IwLink::default()
    };
    for line in lines {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "freq" => link.freq = leading_number(value),
            "signal" => link.signal = value.split_whitespace().next().and_then(|v| v.parse().ok()),
            "dtim period" => link.dtim_period = leading_number(value),
            "beacon int" => link.beacon_interval = leading_number(value),
            _ => {}
        }
    }
    Some(link)
}

/// `iw` prints "5180" or "5180.0" depending on its version.
fn leading_number(value: &str) -> Option<u32> {
    let number = value.split_whitespace().next()?;
    number
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && *n >= 0.0)
        .map(|n| n as u32)
}

fn is_mac(value: &str) -> bool {
    value.len() == 17
        && value
            .split(':')
            .all(|part| part.len() == 2 && part.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Band and channel of a frequency in MHz.
pub fn channel(freq: u32) -> Option<(&'static str, u32)> {
    match freq {
        2412..=2472 => Some(("2.4GHz", (freq - 2407) / 5)),
        2484 => Some(("2.4GHz", 14)),
        5150..=5895 => Some(("5GHz", (freq - 5000) / 5)),
        5955..=7115 => Some(("6GHz", (freq - 5950) / 5)),
        _ => None,
    }
}

/// The first three octets of a BSSID, and whether the address is locally
/// administered (an AP's extra BSSIDs often are, and then name no vendor).
pub fn oui(bssid: &str) -> Option<(String, bool)> {
    if !is_mac(bssid) {
        return None;
    }
    let first = u8::from_str_radix(&bssid[..2], 16).ok()?;
    Some((bssid[..8].to_ascii_lowercase(), first & 0x02 != 0))
}

/// `iw dev wlan0 get power_save`: "on" or "off".
pub fn power_save(text: &str) -> Option<String> {
    let value = text.trim().strip_prefix("Power save:")?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// The `key=value` lines of `wpa_cli status`, `ethtool -i` (`key: value`)
/// or `/proc/mwlan/*/info`, restricted to `keys`.
pub fn fields(text: &str, separator: char, keys: &[&str]) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once(separator))
        .map(|(key, value)| (key.trim(), value.trim().trim_matches('"')))
        .filter(|(key, value)| keys.contains(key) && !value.is_empty())
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// What the AP's information elements (`wpa_cli bss <bssid>`, `ie=`) say
/// about its security and make.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BssElements {
    /// Management frame protection the AP offers: "required", "capable" or
    /// "off"; `None` without an RSN element.
    pub pmf: Option<String>,
    pub akms: Vec<String>,
    /// OUIs of its vendor-specific elements, which name chipset and AP
    /// vendors (Microsoft's WMM/WPS one aside).
    pub vendor_ouis: Vec<String>,
}

pub fn bss_elements(text: &str) -> Option<BssElements> {
    let hex = text.lines().find_map(|line| line.strip_prefix("ie="))?;
    let bytes = hex::decode(hex.trim()).ok()?;
    let mut elements = BssElements::default();
    let mut rest = bytes.as_slice();
    while let [id, len, tail @ ..] = rest {
        let len = usize::from(*len);
        if tail.len() < len {
            break;
        }
        let (body, next) = tail.split_at(len);
        match id {
            48 => rsn(body, &mut elements),
            221 if body.len() >= 3 => {
                let oui = format!("{:02x}:{:02x}:{:02x}", body[0], body[1], body[2]);
                if oui != "00:50:f2" && !elements.vendor_ouis.contains(&oui) {
                    elements.vendor_ouis.push(oui);
                }
            }
            _ => {}
        }
        rest = next;
    }
    Some(elements)
}

/// The RSN element (IEEE 802.11-2020 9.4.2.24): version, group cipher,
/// pairwise ciphers, AKM suites, then the capabilities with MFPR (bit 6)
/// and MFPC (bit 7).
fn rsn(body: &[u8], elements: &mut BssElements) {
    let count_at = |at: usize| -> Option<usize> {
        Some(usize::from(u16::from_le_bytes([
            *body.get(at)?,
            *body.get(at + 1)?,
        ])))
    };
    let Some(pairwise) = count_at(6) else { return };
    let akm_at = 8 + 4 * pairwise;
    let Some(akms) = count_at(akm_at) else { return };
    for i in 0..akms {
        let at = akm_at + 2 + 4 * i;
        if let Some(suite) = body.get(at..at + 4) {
            elements.akms.push(akm_name(suite));
        }
    }
    let capabilities = count_at(akm_at + 2 + 4 * akms).unwrap_or(0);
    elements.pmf = Some(
        if capabilities & 0x40 != 0 {
            "required"
        } else if capabilities & 0x80 != 0 {
            "capable"
        } else {
            "off"
        }
        .to_string(),
    );
}

fn akm_name(suite: &[u8]) -> String {
    if suite[..3] != [0x00, 0x0f, 0xac] {
        return format!(
            "{:02x}:{:02x}:{:02x}:{}",
            suite[0], suite[1], suite[2], suite[3]
        );
    }
    match suite[3] {
        1 => "802.1x".into(),
        2 => "psk".into(),
        3 => "ft-802.1x".into(),
        4 => "ft-psk".into(),
        5 => "802.1x-sha256".into(),
        6 => "psk-sha256".into(),
        8 => "sae".into(),
        9 => "ft-sae".into(),
        18 => "owe".into(),
        24 => "sae-ext-key".into(),
        other => format!("akm-{other}"),
    }
}

/// The NXP driver's `driver_version` line,
/// `SDIW612---18.99.3.p21.10-MM6X18505.p14-GPL-(FP92)`: chip, firmware and
/// driver release.
pub fn mwlan_version(text: &str) -> Option<(String, String, String)> {
    let version = fields(text, '=', &["driver_version"]).remove("driver_version")?;
    let parts: Vec<&str> = version.split('-').filter(|p| !p.is_empty()).collect();
    let firmware_at = parts
        .iter()
        .position(|p| p.starts_with(|c: char| c.is_ascii_digit()))?;
    Some((
        parts.first()?.to_string(),
        parts[firmware_at].to_string(),
        parts.get(firmware_at + 1)?.to_string(),
    ))
}

/// `mlanutl wlan0 getlog`: the named counters.
pub fn getlog(text: &str) -> BTreeMap<String, u64> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let name = parts.next()?;
            let value = parts.next()?.parse().ok()?;
            parts.next().is_none().then(|| (name.to_string(), value))
        })
        .collect()
}

/// One `journalctl -o short-unix` line, without the hostname.
#[derive(Debug, Clone, PartialEq)]
pub struct JournalLine {
    pub time: f64,
    /// `wpa_supplicant`, `kernel`, ...
    pub identifier: String,
    pub message: String,
}

pub fn journal_line(line: &str) -> Option<JournalLine> {
    let (time, rest) = line.split_once(' ')?;
    let time: f64 = time.parse().ok()?;
    let (_host, rest) = rest.split_once(' ')?;
    let (identifier, message) = rest.split_once(": ")?;
    let identifier = identifier.split('[').next().unwrap_or(identifier);
    Some(JournalLine {
        time,
        identifier: identifier.to_string(),
        message: message.to_string(),
    })
}

/// The wpa_supplicant events the monitor counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WpaEvent {
    /// "WPA: Group rekeying completed": a new group key was installed.
    Rekey,
    /// "CTRL-EVENT-CONNECTED": an association (and its 4-way handshake).
    Connected,
}

pub fn wpa_event(message: &str, iface: &str) -> Option<WpaEvent> {
    let message = message.strip_prefix(iface)?.strip_prefix(": ")?;
    if message.starts_with("WPA: Group rekeying completed") {
        Some(WpaEvent::Rekey)
    } else if message.starts_with("CTRL-EVENT-CONNECTED") {
        Some(WpaEvent::Connected)
    } else {
        None
    }
}

/// Kernel lines that come from the Wi-Fi drivers and stack.
pub fn is_wifi_kernel_line(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "mlan",
        "moal",
        "woal",
        "wlan",
        "brcmf",
        "sdio",
        "cfg80211",
        "ieee80211",
    ]
    .iter()
    .any(|word| lower.contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `iw dev wlan0 link` on 10.10.0.84 (IW612), 2026-11-03.
    const IW_LINK: &str = "Connected to 82:83:C2:9E:22:71 (on wlan0)
\tSSID: Some Network: with colons
\tfreq: 5180
\tRX: 504812060 bytes (1266192 packets)
\tTX: 33957514 bytes (322108 packets)
\tsignal: -58 dBm
\trx bitrate: 200.0 MBit/s VHT-MCS 9 40MHz short GI VHT-NSS 1

\tbss flags:\tshort-slot-time
\tdtim period:\t3
\tbeacon int:\t100
";

    #[test]
    fn iw_link_while_associated_and_not() {
        assert_eq!(
            iw_link(IW_LINK),
            Some(IwLink {
                bssid: "82:83:c2:9e:22:71".into(),
                freq: Some(5180),
                signal: Some(-58),
                dtim_period: Some(3),
                beacon_interval: Some(100),
            })
        );
        assert_eq!(iw_link("Not connected.\n"), None);
        assert_eq!(iw_link(""), None);
        assert_eq!(
            iw_link("Connected to 2412.0\n").map(|l| l.bssid),
            None,
            "not an address"
        );
        assert_eq!(
            iw_link("Connected to 00:11:22:33:44:55 (on wlan0)\n\tfreq: 2437.0\n")
                .and_then(|l| l.freq),
            Some(2437)
        );
    }

    #[test]
    fn channels_ouis_and_power_save() {
        assert_eq!(channel(2412), Some(("2.4GHz", 1)));
        assert_eq!(channel(2484), Some(("2.4GHz", 14)));
        assert_eq!(channel(5180), Some(("5GHz", 36)));
        assert_eq!(channel(5955), Some(("6GHz", 1)));
        assert_eq!(channel(900), None);
        assert_eq!(oui("82:83:c2:9e:22:71"), Some(("82:83:c2".into(), true)));
        assert_eq!(oui("d0:21:f9:ed:2a:8a"), Some(("d0:21:f9".into(), false)));
        assert_eq!(oui("nope"), None);
        assert_eq!(power_save("Power save: off\n"), Some("off".into()));
        assert_eq!(power_save("command failed: Operation not supported"), None);
    }

    /// `wpa_cli -i wlan0 status` and `ethtool -i wlan0` on the office
    /// machines: only the asked keys come back, never `ssid`.
    #[test]
    fn key_value_fields() {
        let status = "bssid=82:83:c2:9e:22:71\nfreq=5180\nssid=HomeNet\nid=0\n\
                      pairwise_cipher=CCMP\ngroup_cipher=CCMP\nkey_mgmt=WPA2-PSK\nwpa_state=COMPLETED\n";
        let wanted = ["key_mgmt", "group_cipher", "pairwise_cipher", "pmf"];
        let got = fields(status, '=', &wanted);
        assert_eq!(got.get("key_mgmt").map(String::as_str), Some("WPA2-PSK"));
        assert_eq!(got.len(), 3);
        let ethtool = "driver: brcmfmac\nversion: 6.37.39.141\nfirmware-version: \n";
        let got = fields(ethtool, ':', &["driver", "version", "firmware-version"]);
        assert_eq!(got.get("version").map(String::as_str), Some("6.37.39.141"));
        assert!(
            !got.contains_key("firmware-version"),
            "empty values are left out"
        );
    }

    /// The `ie=` of `wpa_cli bss` for the office test AP (WPA2-PSK, no PMF):
    /// an SSID element, then an RSN element with CCMP and PSK, then a
    /// vendor element; and a WPA3 one with PMF required.
    #[test]
    fn information_elements() {
        let ie = "000b465545474f2d72656b6579\
                  30140100000fac040100000fac040100000fac020000\
                  dd0700156d00010100\
                  dd0700156d00010200\
                  dd090050f204104a000110";
        let text = format!("bssid=82:83:c2:9e:22:71\nssid=FUEGO-rekey\nie={ie}\n");
        assert_eq!(
            bss_elements(&text),
            Some(BssElements {
                pmf: Some("off".into()),
                akms: vec!["psk".into()],
                vendor_ouis: vec!["00:15:6d".into()],
            })
        );
        let wpa3 = "ie=30140100000fac040100000fac040100000fac08c000\n";
        let elements = bss_elements(wpa3).expect("elements");
        assert_eq!(elements.pmf.as_deref(), Some("required"));
        assert_eq!(elements.akms, vec!["sae".to_string()]);
        let transition = "ie=30180100000fac040100000fac040200000fac02000fac088000\n";
        let elements = bss_elements(transition).expect("elements");
        assert_eq!(elements.pmf.as_deref(), Some("capable"));
        assert_eq!(elements.akms, vec!["psk".to_string(), "sae".to_string()]);
        assert_eq!(bss_elements("ie=zz\n"), None);
        assert_eq!(bss_elements("bssid=x\n"), None);
        assert_eq!(
            bss_elements("ie=3004010000\n").and_then(|e| e.pmf),
            None,
            "a truncated RSN element"
        );
    }

    /// `/proc/mwlan/adapter0/wlan0/info` and `mlanutl wlan0 getlog` on
    /// 10.10.0.84.
    #[test]
    fn nxp_driver_files() {
        let info = "driver_name = \"wlan\"\n\
                    driver_version = SDIW612---18.99.3.p21.10-MM6X18505.p14-GPL-(FP92) \n\
                    interface_name=\"wlan0\"\nessid=\"HomeNet\"\n";
        assert_eq!(
            mwlan_version(info),
            Some((
                "SDIW612".into(),
                "18.99.3.p21.10".into(),
                "MM6X18505.p14".into()
            ))
        );
        assert_eq!(mwlan_version("driver_name = \"wlan\"\n"), None);
        let log = "Get log: timestamp 1793768495.148237 sec\n\
                   dot11GroupReceivedFrameCount       11312\n\
                   beaconMissedCount                  0\n";
        let counters = getlog(log);
        assert_eq!(counters.get("dot11GroupReceivedFrameCount"), Some(&11312));
        assert_eq!(counters.get("beaconMissedCount"), Some(&0));
        assert_eq!(counters.len(), 2);
    }

    #[test]
    fn journal_lines_and_events() {
        let line = "1793768179.874525 meticulousCanonizedAffogato-000000 wpa_supplicant[297]: \
                    wlan0: WPA: Group rekeying completed with 82:83:c2:9e:22:71 [GTK=CCMP]";
        let parsed = journal_line(line).expect("line");
        assert_eq!(parsed.time, 1_793_768_179.874_525);
        assert_eq!(parsed.identifier, "wpa_supplicant");
        assert!(
            !parsed.message.contains("meticulous"),
            "the hostname is dropped"
        );
        assert_eq!(wpa_event(&parsed.message, "wlan0"), Some(WpaEvent::Rekey));
        assert_eq!(
            wpa_event(
                "wlan0: CTRL-EVENT-CONNECTED - Connection to 82:83:c2:9e:22:71 completed [id=0 id_str=]",
                "wlan0"
            ),
            Some(WpaEvent::Connected)
        );
        assert_eq!(
            wpa_event("p2p-dev-wlan0: CTRL-EVENT-CONNECTED", "wlan0"),
            None
        );
        assert_eq!(
            wpa_event("wlan0: CTRL-EVENT-DISCONNECTED bssid=x", "wlan0"),
            None
        );
        let kernel = journal_line("1793768179.1 host kernel: wlan: SCAN COMPLETED").expect("line");
        assert_eq!(kernel.identifier, "kernel");
        assert!(is_wifi_kernel_line(&kernel.message));
        assert!(!is_wifi_kernel_line("usb 1-1: new device"));
        assert_eq!(journal_line("-- No entries --"), None);
    }
}
