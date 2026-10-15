//! NAT hole punching (BEP 55).
//!
//! Most peers sit behind NATs that drop connections nobody asked for, so two
//! such peers can't reach each other directly — unless they send at the same
//! moment: each side's outgoing packet makes its own NAT expect the other's
//! reply. A peer connected to both of them (the relay) sets that up: we ask it
//! to introduce us (`rendezvous`), it tells both sides to `connect`, and both
//! dial over µTP at once, retrying every second until the holes line up. We
//! play relay for other peers the same way.

use super::*;
use crate::peer::extension::{HP_NO_SELF, HP_NO_SUPPORT, HP_NOT_CONNECTED, HolepunchMsg};
use crate::utp::Dial;

/// Hole-punch dials in flight per torrent.
const MAX_PUNCHING: usize = 24;
/// Introductions one peer may ask of us per minute.
const RELAYS_PER_MINUTE: u32 = 30;
/// Attempts per unreachable peer, at least this far apart.
const MAX_PUNCHES: u8 = 3;
const PUNCH_AGAIN_AFTER: Duration = Duration::from_secs(300);

impl PeerState {
    /// Where the other side of an introduction should send to reach this
    /// peer: over µTP, the exact address its NAT gave the connection; over
    /// TCP, its listen port (NATs mostly keep that number for UDP too).
    fn punch_endpoint(&self) -> SocketAddr {
        if self.utp {
            self.addr
        } else {
            self.dialable().unwrap_or(self.addr)
        }
    }

    fn is_at(&self, addr: SocketAddr) -> bool {
        self.addr == addr || self.dialable() == Some(addr)
    }
}

impl Torrent {
    fn punch_room(&self, st: &State) -> bool {
        let s = self.ctx.settings.read();
        let private = st.meta.as_ref().is_some_and(|m| m.info.private);
        st.phase.is_active()
            && !private
            && st.punching.len() < MAX_PUNCHING
            && st.peers.len() + st.connecting < s.max_peers_per_torrent
            && self.ctx.connections.load(Ordering::Relaxed) < s.max_peers_global
    }

    /// `addr` didn't answer: if a connected peer told us about it and both
    /// speak ut_holepunch, ask that peer to introduce us, and start dialling.
    pub(super) fn try_holepunch(self: &Arc<Self>, st: &mut State, addr: SocketAddr) -> bool {
        if self.ctx.utp().is_none() || !self.punch_room(st) {
            return false;
        }
        let now = Instant::now();
        let Some(c) = st.candidates.get(&addr) else {
            return false;
        };
        if c.flags & PEX_HOLEPUNCH == 0
            || c.connected
            || c.connecting
            || c.punches >= MAX_PUNCHES
            || c.last_punch.is_some_and(|t| now - t < PUNCH_AGAIN_AFTER)
        {
            return false;
        }
        let relay = c.relays.iter().rev().find_map(|k| {
            let p = st.peers.get(k)?;
            Some((p.tx.clone(), p.ext_holepunch?))
        });
        let Some((tx, id)) = relay else {
            return false;
        };
        let _ = tx.send(PeerCmd::Ext(id, HolepunchMsg::Rendezvous(addr).encode()));
        if let Some(c) = st.candidates.get_mut(&addr) {
            c.punches += 1;
            c.last_punch = Some(now);
        }
        // Dial at once: our first packets open our own NAT for the replies.
        st.punching.insert(addr.ip(), now);
        debug!("{}: hole punching {addr}", st.name);
        self.dial(st, addr, Route::Utp(Dial::Holepunch));
        true
    }

    /// A relay introduced us to `addr` (because we or it asked): dial it now.
    pub(super) fn on_holepunch_connect(self: &Arc<Self>, st: &mut State, addr: SocketAddr) {
        if self.ctx.utp().is_none() || !self.punch_room(st) {
            return;
        }
        let addr = SocketAddr::new(addr.ip().to_canonical(), addr.port());
        if crate::torrent::unreachable(addr, PeerSource::Pex) || st.banned.contains(&addr.ip()) {
            return;
        }
        if st.peers.values().any(|p| p.is_at(addr)) {
            return;
        }
        let now = Instant::now();
        // Our own rendezvous already started a dial there.
        if st
            .punching
            .get(&addr.ip())
            .is_some_and(|t| now - *t < Duration::from_secs(12))
        {
            return;
        }
        add_candidate(st, addr, PeerSource::Pex);
        if st.candidates.get(&addr).is_none_or(|c| c.connecting) {
            return;
        }
        st.punching.insert(addr.ip(), now);
        debug!("{}: introduced to {addr}, dialling", st.name);
        self.dial(st, addr, Route::Utp(Dial::Holepunch));
    }

    /// Peer `from` asks us to introduce it to `target`.
    pub(crate) fn relay_holepunch(&self, from: PeerKey, target: SocketAddr) {
        let target = SocketAddr::new(target.ip().to_canonical(), target.port());
        let mut st = self.st.lock();
        let private = st.meta.as_ref().is_some_and(|m| m.info.private);
        let Some(a) = st.peers.get_mut(&from) else {
            return;
        };
        let Some(a_id) = a.ext_holepunch else {
            return;
        };
        if private {
            return;
        }
        let now = Instant::now();
        if now.duration_since(a.relayed.0) >= Duration::from_secs(60) {
            a.relayed = (now, 0);
        }
        a.relayed.1 += 1;
        if a.relayed.1 > RELAYS_PER_MINUTE {
            return;
        }
        let a_tx = a.tx.clone();
        let a_end = a.punch_endpoint();
        let reply = |msg: HolepunchMsg| {
            let _ = a_tx.send(PeerCmd::Ext(a_id, msg.encode()));
        };
        if st.peers[&from].is_at(target) {
            reply(HolepunchMsg::Error(target, HP_NO_SELF));
            return;
        }
        let Some(b) = st.peers.values().find(|p| p.is_at(target) && !p.kicked) else {
            reply(HolepunchMsg::Error(target, HP_NOT_CONNECTED));
            return;
        };
        let Some(b_id) = b.ext_holepunch else {
            reply(HolepunchMsg::Error(target, HP_NO_SUPPORT));
            return;
        };
        if a_end.is_ipv4() != b.punch_endpoint().is_ipv4() {
            reply(HolepunchMsg::Error(target, HP_NOT_CONNECTED));
            return;
        }
        let _ =
            b.tx.send(PeerCmd::Ext(b_id, HolepunchMsg::Connect(a_end).encode()));
        reply(HolepunchMsg::Connect(b.punch_endpoint()));
        debug!("{}: introduced {a_end} to {target}", st.name);
    }

    /// Of two connections to the same peer, one each way, whether to keep
    /// ours (the outgoing one). libtorrent's rule, so both ends drop the same
    /// one: the side with the lower listen port keeps its outgoing connection.
    pub(super) fn keep_outgoing(&self, hs: &Handshake, outgoing_addr: SocketAddr) -> bool {
        let ours = self.ctx.listen_port.load(Ordering::Relaxed);
        let theirs = outgoing_addr.port();
        if ours == theirs && hs.peer_id.starts_with(b"-TV") {
            // Two Travs on the same port: the lower peer id keeps its outgoing.
            return self.ctx.peer_id < hs.peer_id;
        }
        ours <= theirs
    }
}
