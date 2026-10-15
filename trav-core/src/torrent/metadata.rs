//! Assembling the info dictionary from peers via ut_metadata (BEP 9).
//!
//! Each peer says how big the dictionary is, and pieces are only taken from
//! peers that agree on it: one assembly per advertised size. A peer that
//! reports the wrong size can only spoil its own assembly, never the one the
//! honest peers are filling. One whose pieces fail the hash check starts that
//! assembly over, with the peers that sent them sitting out for a while so
//! the next round mixes in others.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use bytes::Bytes;

use crate::peer::extension::METADATA_PIECE;
use crate::picker::PeerKey;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(6);
/// Once every piece is out, a piece waited on this long is also asked of
/// another peer: one slow peer shouldn't hold up the whole torrent.
const DUPLICATE_AFTER: Duration = Duration::from_secs(2);
/// Different sizes we assemble at once (each peer advertises one).
const MAX_SIZES: usize = 4;
/// How long a peer that sent part of a dictionary which failed its hash
/// check sits out (random within this range, as libtorrent does).
const PENALTY_SECS: std::ops::Range<u64> = 5..30;

pub(crate) struct MetadataAssembly {
    pub size: usize,
    pieces: Vec<Option<Bytes>>,
    requested: Vec<Option<(PeerKey, Instant)>>,
    /// Who sent each piece we hold.
    sources: Vec<Option<PeerKey>>,
    /// Handed out whole and waiting for its hash check.
    delivered: bool,
    /// Peers not asked again until then.
    benched: HashMap<PeerKey, Instant>,
}

impl MetadataAssembly {
    pub fn new(size: usize) -> Self {
        let n = size.div_ceil(METADATA_PIECE);
        Self {
            size,
            pieces: vec![None; n],
            requested: vec![None; n],
            sources: vec![None; n],
            delivered: false,
            benched: HashMap::new(),
        }
    }

    pub fn progress(&self) -> f64 {
        self.pieces.iter().filter(|p| p.is_some()).count() as f64 / self.pieces.len().max(1) as f64
    }

    fn piece_len(&self, i: usize) -> usize {
        if i + 1 == self.pieces.len() {
            self.size - i * METADATA_PIECE
        } else {
            METADATA_PIECE
        }
    }

    /// Next piece this peer should ask for: unrequested or stale first, then
    /// (endgame) the one another peer has kept us waiting on longest.
    pub fn next_request(&mut self, peer: PeerKey) -> Option<u32> {
        let now = Instant::now();
        if self.delivered || self.benched.get(&peer).is_some_and(|&until| now < until) {
            return None;
        }
        let missing = || (0..self.pieces.len()).filter(|&i| self.pieces[i].is_none());
        let fresh = missing().find(|&i| match self.requested[i] {
            None => true,
            Some((_, at)) => now.duration_since(at) > REQUEST_TIMEOUT,
        });
        let idx = fresh.or_else(|| {
            missing()
                .filter_map(|i| self.requested[i].map(|r| (i, r)))
                .filter(|&(_, (p, at))| p != peer && now.duration_since(at) > DUPLICATE_AFTER)
                .min_by_key(|&(_, (_, at))| at)
                .map(|(i, _)| i)
        })?;
        self.requested[idx] = Some((peer, now));
        Some(idx as u32)
    }

    /// `peer` doesn't have `piece` after all.
    pub fn reject(&mut self, piece: u32, peer: PeerKey) {
        if let Some(r) = self.requested.get_mut(piece as usize)
            && matches!(r, Some((p, _)) if *p == peer)
        {
            *r = None;
        }
    }

    pub fn release_peer(&mut self, peer: PeerKey) {
        for r in self.requested.iter_mut() {
            if matches!(r, Some((p, _)) if *p == peer) {
                *r = None;
            }
        }
    }

    /// Store a piece; returns the full info dict once every piece is present.
    pub fn on_data(&mut self, piece: u32, data: Bytes, from: PeerKey) -> Option<Vec<u8>> {
        let i = piece as usize;
        if self.delivered || i >= self.pieces.len() || data.len() != self.piece_len(i) {
            return None;
        }
        self.pieces[i] = Some(data);
        self.sources[i] = Some(from);
        if self.pieces.iter().all(Option::is_some) {
            self.delivered = true;
            let mut out = Vec::with_capacity(self.size);
            for p in self.pieces.iter().flatten() {
                out.extend_from_slice(p);
            }
            return Some(out);
        }
        None
    }

