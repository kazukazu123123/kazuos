use crate::drivers::e1000;
use crate::util::rdtsc;
use core::fmt::Write;

const ETHERTYPE_IPV4: u16 = 0x0800;
const ETHERTYPE_ARP: u16 = 0x0806;

const ARP_REQUEST: u16 = 1;
const ARP_REPLY: u16 = 2;

const IP_PROTO_ICMP: u8 = 1;
const IP_PROTO_UDP: u8 = 17;

const ICMP_ECHO_REQUEST: u8 = 8;
const ICMP_ECHO_REPLY: u8 = 0;

const PORT_DHCP_SERVER: u16 = 67;
const PORT_DHCP_CLIENT: u16 = 68;
const PORT_DNS: u16 = 53;
const PORT_EPHEMERAL: u16 = 49152;

const BROADCAST_MAC: [u8; 6] = [0xFF; 6];

const FRAME_MAX: usize = 1518;

const PING_COUNT: u32 = 4;

#[derive(Clone, Copy, Default)]
struct Config {
    mac: [u8; 6],
    ip: [u8; 4],
    gw: [u8; 4],
    dns: [u8; 4],
    mask: [u8; 4],
}

struct Report<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl Write for Report<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &b in s.as_bytes() {
            if self.pos >= self.buf.len() {
                break;
            }
            self.buf[self.pos] = b;
            self.pos += 1;
        }
        Ok(())
    }
}

