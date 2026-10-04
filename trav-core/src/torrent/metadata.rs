//! Assembling the info dictionary from peers via ut_metadata (BEP 9).

use std::time::{Duration, Instant};

use bytes::Bytes;

use crate::peer::extension::METADATA_PIECE;
use crate::picker::PeerKey;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

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

    /// Next piece this peer should ask for (unrequested, or stale).
    pub fn next_request(&mut self, peer: PeerKey) -> Option<u32> {
        let now = Instant::now();
        let idx = (0..self.pieces.len()).find(|&i| {
            self.pieces[i].is_none()
                && match self.requested[i] {
                    None => true,
                    Some((_, at)) => now.duration_since(at) > REQUEST_TIMEOUT,
                }
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
