//! One µTP connection as a state machine without I/O: datagrams and the clock
//! go in; datagrams to send and bytes for the reader come out. The socket
//! driver (`socket.rs`) feeds it, and the tests below run two of them over a
//! simulated lossy, reordering network.
//!
//! Sequencing follows libutp and libtorrent (the implementations everyone
//! talks to): a SYN, data and FIN each take a sequence number, a STATE (pure
//! ack) carries the next number its sender will use and takes none.
//!
//! Congestion control is LEDBAT: the window grows while the one-way delay our
//! packets see stays under 100 ms above its base and shrinks as it rises, so
//! uploads give way to everything else on the line. Slow start gets a new
//! connection up to speed in a few round trips; losses (from selective acks,
//! triple duplicate acks or a timeout) cut the window as TCP would.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};
use tokio::io::ReadBuf;

use super::packet::{self, HEADER_LEN, Header, Type};

/// Largest datagram we send. Small enough for a VPN tunnel's MTU (WireGuard
/// leaves 1392 bytes for UDP over IPv4), so nothing gets fragmented.
pub const PACKET_SIZE: usize = 1360;
/// Payload per data packet.
pub const MSS: usize = PACKET_SIZE - HEADER_LEN;
/// Bytes we accept ahead of the reader (in order and out of order together).
pub const RECV_WINDOW: usize = 1 << 20;
/// Bytes the writer may queue ahead of the network.
pub const SEND_BUFFER: usize = 1 << 20;

const TARGET_DELAY_US: f64 = 100_000.0;
/// Most the window grows (or shrinks) per round trip under LEDBAT.
const GAIN: f64 = 3000.0;
const MIN_CWND: f64 = (2 * MSS) as f64;
const MAX_CWND: f64 = (4 << 20) as f64;
const INITIAL_CWND: f64 = (2 * MSS) as f64;
const MIN_RTO: Duration = Duration::from_millis(500);
const MAX_RTO: Duration = Duration::from_secs(30);
const INITIAL_RTO: Duration = Duration::from_secs(1);
/// Consecutive timeouts on the same data before giving up (~30 s with backoff).
const MAX_TIMEOUTS: u32 = 6;
/// Idle connections still send something this often, so NATs keep the mapping.
const KEEPALIVE: Duration = Duration::from_secs(29);
/// Silence after which the other end is gone.
const DEAD_AFTER: Duration = Duration::from_secs(90);
/// Packets kept in flight at most (well inside the 16-bit sequence space).
const MAX_OUT: usize = 4096;
/// How far ahead of the next expected packet we buffer.
const MAX_REORDER: u16 = 4096;
/// Sequence numbers a selective ack can describe (32 bytes of bitmask).
const SACK_SPAN: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    SynSent,
    Connected,
}

/// How hard to try a connection that gets no answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dial {
    /// A few SYNs over five seconds.
    Normal,
    /// NAT hole punching: both ends send at once, and the first packets die at
    /// the other end's NAT, so keep sending a SYN every second for longer.
    Holepunch,
}

struct OutPkt {
    seq: u16,
    ty: Type,
    payload: Bytes,
    sent_at: Instant,
    transmissions: u32,
    /// Counted in `in_flight` (sent, and not yet acked or given up as lost).
    counted: bool,
    need_resend: bool,
    sacked: bool,
}

/// Minimum one-way delay over the last few minutes (the path without
/// queueing) and the latest samples above it.
#[derive(Default)]
struct DelayHist {
    /// Per-minute minima, newest first.
    mins: [Option<u32>; 3],
    minute_start: Option<Instant>,
    recent: [u32; 3],
    n: usize,
    next: usize,
}