fn fmt_ip(r: &mut Report, ip: [u8; 4]) {
    let _ = write!(r, "{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
}

fn tsc_per_ms() -> u64 {
    let v = unsafe { core::ptr::read_volatile(core::ptr::addr_of!(crate::user::TSC_PER_MS)) };
    if v == 0 { 1 } else { v }
}

fn now_ms() -> u64 {
    rdtsc() / tsc_per_ms()
}

fn wr_be16(buf: &mut [u8], off: usize, val: u16) {
    buf[off] = (val >> 8) as u8;
    buf[off + 1] = val as u8;
}

fn wr_be32(buf: &mut [u8], off: usize, val: u32) {
    buf[off] = (val >> 24) as u8;
    buf[off + 1] = (val >> 16) as u8;
    buf[off + 2] = (val >> 8) as u8;
    buf[off + 3] = val as u8;
}

fn rd_be16(buf: &[u8], off: usize) -> u16 {
    ((buf[off] as u16) << 8) | buf[off + 1] as u16
}

fn checksum16(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += ((data[i] as u32) << 8) | data[i + 1] as u32;
        i += 2;
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

fn write_eth(buf: &mut [u8], dst: [u8; 6], src: [u8; 6], ethertype: u16) {
    buf[0..6].copy_from_slice(&dst);
    buf[6..12].copy_from_slice(&src);
    wr_be16(buf, 12, ethertype);
}

fn write_ipv4(
    buf: &mut [u8],
    src: [u8; 4],
    dst: [u8; 4],
    proto: u8,
    payload_len: usize,
    ident: u16,
) {
    let ip = &mut buf[14..34];
    for b in ip.iter_mut() {
        *b = 0;
    }
    ip[0] = 0x45;
    wr_be16(ip, 2, (20 + payload_len) as u16);
    wr_be16(ip, 4, ident);
    ip[8] = 64;
    ip[9] = proto;
    ip[12..16].copy_from_slice(&src);
    ip[16..20].copy_from_slice(&dst);
    let csum = checksum16(&buf[14..34]);
    wr_be16(buf, 24, csum);
}

fn poll_recv(out: &mut [u8], deadline_ms: u64) -> Option<usize> {
    loop {
        if let Some(len) = e1000::receive(out) {
            if len > 0 {
                return Some(len);
            }
        }
        if now_ms() >= deadline_ms {
            return None;
        }
        core::hint::spin_loop();
    }
}

fn arp_resolve(cfg: &Config, target: [u8; 4]) -> Option<[u8; 6]> {
    let mut frame = [0u8; FRAME_MAX];
    write_eth(&mut frame, BROADCAST_MAC, cfg.mac, ETHERTYPE_ARP);
    let a = &mut frame[14..42];
    wr_be16(a, 0, 1);
    wr_be16(a, 2, ETHERTYPE_IPV4);
    a[4] = 6;
    a[5] = 4;
    wr_be16(a, 6, ARP_REQUEST);
    a[8..14].copy_from_slice(&cfg.mac);
    a[14..18].copy_from_slice(&cfg.ip);
    a[18..24].copy_from_slice(&[0u8; 6]);
    a[24..28].copy_from_slice(&target);

    for _ in 0..3 {
        if !e1000::transmit(&frame[..42]) {
            continue;
        }
        let deadline = now_ms() + 500;
        let mut rx = [0u8; FRAME_MAX];
        while let Some(len) = poll_recv(&mut rx, deadline) {
            if len < 42 || rd_be16(&rx, 12) != ETHERTYPE_ARP {
                continue;
            }
            let a = &rx[14..42];
            if rd_be16(a, 6) == ARP_REPLY && a[14..18] == target {
                let mut mac = [0u8; 6];
                mac.copy_from_slice(&a[8..14]);
                return Some(mac);
            }
        }
    }
    None
}

fn dhcp_build(
    msg_type: u8,
    xid: u32,
    mac: [u8; 6],
    requested_ip: Option<[u8; 4]>,
    server_id: Option<[u8; 4]>,
    out: &mut [u8],
) -> usize {
    write_eth(out, BROADCAST_MAC, mac, ETHERTYPE_IPV4);

    let dhcp_len = 300;
    let udp_len = 8 + dhcp_len;
    write_ipv4(out, [0, 0, 0, 0], [255, 255, 255, 255], IP_PROTO_UDP, udp_len, 0);

    let udp = &mut out[34..42];
    wr_be16(udp, 0, PORT_DHCP_CLIENT);
    wr_be16(udp, 2, PORT_DHCP_SERVER);
    wr_be16(udp, 4, udp_len as u16);
    wr_be16(udp, 6, 0);

    let d = &mut out[42..42 + dhcp_len];
    for b in d.iter_mut() {
        *b = 0;
    }
    d[0] = 1;
    d[1] = 1;
    d[2] = 6;
    wr_be32(d, 4, xid);
    wr_be16(d, 10, 0x8000);
    d[28..34].copy_from_slice(&mac);
    wr_be32(d, 236, 0x6382_5363);

    let mut o = 240;
    d[o] = 53;
    d[o + 1] = 1;
    d[o + 2] = msg_type;
    o += 3;
    if let Some(ip) = requested_ip {
        d[o] = 50;
        d[o + 1] = 4;
        d[o + 2..o + 6].copy_from_slice(&ip);
        o += 6;
    }
    if let Some(sid) = server_id {
        d[o] = 54;
        d[o + 1] = 4;
        d[o + 2..o + 6].copy_from_slice(&sid);
        o += 6;
    }
    d[o] = 55;
    d[o + 1] = 3;
    d[o + 2] = 1;
    d[o + 3] = 3;
    d[o + 4] = 6;
    o += 5;
    d[o] = 255;

    42 + dhcp_len
}

fn dhcp_parse(frame: &[u8], xid: u32) -> Option<(u8, Config)> {
    if frame.len() < 282 || rd_be16(frame, 12) != ETHERTYPE_IPV4 {
        return None;
    }
    if frame[23] != IP_PROTO_UDP {
        return None;
    }
    let ihl = (frame[14] & 0x0F) as usize * 4;
    let udp = 14 + ihl;
    if rd_be16(frame, udp + 2) != PORT_DHCP_CLIENT {
        return None;
    }
    let d = &frame[udp + 8..];
    if d.len() < 240 {
        return None;
    }
    if (d[4] as u32) << 24 | (d[5] as u32) << 16 | (d[6] as u32) << 8 | d[7] as u32 != xid {
        return None;
    }

    let mut cfg = Config::default();
    cfg.ip.copy_from_slice(&d[16..20]);
    let mut msg_type = 0u8;
    let mut o = 240;
    while o < d.len() && d[o] != 255 {
        if d[o] == 0 {
            o += 1;
            continue;
        }
        let code = d[o];
        let len = d[o + 1] as usize;
        let val = &d[o + 2..o + 2 + len];
        match code {
            53 if len == 1 => msg_type = val[0],
            1 if len == 4 => cfg.mask.copy_from_slice(val),
            3 if len >= 4 => cfg.gw.copy_from_slice(&val[..4]),
            6 if len >= 4 => cfg.dns.copy_from_slice(&val[..4]),
            _ => {}
        }
        o += 2 + len;
    }
    Some((msg_type, cfg))
}

fn dhcp_configure(mac: [u8; 6]) -> Option<Config> {
    let xid = (rdtsc() as u32) ^ 0x4B41_5A55;
    let mut frame = [0u8; FRAME_MAX];

    let len = dhcp_build(1, xid, mac, None, None, &mut frame);
    e1000::transmit(&frame[..len]);

    let deadline = now_ms() + 2000;
    let mut rx = [0u8; FRAME_MAX];
    let mut offer: Option<Config> = None;
    while let Some(rlen) = poll_recv(&mut rx, deadline) {
        if let Some((mt, cfg)) = dhcp_parse(&rx[..rlen], xid) {
            if mt == 2 {
                offer = Some(cfg);
                break;
            }
        }
    }
    let mut offer = offer?;
    offer.mac = mac;

    let len = dhcp_build(3, xid, mac, Some(offer.ip), Some(offer.gw), &mut frame);
    e1000::transmit(&frame[..len]);

    let deadline = now_ms() + 2000;
    while let Some(rlen) = poll_recv(&mut rx, deadline) {
        if let Some((mt, _)) = dhcp_parse(&rx[..rlen], xid) {
            if mt == 5 {
                return Some(offer);
            }
        }
    }
    None
}

fn dns_encode_name(host: &str, out: &mut [u8]) -> usize {
    let mut pos = 0;
    for label in host.split('.') {
        if label.is_empty() || label.len() > 63 {
            continue;
        }
        out[pos] = label.len() as u8;
        pos += 1;
        out[pos..pos + label.len()].copy_from_slice(label.as_bytes());
        pos += label.len();
    }
    out[pos] = 0;
    pos + 1
}

fn dns_resolve(cfg: &Config, dns_mac: [u8; 6], host: &str) -> Option<[u8; 4]> {
    let mut name = [0u8; 256];
    let name_len = dns_encode_name(host, &mut name);

    let dns_payload_len = 12 + name_len + 4;
    let udp_len = 8 + dns_payload_len;

    let mut frame = [0u8; FRAME_MAX];
    write_eth(&mut frame, dns_mac, cfg.mac, ETHERTYPE_IPV4);
    write_ipv4(&mut frame, cfg.ip, cfg.dns, IP_PROTO_UDP, udp_len, 0x1234);

    let udp = &mut frame[34..42];
    wr_be16(udp, 0, PORT_EPHEMERAL);
    wr_be16(udp, 2, PORT_DNS);
    wr_be16(udp, 4, udp_len as u16);
    wr_be16(udp, 6, 0);

    let id = rdtsc() as u16;
    let dns = &mut frame[42..42 + dns_payload_len];
    wr_be16(dns, 0, id);
    wr_be16(dns, 2, 0x0100);
    wr_be16(dns, 4, 1);
    wr_be16(dns, 6, 0);
    wr_be16(dns, 8, 0);
    wr_be16(dns, 10, 0);
    dns[12..12 + name_len].copy_from_slice(&name[..name_len]);
    wr_be16(dns, 12 + name_len, 1);
    wr_be16(dns, 14 + name_len, 1);

    let total = 42 + dns_payload_len;
    e1000::transmit(&frame[..total]);

    let deadline = now_ms() + 2000;
    let mut rx = [0u8; FRAME_MAX];
    while let Some(rlen) = poll_recv(&mut rx, deadline) {
        if let Some(ip) = dns_parse(&rx[..rlen], id) {
            return Some(ip);
        }
    }
    None
}

fn dns_parse(frame: &[u8], id: u16) -> Option<[u8; 4]> {
    if frame.len() < 42 || rd_be16(frame, 12) != ETHERTYPE_IPV4 || frame[23] != IP_PROTO_UDP {
        return None;
    }
    let ihl = (frame[14] & 0x0F) as usize * 4;
    let udp = 14 + ihl;
    if rd_be16(frame, udp) != PORT_DNS {
        return None;
    }
    let dns = &frame[udp + 8..];
    if dns.len() < 12 || rd_be16(dns, 0) != id {
        return None;
    }
    let qd = rd_be16(dns, 4);
    let an = rd_be16(dns, 6);
    if an == 0 {
        return None;
    }

    let mut pos = 12;
    for _ in 0..qd {
        pos = dns_skip_name(dns, pos)?;
        pos += 4;
    }
    for _ in 0..an {
        pos = dns_skip_name(dns, pos)?;
        if pos + 10 > dns.len() {
            return None;
        }
        let rtype = rd_be16(dns, pos);
        let rdlen = rd_be16(dns, pos + 8) as usize;
        pos += 10;
        if pos + rdlen > dns.len() {
            return None;
        }
        if rtype == 1 && rdlen == 4 {
            let mut ip = [0u8; 4];
            ip.copy_from_slice(&dns[pos..pos + 4]);
            return Some(ip);
        }
        pos += rdlen;
    }
    None
}

fn dns_skip_name(dns: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        if pos >= dns.len() {
            return None;
        }
        let len = dns[pos];
        if len & 0xC0 == 0xC0 {
            return Some(pos + 2);
        }
        if len == 0 {
            return Some(pos + 1);
        }
        pos += 1 + len as usize;
    }
}

fn parse_ipv4(host: &str) -> Option<[u8; 4]> {
    let mut octets = [0u8; 4];
    let mut count = 0;
    for part in host.split('.') {
        if count >= 4 || part.is_empty() || part.len() > 3 {
            return None;
        }
        let mut val: u32 = 0;
        for b in part.bytes() {
            if !b.is_ascii_digit() {
                return None;
            }
            val = val * 10 + (b - b'0') as u32;
        }
        if val > 255 {
            return None;
        }
        octets[count] = val as u8;
        count += 1;
    }
    if count == 4 { Some(octets) } else { None }
}

fn same_subnet(cfg: &Config, ip: [u8; 4]) -> bool {
    for i in 0..4 {
        if (cfg.ip[i] & cfg.mask[i]) != (ip[i] & cfg.mask[i]) {
            return false;
        }
    }
    true
}

fn ping_once(cfg: &Config, dst_mac: [u8; 6], dst_ip: [u8; 4], seq: u16) -> Option<(u8, u64)> {
    let icmp_len = 8 + 32;
    let mut frame = [0u8; FRAME_MAX];
    write_eth(&mut frame, dst_mac, cfg.mac, ETHERTYPE_IPV4);
    write_ipv4(&mut frame, cfg.ip, dst_ip, IP_PROTO_ICMP, icmp_len, 0x2000 + seq);

    let icmp = &mut frame[34..34 + icmp_len];
    for b in icmp.iter_mut() {
        *b = 0;
    }
    icmp[0] = ICMP_ECHO_REQUEST;
    wr_be16(icmp, 4, 0x4B5A);
    wr_be16(icmp, 6, seq);
    for (i, b) in icmp[8..].iter_mut().enumerate() {
        *b = i as u8;
    }
    let csum = checksum16(icmp);
    wr_be16(&mut frame[34..], 2, csum);

    let total = 34 + icmp_len;
    let start = rdtsc();
    e1000::transmit(&frame[..total]);

    let deadline = now_ms() + 2000;
    let mut rx = [0u8; FRAME_MAX];
    while let Some(rlen) = poll_recv(&mut rx, deadline) {
        if rlen < 42 || rd_be16(&rx, 12) != ETHERTYPE_IPV4 || rx[23] != IP_PROTO_ICMP {
            continue;
        }
        if rx[26..30] != dst_ip {
            continue;
        }
        let ihl = (rx[14] & 0x0F) as usize * 4;
        let icmp = &rx[14 + ihl..];
        if icmp.len() >= 8 && icmp[0] == ICMP_ECHO_REPLY && rd_be16(icmp, 6) == seq {
            let ttl = rx[22];
            let rtt = (rdtsc() - start) / tsc_per_ms();
            return Some((ttl, rtt));
        }
    }
    None
}

pub fn run_nettest(host: &str, out: &mut [u8]) -> usize {
    let mut r = Report { buf: out, pos: 0 };

    if !e1000::is_available() {
        let _ = write!(r, "nettest: no network device\r\n");
        return r.pos;
    }
    let mac = match e1000::mac() {
        Some(m) => m,
        None => {
            let _ = write!(r, "nettest: MAC unavailable\r\n");
            return r.pos;
        }
    };

    let cfg = match dhcp_configure(mac) {
        Some(c) => c,
        None => {
            let _ = write!(r, "nettest: DHCP failed\r\n");
            return r.pos;
        }
    };
    let _ = write!(r, "DHCP: ip=");
    fmt_ip(&mut r, cfg.ip);
    let _ = write!(r, " gw=");
    fmt_ip(&mut r, cfg.gw);
    let _ = write!(r, " dns=");
    fmt_ip(&mut r, cfg.dns);
    let _ = write!(r, "\r\n");

    let target = if let Some(ip) = parse_ipv4(host) {
        ip
    } else {
        let dns_next = if same_subnet(&cfg, cfg.dns) { cfg.dns } else { cfg.gw };
        let dns_mac = match arp_resolve(&cfg, dns_next) {
            Some(m) => m,
            None => {
                let _ = write!(r, "nettest: ARP for DNS failed\r\n");
                return r.pos;
            }
        };
        match dns_resolve(&cfg, dns_mac, host) {
            Some(ip) => {
                let _ = write!(r, "resolved {} -> ", host);
                fmt_ip(&mut r, ip);
                let _ = write!(r, "\r\n");
                ip
            }
            None => {
                let _ = write!(r, "nettest: DNS resolve failed for {}\r\n", host);
                return r.pos;
            }
        }
    };

    let next = if same_subnet(&cfg, target) { target } else { cfg.gw };
    let dst_mac = match arp_resolve(&cfg, next) {
        Some(m) => m,
        None => {
            let _ = write!(r, "nettest: ARP for route failed\r\n");
            return r.pos;
        }
    };

    let mut received = 0u32;
    for seq in 0..PING_COUNT as u16 {
        match ping_once(&cfg, dst_mac, target, seq) {
            Some((ttl, rtt)) => {
                received += 1;
                let _ = write!(r, "ping ");
                fmt_ip(&mut r, target);
                let _ = write!(r, ": seq={} ttl={} time={}ms\r\n", seq, ttl, rtt);
            }
            None => {
                let _ = write!(r, "ping ");
                fmt_ip(&mut r, target);
                let _ = write!(r, ": seq={} timeout\r\n", seq);
            }
        }
    }
    let _ = write!(r, "{}/{} replies received\r\n", received, PING_COUNT);

    r.pos
}
