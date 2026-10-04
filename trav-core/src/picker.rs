//! Piece/block picker: rarest-first (or sequential) with per-file priorities,
//! shared partial pieces across peers, and end-game duplication.
//!
//! The picker owns all in-flight piece buffers. Peers ask it for block
//! requests filtered by *their* bitfield and hand received blocks back; it
//! reports when a piece is fully assembled so the torrent can hash and persist
//! it off the reactor.

use std::collections::{HashMap, HashSet};

use crate::bitfield::Bitfield;
use crate::metainfo::Info;

pub type PeerKey = u64;
pub const BLOCK_SIZE: u32 = 16 * 1024;
/// Total bytes of piece buffers we keep in flight before refusing to open new pieces.
const PARTIAL_BUDGET: u64 = 256 * 1024 * 1024;
/// End-game: at most this many peers race for the same block.
const ENDGAME_DUP: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockReq {
    pub piece: u32,
    pub begin: u32,
    pub len: u32,
}

#[derive(Debug, Clone)]
enum Block {
    Free,
    Requested(Vec<PeerKey>),
    Done,
}

#[derive(Debug)]
struct Partial {
    blocks: Vec<Block>,
    data: Vec<u8>,
    done: u32,
    contributors: Vec<PeerKey>,
}

impl Partial {
    fn new(size: u32) -> Self {
        Self {
            blocks: vec![Block::Free; size.div_ceil(BLOCK_SIZE) as usize],
            data: vec![0; size as usize],
            done: 0,
            contributors: Vec::new(),
        }
    }

    fn idle(&self) -> bool {
        self.done == 0 && self.blocks.iter().all(|b| matches!(b, Block::Free))
    }
}

#[derive(Debug)]
pub enum OnBlock {
    /// Not something we asked for (wrong size/offset, already have the piece, ...).
    Rejected,
    /// Already received from another peer during end-game.
    Duplicate,
    Accepted { cancel: Vec<PeerKey> },
    Complete { piece: u32, data: Vec<u8>, contributors: Vec<PeerKey>, cancel: Vec<PeerKey> },
}

pub struct Picker {
    piece_length: u32,
    total_length: u64,
    have: Bitfield,
    /// Per-piece priority: 0 = skip, 1 = normal, 2+ = high.
    priority: Vec<u8>,
    availability: Vec<u32>,
    partial: HashMap<u32, Partial>,
    verifying: HashSet<u32>,
    sequential: bool,
    max_partial: usize,
    seed: u32,
}

impl Picker {
    pub fn new(info: &Info, have: Bitfield) -> Self {
        let n = info.num_pieces();
        Self {
            piece_length: info.piece_length,
            total_length: info.total_length,
            have,
            priority: vec![1; n],
            availability: vec![0; n],
            partial: HashMap::new(),
            verifying: HashSet::new(),
            sequential: false,
            max_partial: (PARTIAL_BUDGET / info.piece_length as u64).clamp(8, 1024) as usize,
            seed: rand::random(),
        }
    }

    #[inline]
    pub fn num_pieces(&self) -> usize {
        self.have.len()
    }

    pub fn piece_size(&self, i: u32) -> u32 {
        let start = self.piece_length as u64 * i as u64;
        (self.total_length - start).min(self.piece_length as u64) as u32
    }

    pub fn have(&self) -> &Bitfield {
        &self.have
    }

    pub fn set_sequential(&mut self, on: bool) {
        self.sequential = on;
    }

    pub fn sequential(&self) -> bool {
        self.sequential
    }

    pub fn set_piece_priorities(&mut self, prio: Vec<u8>) {
        debug_assert_eq!(prio.len(), self.priority.len());
        self.priority = prio;
    }

    #[inline]
    fn wanted(&self, i: usize) -> bool {
        self.priority[i] > 0 && !self.have.get(i) && !self.verifying.contains(&(i as u32))
    }

    /// Every selected piece is on disk and verified.
    pub fn wanted_complete(&self) -> bool {
        (0..self.num_pieces()).all(|i| self.priority[i] == 0 || self.have.get(i))
    }