/// `a < b` for wrapping microsecond clocks.
fn lt32(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

fn seq_lt(a: u16, b: u16) -> bool {
    (a.wrapping_sub(b) as i16) < 0
}

impl DelayHist {
    fn add(&mut self, sample: u32, now: Instant) {
        let start = *self.minute_start.get_or_insert(now);
        if now.duration_since(start) >= Duration::from_secs(60) {
            self.mins.rotate_right(1);
            self.mins[0] = None;
            self.minute_start = Some(now);
        }
        self.mins[0] = Some(match self.mins[0] {
            Some(m) if !lt32(sample, m) => m,
            _ => sample,
        });
        self.recent[self.next] = sample;
        self.next = (self.next + 1) % self.recent.len();
        self.n = (self.n + 1).min(self.recent.len());
    }

    /// Current queueing delay in microseconds.
    fn queueing_us(&self) -> Option<u32> {
        let base = self
            .mins
            .iter()
            .flatten()
            .copied()
            .reduce(|a, b| if lt32(b, a) { b } else { a })?;
        let cur = self.recent[..self.n]
            .iter()
            .copied()
            .reduce(|a, b| if lt32(b, a) { b } else { a })?;
        Some((cur.wrapping_sub(base) as i32).max(0) as u32)
    }
}

pub struct Conn {
    pub recv_id: u16,
    pub send_id: u16,
    pub state: State,
    epoch: Instant,
    dial: Dial,
    /// Next sequence number to send.
    seq_nr: u16,
    /// Last sequence number received in order.
    ack_nr: u16,
    /// For an accepted connection, the SYN's number (to spot retransmitted SYNs).
    syn_seq: Option<u16>,
    syn_tries: u32,

    // ── Sending ──────────────────────────────────────────────────────────
    send_buf: BytesMut,
    out: VecDeque<OutPkt>,
    in_flight: usize,
    resends: usize,
    cwnd: f64,
    ssthresh: f64,
    slow_start: bool,
    /// The window, not the application, was what held sending back.
    cwnd_limited: bool,
    peer_wnd: usize,
    /// Losses of packets up to here were already answered with a window cut.
    loss_seq: u16,
    dup_acks: u32,
    srtt: Option<Duration>,
    rttvar: Duration,
    rto: Duration,
    rto_deadline: Option<Instant>,
    timeouts: u32,
    /// Send one packet into a closed receive window (probe) at the next chance.
    probe: bool,
    delay: DelayHist,
    /// Our clock minus the timestamp of the last packet received, echoed back.
    reply_micro: u32,
    closing: bool,
    fin_seq: Option<u16>,
    fin_acked: bool,
    dropped: bool,
    send_reset: bool,

    // ── Receiving ────────────────────────────────────────────────────────
    recv_buf: VecDeque<Bytes>,
    recv_len: usize,
    reorder: HashMap<u16, (Bytes, bool)>,
    reorder_bytes: usize,
    eof_seq: Option<u16>,
    eof: bool,
    need_ack: bool,

    last_recv: Instant,
    last_send: Instant,
    error: Option<io::ErrorKind>,
    pub stats: Stats,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    pub sent_packets: u64,
    pub resent_packets: u64,
    pub recv_packets: u64,
}

impl Conn {
    fn new(recv_id: u16, send_id: u16, state: State, epoch: Instant, now: Instant) -> Self {
        Self {
            recv_id,
            send_id,
            state,
            epoch,
            dial: Dial::Normal,
            seq_nr: rand::random(),
            ack_nr: 0,
            syn_seq: None,
            syn_tries: 0,
            send_buf: BytesMut::new(),
            out: VecDeque::new(),
            in_flight: 0,
            resends: 0,
            cwnd: INITIAL_CWND,
            ssthresh: MAX_CWND,
            slow_start: true,
            cwnd_limited: false,
            peer_wnd: MSS * 4,
            loss_seq: 0,
            dup_acks: 0,
            srtt: None,
            rttvar: Duration::ZERO,
            rto: INITIAL_RTO,
            rto_deadline: None,
            timeouts: 0,
            probe: false,
            delay: DelayHist::default(),
            reply_micro: 0,
            closing: false,
            fin_seq: None,
            fin_acked: false,
            dropped: false,
            send_reset: false,
            recv_buf: VecDeque::new(),
            recv_len: 0,
            reorder: HashMap::new(),
            reorder_bytes: 0,
            eof_seq: None,
            eof: false,
            need_ack: false,
            last_recv: now,
            last_send: now,
            error: None,
            stats: Stats::default(),
        }
    }

    /// Start a connection: the SYN goes out with the first `flush`.
    pub fn connect(recv_id: u16, epoch: Instant, now: Instant, dial: Dial) -> Self {
        let mut c = Self::new(recv_id, recv_id.wrapping_add(1), State::SynSent, epoch, now);
        c.dial = dial;
        c.loss_seq = c.seq_nr;
        c.push(Type::Syn, Bytes::new());
        c
    }

    /// Answer a SYN; the STATE reply goes out with the first `flush`.
    pub fn accept(syn: &Header, epoch: Instant, now: Instant) -> Self {
        let mut c = Self::new(
            syn.conn_id.wrapping_add(1),
            syn.conn_id,
            State::Connected,
            epoch,
            now,
        );
        c.ack_nr = syn.seq;
        c.syn_seq = Some(syn.seq);
        c.loss_seq = c.seq_nr.wrapping_sub(1);
        c.peer_wnd = (syn.wnd as usize).max(MSS);
        c.reply_micro = c.micros(now).wrapping_sub(syn.ts);
        c.need_ack = true;
        c
    }

    fn micros(&self, now: Instant) -> u32 {
        now.saturating_duration_since(self.epoch).as_micros() as u32
    }

    pub fn error(&self) -> Option<io::ErrorKind> {
        self.error
    }

    pub fn srtt(&self) -> Option<Duration> {
        self.srtt
    }

    fn fail(&mut self, kind: io::ErrorKind) {
        if self.error.is_none() {
            self.error = Some(kind);
        }
    }

    fn recv_window(&self) -> usize {
        RECV_WINDOW.saturating_sub(self.recv_len + self.reorder_bytes)
    }

    fn push(&mut self, ty: Type, payload: Bytes) {
        self.out.push_back(OutPkt {
            seq: self.seq_nr,
            ty,
            payload,
            sent_at: self.epoch,
            transmissions: 0,
            counted: false,
            need_resend: true,
            sacked: false,
        });
        self.resends += 1;
        self.seq_nr = self.seq_nr.wrapping_add(1);
    }

    fn uncount(&mut self, i: usize) {
        let p = &mut self.out[i];
        if p.counted {
            p.counted = false;
            self.in_flight -= p.payload.len();
        }
    }

    // ── Input ────────────────────────────────────────────────────────────

    /// Process one datagram for this connection, received at `now`.
    pub fn on_datagram(&mut self, dg: &Bytes, now: Instant) {
        let Some(p) = packet::parse(dg) else { return };
        if self.error.is_some() {
            return;
        }
        let h = p.h;
        self.stats.recv_packets += 1;
        match h.ty {
            Type::Reset => {
                self.fail(if self.state == State::SynSent {
                    io::ErrorKind::ConnectionRefused
                } else {
                    io::ErrorKind::ConnectionReset
                });
                return;
            }
            Type::Syn => {
                // Our STATE reply was lost: send it again.
                if self.syn_seq == Some(h.seq) {
                    self.need_ack = true;
                }
                return;
            }
            _ => {}
        }
        // An ack for something we never sent: not for us (or forged).
        if seq_lt(self.seq_nr.wrapping_sub(1), h.ack) {
            return;
        }
        if self.state == State::SynSent {
            // Until our SYN is acked nothing else makes sense.
            if self
                .out
                .front()
                .is_none_or(|s| s.ty != Type::Syn || s.seq != h.ack)
            {
                return;
            }
            // Their STATE names the first number they'll send.
            self.ack_nr = h.seq.wrapping_sub(1);
            self.state = State::Connected;
        }
        self.last_recv = now;
        self.reply_micro = self.micros(now).wrapping_sub(h.ts);
        self.peer_wnd = h.wnd as usize;
        if h.ts_diff != 0 {
            self.delay.add(h.ts_diff, now);
        }
        let acked = self.on_ack(
            h.ack,
            p.sack,
            h.ty == Type::State && p.payload.is_empty(),
            now,
        );
        self.grow(acked);

        match h.ty {
            Type::Data if !p.payload.is_empty() => {
                let off = dg.len() - p.payload.len();
                self.on_data(h.seq, dg.slice(off..), false);
            }
            Type::Fin => {
                let off = dg.len() - p.payload.len();
                self.on_data(h.seq, dg.slice(off..), true);
            }
            _ => {}
        }
    }

    /// Process an ack (cumulative and selective); returns the bytes newly acked.
    fn on_ack(&mut self, ack: u16, sack: Option<&[u8]>, pure: bool, now: Instant) -> usize {
        let Some(front) = self.out.front().map(|p| p.seq) else {
            return 0;
        };
        let mut acked = 0usize;
        let n = ack.wrapping_sub(front).wrapping_add(1) as usize;
        let n = if n > self.out.len() { 0 } else { n };
        for _ in 0..n {
            self.uncount(0);
            let p = self.out.pop_front().expect("n <= len");
            if p.need_resend {
                self.resends -= 1;
            }
            if !p.sacked {
                acked += p.payload.len();
                if p.transmissions == 1 {
                    self.rtt_sample(now.saturating_duration_since(p.sent_at));
                }
            }
            if p.ty == Type::Fin {
                self.fin_acked = true;
            }
        }
        if n > 0 {
            self.dup_acks = 0;
            self.timeouts = 0;
            self.rto_deadline = (!self.out.is_empty()).then(|| now + self.rto);
        } else if pure && sack.is_none() && !self.out.is_empty() && self.in_flight > 0 {
            self.dup_acks += 1;
            if self.dup_acks == 3 {
                self.lost(0, now);
            }
        }

        if let Some(mask) = sack {
            let base = ack.wrapping_add(2);
            let front = self.out.front().map(|p| p.seq);
            if let Some(front) = front {
                for (byte_i, byte) in mask.iter().enumerate() {
                    for bit in 0..8 {
                        if byte & (1 << bit) == 0 {
                            continue;
                        }
                        let seq = base.wrapping_add((byte_i * 8 + bit) as u16);
                        let i = seq.wrapping_sub(front) as usize;
                        if i >= self.out.len() || self.out[i].sacked {
                            continue;
                        }
                        self.uncount(i);
                        let p = &mut self.out[i];
                        p.sacked = true;
                        if p.need_resend {
                            p.need_resend = false;
                            self.resends -= 1;
                        }
                        acked += p.payload.len();
                        if p.transmissions == 1 {
                            let rtt = now.saturating_duration_since(p.sent_at);
                            self.rtt_sample(rtt);
                        }
                    }
                }
                // Anything with three or more packets acked after it is lost.
                let mut after = 0;
                let srtt = self.srtt.unwrap_or(INITIAL_RTO);
                for i in (0..self.out.len()).rev() {
                    let p = &self.out[i];
                    if p.sacked {
                        after += 1;
                    } else if after >= 3
                        && !p.need_resend
                        && p.transmissions > 0
                        && (p.transmissions == 1 || now.saturating_duration_since(p.sent_at) > srtt)
                    {
                        self.lost(i, now);
                    }
                }
            }
        }
        acked
    }

    /// Mark a packet lost: resend it, and cut the window once per loss event.
    fn lost(&mut self, i: usize, _now: Instant) {
        self.uncount(i);
        let p = &mut self.out[i];
        if !p.need_resend {
            p.need_resend = true;
            self.resends += 1;
        }
        let seq = p.seq;
        if seq_lt(self.loss_seq, seq) {
            self.cwnd = (self.cwnd * 0.5).max(MIN_CWND);
            self.ssthresh = self.cwnd;
            self.slow_start = false;
            self.loss_seq = self.seq_nr.wrapping_sub(1);
        }
    }

    fn rtt_sample(&mut self, rtt: Duration) {
        match self.srtt {
            None => {
                self.srtt = Some(rtt);
                self.rttvar = rtt / 2;
            }
            Some(s) => {
                let diff = s.abs_diff(rtt);
                self.rttvar = (self.rttvar * 3 + diff) / 4;
                self.srtt = Some((s * 7 + rtt) / 8);
            }
        }
        let s = self.srtt.unwrap_or(rtt);
        self.rto = (s + self.rttvar * 4).clamp(MIN_RTO, MAX_RTO);
    }

    /// LEDBAT (with slow start): grow towards the target delay, shrink past it.
    fn grow(&mut self, acked: usize) {
        if acked == 0 {
            return;
        }
        let delay = self.delay.queueing_us().map_or(0.0, f64::from);
        let off_target = ((TARGET_DELAY_US - delay) / TARGET_DELAY_US).clamp(-1.0, 1.0);
        let window_factor = (acked as f64).min(self.cwnd) / self.cwnd;
        let linear = GAIN * off_target * window_factor;
        let mut gain = linear;
        if self.slow_start {
            if delay > TARGET_DELAY_US / 2.0 || self.cwnd >= self.ssthresh {
                self.slow_start = false;
                self.ssthresh = self.cwnd;
            } else {
                gain = gain.max(acked as f64);
            }
        }
        // An application-limited window would only grow on paper.
        if gain > 0.0 && !self.cwnd_limited {
            return;
        }
        self.cwnd = (self.cwnd + gain).clamp(MIN_CWND, MAX_CWND);
    }

    fn on_data(&mut self, seq: u16, payload: Bytes, fin: bool) {
        self.need_ack = true;
        if let Some(e) = self.eof_seq
            && seq_lt(e, seq)
        {
            return;
        }
        let off = seq.wrapping_sub(self.ack_nr.wrapping_add(1));
        if off == 0 {
            if self.recv_len + payload.len() > RECV_WINDOW + MSS * 8 {
                // Way past our window: drop it, the sender will try again.
                return;
            }
            self.deliver(seq, payload, fin);
            while let Some((p, f)) = self.reorder.remove(&self.ack_nr.wrapping_add(1)) {
                self.reorder_bytes -= p.len();
                self.deliver(self.ack_nr.wrapping_add(1), p, f);
            }
        } else if off < MAX_REORDER {
            if fin {
                self.eof_seq = Some(seq);
            }
            if !self.reorder.contains_key(&seq)
                && self.recv_len + self.reorder_bytes + payload.len() <= RECV_WINDOW
            {
                self.reorder_bytes += payload.len();
                self.reorder.insert(seq, (payload, fin));
            }
        }
        // Otherwise a duplicate of something delivered: the ack goes out again.
    }

    fn deliver(&mut self, seq: u16, payload: Bytes, fin: bool) {
        self.ack_nr = seq;
        if !payload.is_empty() {
            self.recv_len += payload.len();
            self.recv_buf.push_back(payload);
        }
        if fin {
            self.eof_seq = Some(seq);
            self.eof = true;
            self.reorder.clear();
            self.reorder_bytes = 0;
        }
    }

    // ── Output ───────────────────────────────────────────────────────────

    fn may_send(&self, len: usize) -> bool {
        if self.in_flight == 0 {
            return self.probe || len <= self.peer_wnd;
        }
        self.in_flight + len <= (self.cwnd as usize).min(self.peer_wnd)
    }

    fn header(&self, ty: Type, seq: u16, now: Instant) -> Header {
        Header {
            ty,
            conn_id: if ty == Type::Syn {
                self.recv_id
            } else {
                self.send_id
            },
            ts: self.micros(now),
            ts_diff: self.reply_micro,
            wnd: self.recv_window() as u32,
            seq,
            ack: self.ack_nr,
        }
    }

    fn transmit(&mut self, i: usize, now: Instant, out: &mut Vec<Vec<u8>>) {
        let h = self.header(self.out[i].ty, self.out[i].seq, now);
        let mut buf = Vec::with_capacity(HEADER_LEN + self.out[i].payload.len());
        packet::encode(&h, None, &self.out[i].payload, &mut buf);
        out.push(buf);
        let p = &mut self.out[i];
        p.transmissions += 1;
        p.sent_at = now;
        if p.need_resend {
            p.need_resend = false;
            self.resends -= 1;
        }
        if !p.counted {
            p.counted = true;
            self.in_flight += p.payload.len();
        }
        self.stats.sent_packets += 1;
        if p.transmissions > 1 {
            self.stats.resent_packets += 1;
        }
        self.probe = false;
        self.need_ack = !self.reorder.is_empty() && self.need_ack;
        self.last_send = now;
        if self.rto_deadline.is_none() {
            self.rto_deadline = Some(now + self.rto);
        }
    }

    fn sack(&self) -> Option<Vec<u8>> {
        if self.reorder.is_empty() {
            return None;
        }
        let base = self.ack_nr.wrapping_add(2);
        let mut mask = [0u8; SACK_SPAN / 8];
        let mut top = None;
        for i in 0..SACK_SPAN {
            if self.reorder.contains_key(&base.wrapping_add(i as u16)) {
                mask[i / 8] |= 1 << (i % 8);
                top = Some(i);
            }
        }
        let top = top?;
        let len = (top / 8 + 1).div_ceil(4) * 4;
        Some(mask[..len].to_vec())
    }

    /// Everything that should go on the wire now.
    pub fn flush(&mut self, now: Instant, out: &mut Vec<Vec<u8>>) {
        if self.send_reset {
            self.send_reset = false;
            let mut buf = Vec::with_capacity(HEADER_LEN);
            packet::encode(
                &self.header(Type::Reset, self.seq_nr, now),
                None,
                &[],
                &mut buf,
            );
            out.push(buf);
            return;
        }
        if self.error.is_some() {
            return;
        }
        let mut blocked = false;
        // Retransmissions first, oldest first.
        if self.resends > 0 {
            for i in 0..self.out.len() {
                if !self.out[i].need_resend {
                    continue;
                }
                if !self.may_send(self.out[i].payload.len()) {
                    blocked = true;
                    break;
                }
                self.transmit(i, now, out);
                if self.resends == 0 {
                    break;
                }
            }
        }
        if self.state == State::Connected && !blocked {
            while !self.send_buf.is_empty() && self.out.len() < MAX_OUT {
                let len = self.send_buf.len().min(MSS);
                if !self.may_send(len) {
                    blocked = true;
                    break;
                }
                let payload = self.send_buf.split_to(len).freeze();
                self.push(Type::Data, payload);
                self.transmit(self.out.len() - 1, now, out);
            }
            if self.closing && self.send_buf.is_empty() && self.fin_seq.is_none() {
                self.fin_seq = Some(self.seq_nr);
                self.push(Type::Fin, Bytes::new());
                self.transmit(self.out.len() - 1, now, out);
            }
        }
        // Held back by the window (not by the receiver, and not for lack of data).
        self.cwnd_limited =
            blocked && (self.cwnd as usize) < self.peer_wnd || (blocked && self.cwnd_limited);
        if !blocked {
            self.cwnd_limited = false;
        }
        if blocked && self.in_flight == 0 && self.rto_deadline.is_none() {
            // The receiver's window is shut: probe it after a while.
            self.rto_deadline = Some(now + self.rto);
        }
        if self.need_ack && self.state == State::Connected {
            let sack = self.sack();
            let mut buf = Vec::with_capacity(HEADER_LEN + 34);
            packet::encode(
                &self.header(Type::State, self.seq_nr, now),
                sack.as_deref(),
                &[],
                &mut buf,
            );
            out.push(buf);
            self.last_send = now;
        }
        self.need_ack = false;
    }

    // ── Timers ───────────────────────────────────────────────────────────

    pub fn next_timeout(&self) -> Option<Instant> {
        if self.error.is_some() || self.send_reset {
            return None;
        }
        let mut t = self.rto_deadline;
        if self.state == State::Connected {
            for d in [self.last_send + KEEPALIVE, self.last_recv + DEAD_AFTER] {
                t = Some(t.map_or(d, |t| t.min(d)));
            }
        }
        t
    }

    pub fn on_timeout(&mut self, now: Instant) {
        if self.error.is_some() {
            return;
        }
        if self.state == State::Connected && now.duration_since(self.last_recv) >= DEAD_AFTER {
            self.fail(io::ErrorKind::TimedOut);
            return;
        }
        if let Some(d) = self.rto_deadline
            && now >= d
        {
            self.rto_deadline = None;
            if self.state == State::SynSent {
                self.syn_tries += 1;
                let (tries, every) = match self.dial {
                    Dial::Normal => (2, Duration::from_secs(self.syn_tries as u64)),
                    Dial::Holepunch => (9, Duration::from_secs(1)),
                };
                if self.syn_tries > tries {
                    self.fail(io::ErrorKind::TimedOut);
                    return;
                }
                if let Some(syn) = self.out.front_mut()
                    && !syn.need_resend
                {
                    syn.need_resend = true;
                    self.resends += 1;
                }
                self.rto_deadline = Some(now + every);
            } else if !self.out.is_empty() {
                self.timeouts += 1;
                if self.timeouts > MAX_TIMEOUTS {
                    self.fail(io::ErrorKind::TimedOut);
                    return;
                }
                // Everything in flight is presumed lost; start over gently.
                self.ssthresh = (self.cwnd / 2.0).max(MIN_CWND);
                self.cwnd = MSS as f64;
                self.slow_start = true;
                for i in 0..self.out.len() {
                    if !self.out[i].sacked && !self.out[i].need_resend {
                        self.uncount(i);
                        self.out[i].need_resend = true;
                        self.resends += 1;
                    }
                }
                self.loss_seq = self.seq_nr.wrapping_sub(1);
                self.rto = (self.rto * 2).min(MAX_RTO);
                self.rto_deadline = Some(now + self.rto);
                if self.out.iter().all(|p| p.sacked || p.payload.is_empty()) {
                    // Only a FIN outstanding: let it through a closed window.
                    self.probe = true;
                }
                if self.peer_wnd < MSS {
                    self.probe = true;
                }
            } else if !self.send_buf.is_empty() {
                self.probe = true;
            }
        }
        if self.state == State::Connected && now.duration_since(self.last_send) >= KEEPALIVE {
            self.need_ack = true;
        }
    }

    // ── The application's side ───────────────────────────────────────────

    /// Copy received bytes out. `None` means nothing yet (register a waker).
    pub fn read(&mut self, dst: &mut ReadBuf<'_>) -> Option<io::Result<()>> {
        if self.recv_len > 0 {
            let before = self.recv_window();
            while dst.remaining() > 0 {
                let Some(front) = self.recv_buf.front_mut() else {
                    break;
                };
                let n = front.len().min(dst.remaining());
                dst.put_slice(&front[..n]);
                self.recv_len -= n;
                if n == front.len() {
                    self.recv_buf.pop_front();
                } else {
                    let _ = front.split_to(n);
                }
            }
            // Tell a sender we had stalled that there's room again.
            if before < RECV_WINDOW / 4 && self.recv_window() >= RECV_WINDOW / 4 {
                self.need_ack = true;
            }
            return Some(Ok(()));
        }
        if self.eof {
            return Some(Ok(()));
        }
        self.error.map(|e| Err(e.into()))
    }

    pub fn readable(&self) -> bool {
        self.recv_len > 0 || self.eof || self.error.is_some()
    }

    /// Queue bytes to send. `None` means the buffer is full (register a waker).
    pub fn write(&mut self, data: &[u8]) -> Option<io::Result<usize>> {
        if let Some(e) = self.error {
            return Some(Err(e.into()));
        }
        if self.closing {
            return Some(Err(io::ErrorKind::BrokenPipe.into()));
        }
        let room = SEND_BUFFER.saturating_sub(self.send_buf.len());
        if room == 0 || self.state == State::SynSent {
            return None;
        }
        let n = room.min(data.len());
        self.send_buf.extend_from_slice(&data[..n]);
        Some(Ok(n))
    }

    pub fn writable(&self) -> bool {
        self.error.is_some()
            || (self.state == State::Connected && self.send_buf.len() < SEND_BUFFER)
    }

    pub fn connected(&self) -> bool {
        self.state == State::Connected
    }

    /// An ack (e.g. a window update after a read) is waiting to go out.
    pub fn needs_ack(&self) -> bool {
        self.need_ack
    }

    /// No more writes: a FIN follows the queued data.
    pub fn close(&mut self) {
        if self.state == State::SynSent {
            self.abort();
            return;
        }
        self.closing = true;
    }

    /// The application is gone: finish sending, then forget the connection.
    pub fn drop_handle(&mut self) {
        self.dropped = true;
        self.close();
    }

    /// Tear down at once and tell the other end.
    pub fn abort(&mut self) {
        if self.error.is_none() {
            self.send_reset = true;
            self.error = Some(io::ErrorKind::ConnectionAborted);
        }
    }

    /// Nothing left to do: the driver can stop and forget this connection.
    pub fn finished(&self) -> bool {
        if self.send_reset {
            return false;
        }
        self.error.is_some() || (self.fin_acked && (self.eof || self.dropped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two connections joined by a deterministic network that can drop,
    /// delay and reorder packets.
    struct Net {
        now: Instant,
        a: Conn,
        b: Conn,
        /// (deliver at, to b?, datagram)
        wire: Vec<(Instant, bool, Bytes)>,
        delay: Duration,
        rng: u64,
        loss: f64,
        jitter: Duration,
    }

    impl Net {
        fn new(delay_ms: u64, loss: f64, jitter_ms: u64) -> Self {
            let epoch = Instant::now();
            let now = epoch + Duration::from_millis(5);
            let mut a = Conn::connect(1000, epoch, now, Dial::Normal);
            let mut out = Vec::new();
            a.flush(now, &mut out);
            assert_eq!(out.len(), 1, "one SYN");
            let syn = packet::parse(&out[0]).unwrap().h;
            assert_eq!(syn.ty, Type::Syn);
            let b = Conn::accept(&syn, epoch, now);
            let mut net = Self {
                now,
                a,
                b,
                wire: Vec::new(),
                delay: Duration::from_millis(delay_ms),
                rng: 0x9e37_79b9_7f4a_7c15,
                loss,
                jitter: Duration::from_millis(jitter_ms),
            };
            net.flush_b();
            net
        }

        fn rand(&mut self) -> f64 {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            (self.rng % 1_000_000) as f64 / 1_000_000.0
        }

        fn send(&mut self, to_b: bool, dgs: Vec<Vec<u8>>) {
            for d in dgs {
                if self.rand() < self.loss {
                    continue;
                }
                let j = self.jitter.mul_f64(self.rand());
                self.wire
                    .push((self.now + self.delay + j, to_b, Bytes::from(d)));
            }
        }

        fn flush_a(&mut self) {
            let mut out = Vec::new();
            self.a.flush(self.now, &mut out);
            self.send(true, out);
        }

        fn flush_b(&mut self) {
            let mut out = Vec::new();
            self.b.flush(self.now, &mut out);
            self.send(false, out);
        }

        /// Advance to the next event (delivery or timer) and process it.
        fn step(&mut self) -> bool {
            let next_wire = self.wire.iter().map(|w| w.0).min();
            let timers = [self.a.next_timeout(), self.b.next_timeout()];
            let next = timers.into_iter().flatten().chain(next_wire).min();
            let Some(next) = next else { return false };
            self.now = self.now.max(next);
            let now = self.now;
            let (due, rest): (Vec<_>, Vec<_>) = self.wire.drain(..).partition(|w| w.0 <= now);
            self.wire = rest;
            for (_, to_b, d) in due {
                if to_b {
                    self.b.on_datagram(&d, now);
                } else {
                    self.a.on_datagram(&d, now);
                }
            }
            if self.a.next_timeout().is_some_and(|t| t <= now) {
                self.a.on_timeout(now);
            }
            if self.b.next_timeout().is_some_and(|t| t <= now) {
                self.b.on_timeout(now);
            }
            self.flush_a();
            self.flush_b();
            true
        }

        fn read_all(c: &mut Conn, into: &mut Vec<u8>) {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let mut rb = ReadBuf::new(&mut buf);
                match c.read(&mut rb) {
                    Some(Ok(())) if !rb.filled().is_empty() => into.extend_from_slice(rb.filled()),
                    _ => break,
                }
            }
        }
    }

    fn data(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 31 + i / 7) as u8).collect()
    }

    /// Push `len` bytes from a to b (and `back` bytes from b to a) and check
    /// both arrive intact; returns the simulated time it took.
    fn transfer(net: &mut Net, len: usize, back: usize) -> Duration {
        let start = net.now;
        let (sa, sb) = (data(len), data(back));
        let (mut wa, mut wb) = (0, 0);
        let (mut ra, mut rb) = (Vec::new(), Vec::new());
        let mut steps = 0;
        while rb.len() < len || ra.len() < back {
            if wa < len
                && let Some(Ok(n)) = net.a.write(&sa[wa..])
            {
                wa += n;
            }
            if wb < back
                && let Some(Ok(n)) = net.b.write(&sb[wb..])
            {
                wb += n;
            }
            net.flush_a();
            net.flush_b();
            assert!(
                net.step(),
                "stalled at {}/{len} and {}/{back}",
                rb.len(),
                ra.len()
            );
            Net::read_all(&mut net.b, &mut rb);
            Net::read_all(&mut net.a, &mut ra);
            assert!(net.a.error().is_none(), "{:?}", net.a.error());
            assert!(net.b.error().is_none(), "{:?}", net.b.error());
            steps += 1;
            assert!(steps < 5_000_000, "no progress");
        }
        assert!(rb == sa, "a→b corrupted");
        assert!(ra == sb, "b→a corrupted");
        net.now - start
    }

    #[test]
    fn handshake_and_small_exchange() {
        let mut net = Net::new(20, 0.0, 0);
        while !net.a.connected() {
            assert!(net.step());
        }
        transfer(&mut net, 68, 68);
    }

    #[test]
    fn bulk_transfer_clean_line() {
        let mut net = Net::new(25, 0.0, 0);
        let t = transfer(&mut net, 8 << 20, 0);
        // 8 MiB over a 50 ms RTT with no loss: slow start should get there fast.
        assert!(t < Duration::from_secs(10), "{t:?}");
        assert_eq!(net.a.stats.resent_packets, 0);
    }

    #[test]
    fn bulk_transfer_with_loss_and_reordering() {
        let mut net = Net::new(30, 0.03, 15);
        let t = transfer(&mut net, 4 << 20, 256 << 10);
        assert!(t < Duration::from_secs(120), "{t:?}");
        assert!(net.a.stats.resent_packets > 0);
    }

    #[test]
    fn survives_heavy_loss() {
        let mut net = Net::new(40, 0.2, 30);
        transfer(&mut net, 512 << 10, 64 << 10);
    }

    #[test]
    fn graceful_close_both_ways() {
        let mut net = Net::new(10, 0.05, 5);
        transfer(&mut net, 100_000, 50_000);
        net.a.close();
        let mut got = Vec::new();
        while !net.b.eof {
            assert!(net.step());
            Net::read_all(&mut net.b, &mut got);
        }
        assert!(got.is_empty());
        net.b.drop_handle();
        net.a.drop_handle();
        let mut n = 0;
        while !(net.a.finished() && net.b.finished()) {
            assert!(net.step());
            n += 1;
            assert!(n < 100_000);
        }
        assert!(net.a.error().is_none() && net.b.error().is_none());
    }

    #[test]
    fn syn_retries_then_times_out() {
        let epoch = Instant::now();
        let mut now = epoch;
        let mut c = Conn::connect(7, epoch, now, Dial::Normal);
        let mut sent = 0;
        while c.error().is_none() {
            let mut out = Vec::new();
            c.flush(now, &mut out);
            sent += out.len();
            now = c.next_timeout().expect("timer armed");
            c.on_timeout(now);
        }
        assert_eq!(c.error(), Some(io::ErrorKind::TimedOut));
        assert_eq!(sent, 3);
        assert!(now - epoch <= Duration::from_secs(5), "{:?}", now - epoch);
    }

    #[test]
    fn holepunch_dial_keeps_trying() {
        let epoch = Instant::now();
        let mut now = epoch;
        let mut c = Conn::connect(7, epoch, now, Dial::Holepunch);
        let mut sent = 0;
        while c.error().is_none() {
            let mut out = Vec::new();
            c.flush(now, &mut out);
            sent += out.len();
            now = c.next_timeout().unwrap();
            c.on_timeout(now);
        }
        assert_eq!(sent, 10);
    }

    #[test]
    fn reset_refuses_a_dial() {
        let mut net = Net::new(10, 0.0, 0);
        net.b.abort();
        while net.a.error().is_none() {
            assert!(net.step());
        }
        assert_eq!(net.a.error(), Some(io::ErrorKind::ConnectionReset));
    }

    #[test]
    fn dead_peer_times_out() {
        let mut net = Net::new(10, 0.0, 0);
        transfer(&mut net, 1000, 0);
        // b vanishes: nothing it sends arrives any more.
        net.loss = 1.0;
        let _ = net.a.write(&[1; 5000]);
        net.flush_a();
        while net.a.error().is_none() {
            assert!(net.step());
        }
        assert_eq!(net.a.error(), Some(io::ErrorKind::TimedOut));
    }

    #[test]
    fn delay_history_tracks_queueing() {
        let mut h = DelayHist::default();
        let t = Instant::now();
        // Clock offset near the wrap point; base delay 20 ms, then 80 ms of queue.
        let off = u32::MAX - 5_000;
        h.add(off.wrapping_add(20_000), t);
        assert_eq!(h.queueing_us(), Some(0));
        for _ in 0..3 {
            h.add(off.wrapping_add(100_000), t);
        }
        assert_eq!(h.queueing_us(), Some(80_000));
    }

    #[test]
    fn sack_bitmask_layout() {
        let epoch = Instant::now();
        let syn = Header {
            ty: Type::Syn,
            conn_id: 5,
            ts: 0,
            ts_diff: 0,
            wnd: 1 << 20,
            seq: 100,
            ack: 0,
        };
        let mut b = Conn::accept(&syn, epoch, epoch);
        let mut send = |seq: u16| {
            let h = Header {
                ty: Type::Data,
                conn_id: 6,
                ts: 1,
                ts_diff: 0,
                wnd: 1 << 20,
                seq,
                ack: b.seq_nr.wrapping_sub(1),
            };
            let mut buf = Vec::new();
            packet::encode(&h, None, b"x", &mut buf);
            b.on_datagram(&Bytes::from(buf), epoch);
        };
        // 101 is missing; 102 and 110 arrived.
        send(102);
        send(110);
        let mut out = Vec::new();
        b.flush(epoch, &mut out);
        let ack = packet::parse(out.last().unwrap()).unwrap();
        assert_eq!(ack.h.ack, 100);
        // Bit 0 is ack+2 = 102, bit 8 is 110.
        assert_eq!(ack.sack, Some(&[0b1, 0b1, 0, 0][..]));
    }
}
