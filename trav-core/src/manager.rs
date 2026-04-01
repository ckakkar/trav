use std::collections::{HashMap, HashSet};
use sha1::{Sha1, Digest};
use crate::error::{BitTorrentError, Result};
use tracing::info;

pub struct PieceManager {
    num_pieces: u32,
    piece_length: u32,
    total_length: u64,
    pieces_hash: Vec<u8>,
    
    /// Count of peers holding each piece. Index corresponds to piece_index.
    availability: Vec<u32>,
    /// Set of pieces successfully downloaded and verified.
    downloaded: HashSet<u32>,
    /// Set of pieces currently being downloaded.
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

    /// Called when a peer sends a BITFIELD message.
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

    /// Called when a peer sends a HAVE message.
    pub fn handle_have(&mut self, piece_index: u32) {
        if piece_index < self.num_pieces {
            self.availability[piece_index as usize] += 1;
        }
    }

    /// Selects the next piece to download using the Rarest-First algorithm.
    pub fn pick_rarest_piece(&mut self) -> Option<u32> {
        let mut rarest_index = None;
        let mut min_availability = u32::MAX;

        for i in 0..self.num_pieces {
            if self.downloaded.contains(&i) || self.in_progress.contains(&i) {
                continue;
            }

            let avail = self.availability[i as usize];
            if avail > 0 && avail < min_availability {
                min_availability = avail;
                rarest_index = Some(i);
            }
        }

        if let Some(index) = rarest_index {
            self.in_progress.insert(index);
        }

        rarest_index
    }

    /// Verifies the SHA-1 hash of a fully assembled piece.
    pub fn verify_piece(&mut self, piece_index: u32, piece_data: &[u8]) -> bool {
        let offset = (piece_index * 20) as usize;
        if offset + 20 > self.pieces_hash.len() {
            return false;
        }

        let expected_hash = &self.pieces_hash[offset..offset + 20];
        
        let mut hasher = Sha1::new();
        hasher.update(piece_data);
        let actual_hash = hasher.finalize();

        if expected_hash == actual_hash.as_slice() {
            self.downloaded.insert(piece_index);
            self.in_progress.remove(&piece_index);
            info!("Piece {} verified successfully.", piece_index);
            true
        } else {
            self.in_progress.remove(&piece_index);
            info!("Piece {} failed hash check.", piece_index);
            false
        }
    }

    pub fn is_complete(&self) -> bool {
        self.downloaded.len() == self.num_pieces as usize
    }
}
