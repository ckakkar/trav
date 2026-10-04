/// MSB-first bitfield as used on the wire (BEP 3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bitfield {
    bytes: Vec<u8>,
    len: usize,
    ones: usize,
}

impl Bitfield {
    pub fn new(len: usize) -> Self {
        Self { bytes: vec![0; len.div_ceil(8)], len, ones: 0 }
    }

    pub fn full(len: usize) -> Self {
        let mut b = Self { bytes: vec![0xff; len.div_ceil(8)], len, ones: len };
        b.clear_spare();
        b
    }

    /// Build from wire bytes. `len` is the piece count; spare trailing bits are cleared.
    pub fn from_bytes(bytes: &[u8], len: usize) -> Self {
        let mut v = bytes.to_vec();
        v.resize(len.div_ceil(8), 0);
        let mut b = Self { bytes: v, len, ones: 0 };
        b.clear_spare();
        b.recount();
        b
    }

    /// Build when the piece count is not yet known (magnet links before metadata).
    pub fn from_bytes_unsized(bytes: &[u8]) -> Self {
        let mut b = Self { bytes: bytes.to_vec(), len: bytes.len() * 8, ones: 0 };
        b.recount();
        b
    }

    fn clear_spare(&mut self) {
        let spare = self.bytes.len() * 8 - self.len;
        if spare > 0 {
            if let Some(last) = self.bytes.last_mut() {
                *last &= 0xffu8 << spare;
            }
        }
    }

    fn recount(&mut self) {
        self.ones = self.bytes.iter().map(|b| b.count_ones() as usize).sum();
    }

    /// Resize to the authoritative piece count once metadata is known.
    pub fn resize(&mut self, len: usize) {
        self.bytes.resize(len.div_ceil(8), 0);
        self.len = len;
        self.clear_spare();
        self.recount();
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub fn get(&self, i: usize) -> bool {
        i < self.len && self.bytes[i / 8] & (0x80 >> (i % 8)) != 0
    }

    /// Returns true when the bit changed.
    pub fn set(&mut self, i: usize) -> bool {
        if i >= self.len || self.get(i) {
            return false;
        }
        self.bytes[i / 8] |= 0x80 >> (i % 8);
        self.ones += 1;
        true
    }

    /// Like `set`, growing the field if needed (pre-metadata `have` messages).
    pub fn set_grow(&mut self, i: usize) -> bool {
        if i >= self.len {
            if i >= 1 << 24 {
                return false;
            }
            self.len = i + 1;
            self.bytes.resize(self.len.div_ceil(8), 0);
        }
        self.set(i)
    }

    pub fn unset(&mut self, i: usize) -> bool {
        if !self.get(i) {
            return false;
        }
        self.bytes[i / 8] &= !(0x80 >> (i % 8));
        self.ones -= 1;
        true
    }

    #[inline]
    pub fn count_ones(&self) -> usize {
        self.ones
    }

    #[inline]
    pub fn all(&self) -> bool {
        self.ones == self.len
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn iter_ones(&self) -> impl Iterator<Item = usize> + '_ {
        self.bytes.iter().enumerate().flat_map(move |(bi, &byte)| {
            (0..8).filter_map(move |bit| {
                let i = bi * 8 + bit;
                (byte & (0x80 >> bit) != 0 && i < self.len).then_some(i)
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics() {
        let mut b = Bitfield::new(10);
        assert!(b.set(0));
        assert!(b.set(9));
        assert!(!b.set(9));
        assert_eq!(b.count_ones(), 2);
        assert_eq!(b.as_bytes(), &[0x80, 0x40]);
        assert_eq!(b.iter_ones().collect::<Vec<_>>(), vec![0, 9]);
        let f = Bitfield::full(10);
        assert!(f.all());
        assert_eq!(f.as_bytes(), &[0xff, 0xc0]);
        let w = Bitfield::from_bytes(&[0xff, 0xff], 10);
        assert_eq!(w.count_ones(), 10);
    }
}
