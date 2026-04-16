use std::collections::HashSet;
use sha1::{Sha1, Digest};
use tracing::info;

pub struct PieceManager {
    pub num_pieces: u32,
    pub piece_length: u32,
    pub total_length: u64,
    pieces_hash: Vec<u8>,
    availability: Vec<u32>,
    downloaded: HashSet<u32>,
    in_progress: HashSet<u32>,
}

impl PieceManager {
    pub fn new(num_pieces: u32, piece_length: u32, total_length: u64, pieces_hash: Vec<u8>) -> Self {
        Self {
            num_pieces,
            piece_length,
            total_length,
            pieces_hash,
            availability: vec![0; num_pieces as usize],
            downloaded: HashSet::new(),
            in_progress: HashSet::new(),
        }
    }

    pub fn handle_bitfield(&mut self, bitfield: &[u8]) {
        for (byte_idx, &byte) in bitfield.iter().enumerate() {
            for bit_idx in 0..8 {
                if (byte & (1 << (7 - bit_idx))) != 0 {
                    let piece_idx = (byte_idx * 8 + bit_idx) as u32;
                    if piece_idx < self.num_pieces {
                        self.availability[piece_idx as usize] += 1;
                    }
                }
            }
        }
    }

    pub fn handle_have(&mut self, piece_index: u32) {
        if piece_index < self.num_pieces {
            self.availability[piece_index as usize] += 1;
        }
    }

    pub fn pick_rarest_piece(&mut self) -> Option<u32> {
        let mut rarest = None;
        let mut min_avail = u32::MAX;

        for i in 0..self.num_pieces {
            if self.downloaded.contains(&i) || self.in_progress.contains(&i) {
                continue;
            }
            let avail = self.availability[i as usize];
            if avail > 0 && avail < min_avail {
                min_avail = avail;
                rarest = Some(i);
            }
        }

        if let Some(idx) = rarest {
            self.in_progress.insert(idx);
        }
        rarest
    }

    pub fn expected_hash(&self, piece_index: u32) -> Option<[u8; 20]> {
        let offset = (piece_index * 20) as usize;
        if offset + 20 > self.pieces_hash.len() {
            return None;
        }
        let mut hash = [0u8; 20];
        hash.copy_from_slice(&self.pieces_hash[offset..offset + 20]);
        Some(hash)
    }

    pub fn mark_piece_verification(&mut self, piece_index: u32, verified: bool) {
        self.in_progress.remove(&piece_index);
        if verified {
            self.downloaded.insert(piece_index);
            info!("Piece {} verified OK ({}/{})", piece_index, self.downloaded.len(), self.num_pieces);
        } else {
            info!("Piece {} hash check failed", piece_index);
        }
    }

    pub fn mark_piece_timed_out(&mut self, piece_index: u32) {
        self.in_progress.remove(&piece_index);
    }

    pub fn piece_size(&self, piece_index: u32) -> Option<u32> {
        if piece_index >= self.num_pieces {
            return None;
        }
        if piece_index + 1 == self.num_pieces {
            let full_before_last = self.piece_length as u64 * self.num_pieces.saturating_sub(1) as u64;
            let remainder = self.total_length.saturating_sub(full_before_last);
            return Some(remainder as u32);
        }
        Some(self.piece_length)
    }

    pub fn downloaded_count(&self) -> usize {
        self.downloaded.len()
    }

    pub fn is_complete(&self) -> bool {
        self.downloaded.len() == self.num_pieces as usize
    }

    /// Returns the bitfield as raw bytes (big-endian bit order per BEP 3).
    pub fn bitfield_bytes(&self) -> Vec<u8> {
        let num_bytes = (self.num_pieces as usize + 7) / 8;
        let mut bits = vec![0u8; num_bytes];
        for &i in &self.downloaded {
            let byte_idx = i as usize / 8;
            let bit_idx = 7 - (i as usize % 8);
            if byte_idx < bits.len() {
                bits[byte_idx] |= 1 << bit_idx;
            }
        }
        bits
    }

    pub fn verify_piece_data(expected_hash: &[u8; 20], piece_data: &[u8]) -> bool {
        let mut hasher = Sha1::new();
        hasher.update(piece_data);
        let actual = hasher.finalize();
        expected_hash == actual.as_slice()
    }
}