    /// The assembled dictionary failed its hash check: start over, with the
    /// peers that sent it sitting out a while.
    fn restart(&mut self) {
        let now = Instant::now();
        for p in self.sources.iter().flatten() {
            let secs = rand::random_range(PENALTY_SECS);
            self.benched.insert(*p, now + Duration::from_secs(secs));
        }
        self.benched.retain(|_, &mut until| until > now);
        let n = self.pieces.len();
        self.pieces = vec![None; n];
        self.requested = vec![None; n];
        self.sources = vec![None; n];
        self.delivered = false;
    }
}

/// The assemblies in progress, one per size peers have advertised.
#[derive(Default)]
pub(crate) struct Metadata {
    by_size: Vec<MetadataAssembly>,
}

impl Metadata {
    /// A peer says the dictionary is `size` bytes: assemble that size too.
    pub fn offer(&mut self, size: usize) {
        if self.by_size.len() < MAX_SIZES && !self.by_size.iter().any(|a| a.size == size) {
            self.by_size.push(MetadataAssembly::new(size));
        }
    }

    /// The assembly for peers that advertised `size`.
    pub fn get(&mut self, size: usize) -> Option<&mut MetadataAssembly> {
        self.by_size.iter_mut().find(|a| a.size == size)
    }

    pub fn progress(&self) -> f64 {
        self.by_size
            .iter()
            .map(MetadataAssembly::progress)
            .fold(0.0, f64::max)
    }

    pub fn release_peer(&mut self, peer: PeerKey) {
        for a in &mut self.by_size {
            a.release_peer(peer);
        }
    }

    /// The `size`-byte dictionary didn't hash to the info-hash: try again.
    pub fn failed(&mut self, size: usize) {
        if let Some(a) = self.get(size) {
            a.restart();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_pieces_are_asked_of_another_peer() {
        let mut m = MetadataAssembly::new(METADATA_PIECE * 2);
        assert_eq!(m.next_request(1), Some(0));
        assert_eq!(m.next_request(2), Some(1));
        // Everything is out and nothing is overdue yet.
        assert_eq!(m.next_request(3), None);
        let past = Instant::now() - DUPLICATE_AFTER - Duration::from_millis(10);
        m.requested[1] = Some((2, past));
        assert_eq!(m.next_request(3), Some(1), "piece 1 duplicated to peer 3");
        // Never duplicated back to the peer already asked.
        m.requested[0] = Some((1, past));
        assert_eq!(m.next_request(1), None);
        assert!(
            m.on_data(1, Bytes::from(vec![0; METADATA_PIECE]), 2)
                .is_none()
        );
        assert!(
            m.on_data(0, Bytes::from(vec![0; METADATA_PIECE]), 1)
                .is_some()
        );
        // Handed out once; nothing more until its hash check says otherwise.
        assert!(
            m.on_data(0, Bytes::from(vec![0; METADATA_PIECE]), 1)
                .is_none()
        );
    }

    #[test]
    fn a_peer_with_the_wrong_size_cant_spoil_the_others() {
        let real = METADATA_PIECE + 100;
        let mut md = Metadata::default();
        // The liar speaks first, then the honest peers.
        md.offer(real * 3);
        md.offer(real);
        md.offer(real);
        let honest = md.get(real).unwrap();
        assert_eq!(honest.next_request(2), Some(0));
        assert_eq!(honest.next_request(3), Some(1));
        assert!(
            honest
                .on_data(0, Bytes::from(vec![1; METADATA_PIECE]), 2)
                .is_none()
        );
        let done = honest.on_data(1, Bytes::from(vec![2; 100]), 3).unwrap();
        assert_eq!(done.len(), real);
        // The liar's own assembly can't take the honest pieces.
        let liar = md.get(real * 3).unwrap();
        assert!(liar.on_data(1, Bytes::from(vec![2; 100]), 3).is_none());
    }

    #[test]
    fn a_bad_dictionary_starts_over_without_its_senders() {
        let mut md = Metadata::default();
        md.offer(METADATA_PIECE * 2);
        let a = md.get(METADATA_PIECE * 2).unwrap();
        a.next_request(1);
        a.next_request(2);
        a.on_data(0, Bytes::from(vec![0; METADATA_PIECE]), 1);
        assert!(
            a.on_data(1, Bytes::from(vec![0; METADATA_PIECE]), 2)
                .is_some()
        );
        md.failed(METADATA_PIECE * 2);
        let a = md.get(METADATA_PIECE * 2).unwrap();
        assert_eq!(a.progress(), 0.0);
        // Those who sent the bad pieces sit out; anyone else may ask at once.
        assert_eq!(a.next_request(1), None);
        assert_eq!(a.next_request(2), None);
        assert_eq!(a.next_request(3), Some(0));
        assert_eq!(a.next_request(4), Some(1));
    }
}
