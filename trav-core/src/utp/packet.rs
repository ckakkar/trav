//! µTP packet format (BEP 29).
//!
//! ```text
//! 0       4       8               16              24              32
//! +-------+-------+---------------+---------------+---------------+
//! | type  | ver   | extension     | connection_id                 |
//! +-------+-------+---------------+---------------+---------------+
//! | timestamp_microseconds                                        |
//! +---------------+---------------+---------------+---------------+
//! | timestamp_difference_microseconds                             |
//! +---------------+---------------+---------------+---------------+
//! | wnd_size                                                      |
//! +---------------+---------------+---------------+---------------+
//! | seq_nr                        | ack_nr                        |
//! +---------------+---------------+---------------+---------------+
//! ```
//!
//! Extensions follow the header as a chain of `next, len, data`; the only one
//! that matters is the selective ack (1), a bitmask of packets received past
//! `ack_nr + 1`.

pub const VERSION: u8 = 1;
pub const HEADER_LEN: usize = 20;
const EXT_SACK: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
    Data = 0,
    Fin = 1,
    State = 2,
    Reset = 3,
    Syn = 4,
}

impl Type {
    fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Self::Data,
            1 => Self::Fin,
            2 => Self::State,
            3 => Self::Reset,
            4 => Self::Syn,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub ty: Type,
    pub conn_id: u16,
    /// Sender's clock when the packet left, in microseconds (wrapping).
    pub ts: u32,
    /// Receiver clock minus the sender timestamp of the last packet received:
    /// the one-way delay of our packets (plus clock offset), echoed back.
    pub ts_diff: u32,
    /// Bytes the sender can still receive.
    pub wnd: u32,
    pub seq: u16,
    pub ack: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct Packet<'a> {
    pub h: Header,
    /// Selective-ack bitmask: bit `i` (LSB first, byte by byte) is `ack + 2 + i`.
    pub sack: Option<&'a [u8]>,
    pub payload: &'a [u8],
}

/// Cheap test for the UDP demultiplexer: could this datagram be µTP?
/// (DHT messages are bencoded dictionaries and start with `d`.)
pub fn looks_like_utp(buf: &[u8]) -> bool {
    buf.len() >= HEADER_LEN && buf[0] & 0x0f == VERSION && buf[0] >> 4 <= 4
}

pub fn parse(buf: &[u8]) -> Option<Packet<'_>> {
    if !looks_like_utp(buf) {
        return None;
    }
    let ty = Type::from_u8(buf[0] >> 4)?;
    let u16_at = |i: usize| u16::from_be_bytes([buf[i], buf[i + 1]]);
    let u32_at = |i: usize| u32::from_be_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]);
    let h = Header {
        ty,
        conn_id: u16_at(2),
        ts: u32_at(4),
        ts_diff: u32_at(8),
        wnd: u32_at(12),
        seq: u16_at(16),
        ack: u16_at(18),
    };
    let mut ext = buf[1];
    let mut i = HEADER_LEN;
    let mut sack = None;
    let mut hops = 0;
    while ext != 0 {
        hops += 1;
        if hops > 8 || i + 2 > buf.len() {
            return None;
        }
        let next = buf[i];
        let len = buf[i + 1] as usize;
        let data = buf.get(i + 2..i + 2 + len)?;
        if ext == EXT_SACK && !data.is_empty() {
            sack = Some(data);
        }
        i += 2 + len;
        ext = next;
    }
    Some(Packet {
        h,
        sack,
        payload: &buf[i..],
    })
}

/// Append a packet to `out`.
pub fn encode(h: &Header, sack: Option<&[u8]>, payload: &[u8], out: &mut Vec<u8>) {
    out.reserve(HEADER_LEN + sack.map_or(0, |s| s.len() + 2) + payload.len());
    out.push((h.ty as u8) << 4 | VERSION);
    out.push(if sack.is_some() { EXT_SACK } else { 0 });
    out.extend_from_slice(&h.conn_id.to_be_bytes());
    out.extend_from_slice(&h.ts.to_be_bytes());
    out.extend_from_slice(&h.ts_diff.to_be_bytes());
    out.extend_from_slice(&h.wnd.to_be_bytes());
    out.extend_from_slice(&h.seq.to_be_bytes());
    out.extend_from_slice(&h.ack.to_be_bytes());
    if let Some(s) = sack {
        out.push(0);
        out.push(s.len() as u8);
        out.extend_from_slice(s);
    }
    out.extend_from_slice(payload);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(ty: Type) -> Header {
        Header {
            ty,
            conn_id: 0xbeef,
            ts: 0x0102_0304,
            ts_diff: 0x0506_0708,
            wnd: 1 << 20,
            seq: 0xfffe,
            ack: 7,
        }
    }

    #[test]
    fn roundtrip_with_sack() {
        let mut buf = Vec::new();
        encode(
            &header(Type::State),
            Some(&[0b101, 0, 0, 0x80]),
            b"",
            &mut buf,
        );
        let p = parse(&buf).unwrap();
        assert_eq!(p.h, header(Type::State));
        assert_eq!(p.sack, Some(&[0b101, 0, 0, 0x80][..]));
        assert!(p.payload.is_empty());
    }

    #[test]
    fn roundtrip_data() {
        let mut buf = Vec::new();
        encode(&header(Type::Data), None, b"hello", &mut buf);
        assert_eq!(buf.len(), HEADER_LEN + 5);
        let p = parse(&buf).unwrap();
        assert_eq!(p.h.ty, Type::Data);
        assert_eq!(p.payload, b"hello");
        assert!(p.sack.is_none());
    }

    #[test]
    fn known_bytes() {
        // A SYN as libutp sends it: type 4, version 1.
        let raw = [
            0x41, 0x00, 0x12, 0x34, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0x10, 0, 0, 0, 1, 0, 0,
        ];
        let p = parse(&raw).unwrap();
        assert_eq!(p.h.ty, Type::Syn);
        assert_eq!(p.h.conn_id, 0x1234);
        assert_eq!(p.h.wnd, 0x10_0000);
        assert_eq!(p.h.seq, 1);
    }

    #[test]
    fn rejects_dht_and_garbage() {
        assert!(!looks_like_utp(
            b"d1:ad2:id20:aaaaaaaaaaaaaaaaaaaae1:q4:ping1:t2:aa1:y1:qe"
        ));
        assert!(parse(&[0x41; 10]).is_none());
        assert!(parse(&[0x51; 20]).is_none(), "type 5 does not exist");
        // An extension chain running past the end.
        let mut raw = vec![0x21, 1];
        raw.extend_from_slice(&[0; 18]);
        raw.extend_from_slice(&[0, 30, 1, 2]);
        assert!(parse(&raw).is_none());
    }

    #[test]
    fn skips_unknown_extensions() {
        let mut raw = vec![0x21, 2];
        raw.extend_from_slice(&[0; 18]);
        // Extension bits (2), 8 bytes, then a sack (1) of 4.
        raw.extend_from_slice(&[1, 8, 0, 0, 0, 0, 0, 0, 0, 0]);
        raw.extend_from_slice(&[0, 4, 1, 0, 0, 0]);
        raw.extend_from_slice(b"x");
        let p = parse(&raw).unwrap();
        assert_eq!(p.sack, Some(&[1, 0, 0, 0][..]));
        assert_eq!(p.payload, b"x");
    }
}
