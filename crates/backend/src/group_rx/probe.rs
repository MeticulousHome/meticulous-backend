//! The active check: a pair of DHCPINFORMs from our address, one with the
//! BOOTP broadcast flag and one without. A DHCP server answers an INFORM
//! without touching any lease; with the flag set it answers with an Ethernet
//! broadcast, which the AP sends as a group-addressed frame. So the flagged
//! answer arriving tests group reception, and the unflagged (unicast) answer
//! arriving tells a broken group path from a server that is not answering.
//!
//! Proven on the office network (IW612 and brcmfmac, 2026-11-03): both
//! answers arrive, the flagged one as a broadcast. Sending our own broadcast
//! and waiting for the AP to relay it does not work: the firmware drops
//! frames with our own source address.

use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

const ETHERNET: usize = 14;
const IPV4: usize = 20;
const UDP: usize = 8;
const BOOTP: usize = ETHERNET + IPV4 + UDP;
const MAGIC_COOKIE: [u8; 4] = [0x63, 0x82, 0x53, 0x63];
/// RFC 1542: relays may drop BOOTP messages shorter than this.
const BOOTP_MIN: usize = 300;
const BROADCAST_FLAG: u16 = 0x8000;

/// One DHCPINFORM to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inform {
    pub mac: [u8; 6],
    pub address: Ipv4Addr,
    pub xid: u32,
    /// Ask for the answer as a broadcast.
    pub broadcast: bool,
}

impl Inform {
    /// The whole Ethernet frame: to ff:ff:ff:ff:ff:ff, UDP 68 to 67, our
    /// address as `ciaddr` and our MAC as `chaddr`, message type 8.
    pub fn frame(&self) -> Vec<u8> {
        let mut bootp = Vec::with_capacity(BOOTP_MIN);
        bootp.extend_from_slice(&[1, 1, 6, 0]);
        bootp.extend_from_slice(&self.xid.to_be_bytes());
        bootp.extend_from_slice(&[0, 0]);
        let flags = if self.broadcast { BROADCAST_FLAG } else { 0 };
        bootp.extend_from_slice(&flags.to_be_bytes());
        bootp.extend_from_slice(&self.address.octets());
        bootp.extend_from_slice(&[0; 12]);
        bootp.extend_from_slice(&self.mac);
        bootp.extend_from_slice(&[0; 10 + 64 + 128]);
        bootp.extend_from_slice(&MAGIC_COOKIE);
        // Message type INFORM; ask for the subnet mask, router and DNS.
        bootp.extend_from_slice(&[53, 1, 8, 55, 3, 1, 3, 6, 255]);
        bootp.resize(BOOTP_MIN, 0);

        let udp_len = (UDP + bootp.len()) as u16;
        let mut frame = Vec::with_capacity(BOOTP + bootp.len());
        frame.extend_from_slice(&[0xff; 6]);
        frame.extend_from_slice(&self.mac);
        frame.extend_from_slice(&0x0800u16.to_be_bytes());
        let mut ip = [0u8; IPV4];
        ip[0] = 0x45;
        ip[2..4].copy_from_slice(&(IPV4 as u16 + udp_len).to_be_bytes());
        ip[8] = 64;
        ip[9] = 17;
        ip[12..16].copy_from_slice(&self.address.octets());
        ip[16..20].copy_from_slice(&[0xff; 4]);
        let checksum = ipv4_checksum(&ip);
        ip[10..12].copy_from_slice(&checksum.to_be_bytes());
        frame.extend_from_slice(&ip);
        // A zero UDP checksum means "none" over IPv4.
        frame.extend_from_slice(&68u16.to_be_bytes());
        frame.extend_from_slice(&67u16.to_be_bytes());
        frame.extend_from_slice(&udp_len.to_be_bytes());
        frame.extend_from_slice(&[0, 0]);
        frame.extend_from_slice(&bootp);
        frame
    }
}

