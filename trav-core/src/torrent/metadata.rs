//! Assembling the info dictionary from peers via ut_metadata (BEP 9).

use std::time::{Duration, Instant};

use bytes::Bytes;

use crate::peer::extension::METADATA_PIECE;
use crate::picker::PeerKey;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(6);
/// Once every piece is out, a piece waited on this long is also asked of
/// another peer: one slow peer shouldn't hold up the whole torrent.
const DUPLICATE_AFTER: Duration = Duration::from_secs(2);

pub(crate) struct MetadataAssembly {
    pub size: usize,
    pieces: Vec<Option<Bytes>>,
    requested: Vec<Option<(PeerKey, Instant)>>,
}

impl MetadataAssembly {
    pub fn new(size: usize) -> Self {
        let n = size.div_ceil(METADATA_PIECE);
        Self {
            size,
            pieces: vec![None; n],
            requested: vec![None; n],
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

    pub fn reject(&mut self, piece: u32) {
        if let Some(r) = self.requested.get_mut(piece as usize) {
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
    pub fn on_data(&mut self, piece: u32, data: Bytes) -> Option<Vec<u8>> {
        let i = piece as usize;
        if i >= self.pieces.len() || data.len() != self.piece_len(i) {
            return None;
        }
        self.pieces[i] = Some(data);
        if self.pieces.iter().all(Option::is_some) {
            let mut out = Vec::with_capacity(self.size);
            for p in self.pieces.iter().flatten() {
                out.extend_from_slice(p);
            }
            return Some(out);
        }
        None
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
        assert!(m.on_data(1, Bytes::from(vec![0; METADATA_PIECE])).is_none());
        assert!(m.on_data(0, Bytes::from(vec![0; METADATA_PIECE])).is_some());
    }
}