    pub fn is_complete(&self) -> bool {
        self.have.all()
    }

    /// Bytes of selected pieces not yet verified (tracker `left`, ETA).
    pub fn wanted_bytes_left(&self) -> u64 {
        (0..self.num_pieces())
            .filter(|&i| self.priority[i] > 0 && !self.have.get(i))
            .map(|i| self.piece_size(i as u32) as u64)
            .sum()
    }

    pub fn wanted_bytes_total(&self) -> u64 {
        (0..self.num_pieces())
            .filter(|&i| self.priority[i] > 0)
            .map(|i| self.piece_size(i as u32) as u64)
            .sum()
    }

    pub fn have_bytes(&self) -> u64 {
        self.have.iter_ones().map(|i| self.piece_size(i as u32) as u64).sum()
    }

    /// Bytes sitting in partially assembled pieces (for a smoother progress readout).
    pub fn partial_bytes(&self) -> u64 {
        self.partial.values().map(|p| p.done as u64 * BLOCK_SIZE as u64).sum()
    }

    pub fn in_progress(&self) -> impl Iterator<Item = u32> + '_ {
        self.partial.keys().copied().chain(self.verifying.iter().copied())
    }

    pub fn is_interesting(&self, peer: &Bitfield) -> bool {
        peer.iter_ones().any(|i| i < self.num_pieces() && self.wanted(i))
    }

    pub fn is_wanted(&self, piece: u32) -> bool {
        (piece as usize) < self.num_pieces() && self.wanted(piece as usize)
    }

    pub fn add_bitfield(&mut self, bf: &Bitfield) {
        for i in bf.iter_ones() {
            if let Some(a) = self.availability.get_mut(i) {
                *a += 1;
            }
        }
    }

    pub fn remove_bitfield(&mut self, bf: &Bitfield) {
        for i in bf.iter_ones() {
            if let Some(a) = self.availability.get_mut(i) {
                *a = a.saturating_sub(1);
            }
        }
    }

    pub fn add_have(&mut self, piece: u32) {
        if let Some(a) = self.availability.get_mut(piece as usize) {
            *a += 1;
        }
    }

    /// Swarm health: min copies + fraction of pieces above the minimum.
    pub fn distributed_copies(&self) -> f64 {
        let Some(&min) = self.availability.iter().min() else { return 0.0 };
        let above = self.availability.iter().filter(|&&a| a > min).count();
        min as f64 + above as f64 / self.availability.len().max(1) as f64
    }

    /// Fill `out` with up to `max` block requests that `peer` (owning `peer_has`) can serve.
    pub fn pick(&mut self, peer: PeerKey, peer_has: &Bitfield, max: usize, out: &mut Vec<BlockReq>) {
        if max == 0 {
            return;
        }
        let start_len = out.len();
        let target = start_len + max;

        // 1. Finish what is already open — keeps the partial set small.
        let mut open: Vec<u32> = self.partial.keys().copied().filter(|&p| peer_has.get(p as usize)).collect();
        if self.sequential {
            open.sort_unstable();
        }
        for p in open {
            self.take_free_blocks(p, peer, target, out);
            if out.len() >= target {
                return;
            }
        }

        // 2. Open new pieces.
        while out.len() < target && self.partial.len() < self.max_partial {
            let Some(p) = self.choose_new_piece(peer_has) else { break };
            self.partial.insert(p, Partial::new(self.piece_size(p)));
            self.take_free_blocks(p, peer, target, out);
        }

        // 3. End-game: nothing left to hand out — race outstanding blocks.
        if out.len() == start_len {
            self.pick_endgame(peer, peer_has, target, out);
        }
    }

    fn take_free_blocks(&mut self, p: u32, peer: PeerKey, target: usize, out: &mut Vec<BlockReq>) {
        let size = self.piece_size(p);
        let Some(part) = self.partial.get_mut(&p) else { return };
        for (bi, b) in part.blocks.iter_mut().enumerate() {
            if out.len() >= target {
                break;
            }
            if matches!(b, Block::Free) {
                *b = Block::Requested(vec![peer]);
                let begin = bi as u32 * BLOCK_SIZE;
                out.push(BlockReq { piece: p, begin, len: (size - begin).min(BLOCK_SIZE) });
            }
        }
    }

    fn choose_new_piece(&self, peer_has: &Bitfield) -> Option<u32> {
        let mut best: Option<(u8, u32, u32, u32)> = None; // (prio, avail, tiebreak, idx)
        for i in peer_has.iter_ones() {
            if i >= self.num_pieces() || !self.wanted(i) || self.partial.contains_key(&(i as u32)) {
                continue;
            }
            let prio = self.priority[i];
            let (avail, tie) = if self.sequential {
                (0, i as u32)
            } else {
                (self.availability[i], (i as u32).wrapping_mul(2_654_435_761) ^ self.seed)
            };
            let better = match best {
                None => true,
                Some((bp, ba, bt, _)) => prio > bp || (prio == bp && (avail, tie) < (ba, bt)),
            };
            if better {
                best = Some((prio, avail, tie, i as u32));
                if self.sequential && prio == u8::MAX {
                    break;
                }
            }
        }
        best.map(|b| b.3)
    }

    fn pick_endgame(&mut self, peer: PeerKey, peer_has: &Bitfield, target: usize, out: &mut Vec<BlockReq>) {
        let mut pieces: Vec<u32> = self.partial.keys().copied().filter(|&p| peer_has.get(p as usize)).collect();
        pieces.sort_unstable();
        for p in pieces {
            let size = self.piece_size(p);
            let part = self.partial.get_mut(&p).expect("listed above");
            for (bi, b) in part.blocks.iter_mut().enumerate() {
                if out.len() >= target {
                    return;
                }
                if let Block::Requested(peers) = b {
                    if peers.len() < ENDGAME_DUP && !peers.contains(&peer) {
                        peers.push(peer);
                        let begin = bi as u32 * BLOCK_SIZE;
                        out.push(BlockReq { piece: p, begin, len: (size - begin).min(BLOCK_SIZE) });
                    }
                }
            }
        }
    }

    pub fn on_block(&mut self, peer: PeerKey, piece: u32, begin: u32, data: &[u8]) -> OnBlock {
        if begin % BLOCK_SIZE != 0 {
            return OnBlock::Rejected;
        }
        let size = self.piece_size_checked(piece);
        let Some(part) = self.partial.get_mut(&piece) else {
            return if self.have.get(piece as usize) || self.verifying.contains(&piece) {
                OnBlock::Duplicate
            } else {
                OnBlock::Rejected
            };
        };
        let bi = (begin / BLOCK_SIZE) as usize;
        let expected = size.map(|s| (s - begin.min(s)).min(BLOCK_SIZE)).unwrap_or(0);
        if bi >= part.blocks.len() || data.len() as u32 != expected {
            return OnBlock::Rejected;
        }
        let cancel = match std::mem::replace(&mut part.blocks[bi], Block::Done) {
            Block::Done => return OnBlock::Duplicate,
            Block::Free => Vec::new(),
            Block::Requested(peers) => peers.into_iter().filter(|&p| p != peer).collect(),
        };
        part.data[begin as usize..begin as usize + data.len()].copy_from_slice(data);
        part.done += 1;
        if !part.contributors.contains(&peer) {
            part.contributors.push(peer);
        }
        if part.done as usize == part.blocks.len() {
            let part = self.partial.remove(&piece).expect("present");
            self.verifying.insert(piece);
            OnBlock::Complete { piece, data: part.data, contributors: part.contributors, cancel }
        } else {
            OnBlock::Accepted { cancel }
        }
    }

    fn piece_size_checked(&self, piece: u32) -> Option<u32> {
        ((piece as usize) < self.num_pieces()).then(|| self.piece_size(piece))
    }

    /// Return a single outstanding request to the pool (timeout, reject, cancel).
    pub fn release(&mut self, peer: PeerKey, req: &BlockReq) {
        let Some(part) = self.partial.get_mut(&req.piece) else { return };
        let bi = (req.begin / BLOCK_SIZE) as usize;
        if let Some(Block::Requested(peers)) = part.blocks.get_mut(bi) {
            peers.retain(|&p| p != peer);
            if peers.is_empty() {
                part.blocks[bi] = Block::Free;
            }
        }
        if part.idle() {
            self.partial.remove(&req.piece);
        }
    }

    /// Return every request held by `peer` (disconnect / choke).
    pub fn release_peer(&mut self, peer: PeerKey) {
        for part in self.partial.values_mut() {
            for b in part.blocks.iter_mut() {
                if let Block::Requested(peers) = b {
                    peers.retain(|&p| p != peer);
                    if peers.is_empty() {
                        *b = Block::Free;
                    }
                }
            }
        }
        self.partial.retain(|_, p| !p.idle());
    }

    /// Hash check finished. Returns true if the piece is now owned.
    pub fn verified(&mut self, piece: u32, ok: bool) -> bool {
        self.verifying.remove(&piece);
        ok && self.have.set(piece as usize)
    }

    /// Forget a piece entirely (used on failed disk writes).
    pub fn unhave(&mut self, piece: u32) {
        self.have.unset(piece as usize);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(pieces: usize, piece_len: u32, total: u64) -> Info {
        Info {
            name: "x".into(),
            piece_length: piece_len,
            pieces: vec![[0; 20]; pieces],
            files: vec![],
            multi_file: false,
            total_length: total,
            private: false,
            raw: vec![],
        }
    }

    #[test]
    fn picks_only_what_peer_has_and_completes() {
        let inf = info(3, BLOCK_SIZE * 2, (BLOCK_SIZE * 5) as u64);
        let mut p = Picker::new(&inf, Bitfield::new(3));
        let mut peer_bf = Bitfield::new(3);
        peer_bf.set(2);
        p.add_bitfield(&peer_bf);
        let mut out = vec![];
        p.pick(1, &peer_bf, 10, &mut out);
        assert_eq!(out, vec![BlockReq { piece: 2, begin: 0, len: BLOCK_SIZE }]);
        match p.on_block(1, 2, 0, &vec![7; BLOCK_SIZE as usize]) {
            OnBlock::Complete { piece, data, .. } => {
                assert_eq!(piece, 2);
                assert_eq!(data.len(), BLOCK_SIZE as usize);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(p.verified(2, true));
        assert!(!p.is_interesting(&peer_bf));
    }

    #[test]
    fn endgame_duplicates_then_cancels() {
        let inf = info(1, BLOCK_SIZE, BLOCK_SIZE as u64);
        let mut p = Picker::new(&inf, Bitfield::new(1));
        let all = Bitfield::full(1);
        let (mut a, mut b) = (vec![], vec![]);
        p.pick(1, &all, 4, &mut a);
        p.pick(2, &all, 4, &mut b);
        assert_eq!(a, b, "second peer should race the same block");
        match p.on_block(2, 0, 0, &vec![0; BLOCK_SIZE as usize]) {
            OnBlock::Complete { cancel, .. } => assert_eq!(cancel, vec![1]),
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(p.on_block(1, 0, 0, &vec![0; BLOCK_SIZE as usize]), OnBlock::Duplicate));
    }

    #[test]
    fn release_frees_blocks() {
        let inf = info(2, BLOCK_SIZE, 2 * BLOCK_SIZE as u64);
        let mut p = Picker::new(&inf, Bitfield::new(2));
        let all = Bitfield::full(2);
        let mut out = vec![];
        p.pick(1, &all, 1, &mut out);
        p.release_peer(1);
        let mut again = vec![];
        p.pick(2, &all, 2, &mut again);
        assert_eq!(again.len(), 2);
    }

    #[test]
    fn skips_unwanted() {
        let inf = info(2, BLOCK_SIZE, 2 * BLOCK_SIZE as u64);
        let mut p = Picker::new(&inf, Bitfield::new(2));
        p.set_piece_priorities(vec![0, 1]);
        let mut out = vec![];
        p.pick(1, &Bitfield::full(2), 8, &mut out);
        assert!(out.iter().all(|r| r.piece == 1));
        assert_eq!(p.wanted_bytes_left(), BLOCK_SIZE as u64);
    }
}