fn ipv4_checksum(header: &[u8]) -> u16 {
    let mut sum: u32 = header
        .chunks(2)
        .map(|pair| {
            u32::from(u16::from_be_bytes([
                pair[0],
                pair.get(1).copied().unwrap_or(0),
            ]))
        })
        .sum();
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// How an answer reached us, by its Ethernet destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    Broadcast,
    Multicast,
    Unicast,
}

/// The delivery of a received frame if it is a BOOTP reply to `xid`.
pub fn reply_delivery(frame: &[u8], xid: u32) -> Option<Delivery> {
    if frame.len() < ETHERNET + IPV4 || frame[12..14] != [0x08, 0x00] || frame[23] != 17 {
        return None;
    }
    let ihl = usize::from(frame[ETHERNET] & 0x0f) * 4;
    let udp = ETHERNET + ihl;
    let bootp = udp + UDP;
    if ihl < IPV4 || frame.len() < bootp + 8 || frame[udp + 2..udp + 4] != [0, 68] {
        return None;
    }
    if frame[bootp] != 2 || frame[bootp + 4..bootp + 8] != xid.to_be_bytes() {
        return None;
    }
    Some(if frame[..6] == [0xff; 6] {
        Delivery::Broadcast
    } else if frame[0] & 1 == 1 {
        Delivery::Multicast
    } else {
        Delivery::Unicast
    })
}

/// The answers to one kind of INFORM, over every attempt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Replies {
    pub sent: u32,
    pub broadcast: u32,
    pub multicast: u32,
    pub unicast: u32,
}

impl Replies {
    pub fn add(&mut self, deliveries: &[Delivery]) {
        self.sent += 1;
        for delivery in deliveries {
            match delivery {
                Delivery::Broadcast => self.broadcast += 1,
                Delivery::Multicast => self.multicast += 1,
                Delivery::Unicast => self.unicast += 1,
            }
        }
    }

    fn group(&self) -> u32 {
        self.broadcast + self.multicast
    }
}

/// The answers to both INFORMs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeResult {
    /// With the broadcast flag.
    pub flagged: Replies,
    /// Without it.
    pub plain: Replies,
}

/// What a probe says about group reception.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// A group-addressed answer arrived.
    GroupRxWorks,
    /// The unicast answer arrived and no answer to the flagged INFORM did:
    /// the stall signature.
    GroupRxBroken,
    /// The server answered the flagged INFORM by unicast, so the probe
    /// cannot test group reception.
    ServerIgnoresFlag,
    /// No answer to either.
    ServerSilent,
    /// The INFORMs could not be sent (no address, socket error).
    SendFailed,
}

impl Verdict {
    pub fn code(self) -> &'static str {
        match self {
            Verdict::GroupRxWorks => "group_rx_works",
            Verdict::GroupRxBroken => "group_rx_broken",
            Verdict::ServerIgnoresFlag => "server_ignores_flag",
            Verdict::ServerSilent => "server_silent",
            Verdict::SendFailed => "send_failed",
        }
    }
}

/// Answers counted over both attempts. The flagged INFORM goes first, so a
/// server that rate-limits INFORMs drops the plain one, never the reverse.
pub fn classify(result: &ProbeResult) -> Verdict {
    let (flagged, plain) = (&result.flagged, &result.plain);
    if flagged.sent == 0 || plain.sent == 0 {
        return Verdict::SendFailed;
    }
    if flagged.group() > 0 || plain.group() > 0 {
        return Verdict::GroupRxWorks;
    }
    if flagged.unicast > 0 {
        return Verdict::ServerIgnoresFlag;
    }
    if plain.unicast > 0 {
        return Verdict::GroupRxBroken;
    }
    Verdict::ServerSilent
}

/// What the network does with INFORMs, learned once per association while
/// broadcast frames are arriving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Calibration {
    /// Not probed yet.
    Pending,
    /// The server answers the flagged INFORM with a broadcast: a missing
    /// answer later means group reception broke.
    Broadcasts,
    /// The server ignores the broadcast flag.
    IgnoresFlag,
    /// The server does not answer INFORMs: only the passive count is left.
    Silent,
    /// Only the unicast answer arrived even while broadcasts did.
    Unclear,
}

impl Calibration {
    pub fn code(self) -> &'static str {
        match self {
            Calibration::Pending => "pending",
            Calibration::Broadcasts => "broadcasts",
            Calibration::IgnoresFlag => "ignores_flag",
            Calibration::Silent => "silent",
            Calibration::Unclear => "unclear",
        }
    }
}

/// `None` when the probe could not run (it is retried).
pub fn calibration(result: &ProbeResult) -> Option<Calibration> {
    match classify(result) {
        Verdict::GroupRxWorks => Some(Calibration::Broadcasts),
        Verdict::GroupRxBroken => Some(Calibration::Unclear),
        Verdict::ServerIgnoresFlag => Some(Calibration::IgnoresFlag),
        Verdict::ServerSilent => Some(Calibration::Silent),
        Verdict::SendFailed => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// The probe showed the signature on a network whose server was seen
    /// answering the flagged INFORM with a broadcast.
    High,
    /// The signature without that calibration.
    Medium,
    /// Only the passive count: the probe could not test group reception.
    Low,
}

impl Confidence {
    pub fn of(verdict: Verdict, calibration: Calibration) -> Confidence {
        match (verdict, calibration) {
            (Verdict::GroupRxBroken, Calibration::Broadcasts) => Confidence::High,
            (Verdict::GroupRxBroken, _) => Confidence::Medium,
            _ => Confidence::Low,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Confidence::High => "high",
            Confidence::Medium => "medium",
            Confidence::Low => "low",
        }
    }
}

/// A classic BPF instruction: (code, jump if true, jump if false, k).
pub type Bpf = (u16, u8, u8, u32);

/// `SKF_AD_OFF + SKF_AD_PKTTYPE`: the kernel's packet type, which
/// `eth_type_trans` sets from the destination address.
const PKTTYPE: u32 = 0xffff_f004;
const PACKET_BROADCAST: u32 = 1;
const PACKET_MULTICAST: u32 = 2;

/// Inbound broadcast and multicast frames only, their first 64 bytes. Our
/// own transmissions are `PACKET_OUTGOING` and never match.
pub const GROUP_FILTER: [Bpf; 5] = [
    (0x20, 0, 0, PKTTYPE),
    (0x15, 2, 0, PACKET_BROADCAST),
    (0x15, 1, 0, PACKET_MULTICAST),
    (0x06, 0, 0, 0),
    (0x06, 0, 0, 64),
];

/// IPv4 UDP to port 68, unfragmented (`udp dst port 68`).
pub const DHCP_CLIENT_FILTER: [Bpf; 11] = [
    (0x28, 0, 0, 12),
    (0x15, 0, 8, 0x0800),
    (0x30, 0, 0, 23),
    (0x15, 0, 6, 17),
    (0x28, 0, 0, 20),
    (0x45, 4, 0, 0x1fff),
    (0xb1, 0, 0, 14),
    (0x48, 0, 0, 16),
    (0x15, 0, 1, 68),
    (0x06, 0, 0, 0xffff),
    (0x06, 0, 0, 0),
];

#[cfg(test)]
mod tests {
    use super::*;

    const MAC: [u8; 6] = [0x34, 0x90, 0xea, 0xac, 0x2c, 0x1c];
    const SERVER: [u8; 6] = [0xd0, 0x21, 0xf9, 0x00, 0x00, 0x01];

    fn inform(broadcast: bool) -> Inform {
        Inform {
            mac: MAC,
            address: Ipv4Addr::new(10, 10, 0, 84),
            xid: 0x1234_5678,
            broadcast,
        }
    }

    /// A DHCPACK as a server sends it, to `destination`.
    fn ack(destination: [u8; 6], xid: u32) -> Vec<u8> {
        let mut frame = inform(false).frame();
        frame[..6].copy_from_slice(&destination);
        frame[6..12].copy_from_slice(&SERVER);
        frame[ETHERNET + IPV4..ETHERNET + IPV4 + 4].copy_from_slice(&[0, 67, 0, 68]);
        frame[BOOTP] = 2;
        frame[BOOTP + 4..BOOTP + 8].copy_from_slice(&xid.to_be_bytes());
        frame
    }

    fn replies(sent: u32, broadcast: u32, unicast: u32) -> Replies {
        Replies {
            sent,
            broadcast,
            multicast: 0,
            unicast,
        }
    }

    /// The INFORM is the frame gateway_probe.py sent on the office network
    /// (RFC 2131 field layout), padded to RFC 1542's 300 bytes, with a valid
    /// IPv4 header checksum.
    #[test]
    fn inform_frame_layout() {
        let frame = inform(true).frame();
        assert_eq!(frame.len(), BOOTP + BOOTP_MIN);
        assert_eq!(frame[..6], [0xff; 6]);
        assert_eq!(frame[6..12], MAC);
        assert_eq!(frame[12..14], [0x08, 0x00]);
        let ip = &frame[ETHERNET..ETHERNET + IPV4];
        assert_eq!(ipv4_checksum(ip), 0, "a valid header sums to 0xffff");
        assert_eq!(ip[9], 17);
        assert_eq!(ip[12..16], [10, 10, 0, 84]);
        assert_eq!(ip[16..20], [0xff; 4]);
        assert_eq!(frame[34..38], [0, 68, 0, 67]);
        let bootp = &frame[BOOTP..];
        assert_eq!(bootp[..4], [1, 1, 6, 0]);
        assert_eq!(bootp[4..8], 0x1234_5678u32.to_be_bytes());
        assert_eq!(bootp[10..12], [0x80, 0x00]);
        assert_eq!(bootp[12..16], [10, 10, 0, 84], "ciaddr");
        assert_eq!(bootp[28..34], MAC, "chaddr");
        assert_eq!(bootp[236..240], MAGIC_COOKIE);
        assert_eq!(bootp[240..243], [53, 1, 8], "DHCPINFORM");
        assert_eq!(inform(false).frame()[BOOTP + 10..BOOTP + 12], [0, 0]);
    }

    #[test]
    fn replies_are_matched_by_xid_and_classified_by_destination() {
        let xid = 0x1234_5678;
        assert_eq!(
            reply_delivery(&ack([0xff; 6], xid), xid),
            Some(Delivery::Broadcast)
        );
        assert_eq!(reply_delivery(&ack(MAC, xid), xid), Some(Delivery::Unicast));
        assert_eq!(
            reply_delivery(&ack([0x01, 0, 0x5e, 0, 0, 1], xid), xid),
            Some(Delivery::Multicast)
        );
        assert_eq!(reply_delivery(&ack(MAC, xid), xid + 1), None, "other xid");
        assert_eq!(
            reply_delivery(&inform(true).frame(), xid),
            None,
            "a request"
        );
        assert_eq!(reply_delivery(&ack(MAC, xid)[..40], xid), None, "short");
    }

    #[test]
    fn verdicts() {
        let probe = |flagged, plain| classify(&ProbeResult { flagged, plain });
        assert_eq!(
            probe(replies(1, 1, 0), replies(1, 0, 1)),
            Verdict::GroupRxWorks
        );
        assert_eq!(
            probe(replies(2, 0, 0), replies(2, 1, 0)),
            Verdict::GroupRxWorks,
            "a server that always broadcasts"
        );
        assert_eq!(
            probe(replies(2, 0, 0), replies(2, 0, 2)),
            Verdict::GroupRxBroken
        );
        assert_eq!(
            probe(replies(2, 0, 2), replies(2, 0, 2)),
            Verdict::ServerIgnoresFlag
        );
        assert_eq!(
            probe(replies(2, 0, 0), replies(2, 0, 0)),
            Verdict::ServerSilent
        );
        assert_eq!(
            probe(replies(0, 0, 0), replies(2, 0, 2)),
            Verdict::SendFailed
        );
    }

    #[test]
    fn calibration_and_confidence() {
        let calibrate = |flagged, plain| calibration(&ProbeResult { flagged, plain });
        assert_eq!(
            calibrate(replies(1, 1, 0), replies(1, 0, 1)),
            Some(Calibration::Broadcasts)
        );
        assert_eq!(
            calibrate(replies(1, 0, 1), replies(1, 0, 1)),
            Some(Calibration::IgnoresFlag)
        );
        assert_eq!(
            calibrate(replies(1, 0, 0), replies(1, 0, 0)),
            Some(Calibration::Silent)
        );
        assert_eq!(
            calibrate(replies(1, 0, 0), replies(1, 0, 1)),
            Some(Calibration::Unclear)
        );
        assert_eq!(calibrate(replies(0, 0, 0), replies(0, 0, 0)), None);

        use Calibration::*;
        assert_eq!(
            Confidence::of(Verdict::GroupRxBroken, Broadcasts),
            Confidence::High
        );
        assert_eq!(
            Confidence::of(Verdict::GroupRxBroken, Pending),
            Confidence::Medium
        );
        for verdict in [
            Verdict::ServerSilent,
            Verdict::ServerIgnoresFlag,
            Verdict::SendFailed,
        ] {
            assert_eq!(Confidence::of(verdict, Broadcasts), Confidence::Low);
        }
    }

    /// The subset of classic BPF the two filters use, with the kernel's
    /// semantics: loads past the end and failed ancillary loads reject.
    fn run_bpf(program: &[Bpf], frame: &[u8], pkttype: u32) -> u32 {
        let (mut a, mut x, mut pc) = (0u32, 0u32, 0usize);
        let byte = |at: usize| frame.get(at).map(|b| u32::from(*b));
        let half = |at: usize| Some(byte(at)? << 8 | byte(at + 1)?);
        loop {
            let (code, jt, jf, k) = program[pc];
            pc += 1;
            let loaded = match code {
                0x20 if k == PKTTYPE => Some(pkttype),
                0x28 => half(k as usize),
                0x30 => byte(k as usize),
                0x48 => half(x as usize + k as usize),
                0xb1 => {
                    let Some(value) = byte(k as usize) else {
                        return 0;
                    };
                    x = (value & 0x0f) * 4;
                    continue;
                }
                0x15 => {
                    pc += usize::from(if a == k { jt } else { jf });
                    continue;
                }
                0x45 => {
                    pc += usize::from(if a & k != 0 { jt } else { jf });
                    continue;
                }
                0x06 => return k,
                other => panic!("instruction {other:#x} is not interpreted"),
            };
            match loaded {
                Some(value) => a = value,
                None => return 0,
            }
        }
    }

    #[test]
    fn group_filter_keeps_inbound_broadcast_and_multicast_only() {
        let frame = ack([0xff; 6], 1);
        assert_eq!(run_bpf(&GROUP_FILTER, &frame, PACKET_BROADCAST), 64);
        assert_eq!(run_bpf(&GROUP_FILTER, &frame, PACKET_MULTICAST), 64);
        // PACKET_HOST, PACKET_OTHERHOST, PACKET_OUTGOING.
        for pkttype in [0, 3, 4] {
            assert_eq!(run_bpf(&GROUP_FILTER, &frame, pkttype), 0);
        }
    }

    #[test]
    fn dhcp_filter_keeps_udp_to_port_68() {
        let reply = ack(MAC, 1);
        assert_eq!(run_bpf(&DHCP_CLIENT_FILTER, &reply, 0), 0xffff);
        assert_eq!(
            run_bpf(&DHCP_CLIENT_FILTER, &inform(true).frame(), 4),
            0,
            "our request goes to port 67"
        );
        let mut fragment = reply.clone();
        fragment[ETHERNET + 7] = 1;
        assert_eq!(run_bpf(&DHCP_CLIENT_FILTER, &fragment, 0), 0);
        let mut tcp = reply.clone();
        tcp[ETHERNET + 9] = 6;
        assert_eq!(run_bpf(&DHCP_CLIENT_FILTER, &tcp, 0), 0);
        let mut arp = reply;
        arp[12..14].copy_from_slice(&[0x08, 0x06]);
        assert_eq!(run_bpf(&DHCP_CLIENT_FILTER, &arp, 0), 0);
    }
}
