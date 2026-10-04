//! Minimal, allocation-conscious bencode codec.
//!
//! `serde_bencode` cannot hand back the raw byte span of a nested value (needed
//! for info-hash computation) and is brittle against the loosely-specified
//! shapes trackers and DHT nodes emit in the wild. This module decodes into a
//! dynamic [`Value`] tree with a hard nesting limit so hostile input cannot
//! blow the stack.

use std::collections::BTreeMap;

use crate::error::{Error, Result};

const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Dict(BTreeMap<Vec<u8>, Value>),
}

impl Value {
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        self.as_bytes().and_then(|b| std::str::from_utf8(b).ok())
    }

    /// Lossy UTF-8 view — torrent metadata frequently carries legacy encodings.
    pub fn as_string_lossy(&self) -> Option<String> {
        self.as_bytes().map(|b| String::from_utf8_lossy(b).into_owned())
    }

    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&BTreeMap<Vec<u8>, Value>> {
        match self {
            Value::Dict(d) => Some(d),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_dict().and_then(|d| d.get(key.as_bytes()))
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_into(&mut out);
        out
    }

    pub fn encode_into(&self, out: &mut Vec<u8>) {
        match self {
            Value::Int(i) => {
                out.push(b'i');
                out.extend_from_slice(i.to_string().as_bytes());
                out.push(b'e');
            }
            Value::Bytes(b) => encode_bytes(b, out),
            Value::List(l) => {
                out.push(b'l');
                for v in l {
                    v.encode_into(out);
                }
                out.push(b'e');
            }
            Value::Dict(d) => {
                out.push(b'd');
                // BTreeMap iterates in raw byte order, as the spec requires.
                for (k, v) in d {
                    encode_bytes(k, out);
                    v.encode_into(out);
                }
                out.push(b'e');
            }
        }
    }
}

pub fn encode_bytes(b: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(b.len().to_string().as_bytes());
    out.push(b':');
    out.extend_from_slice(b);
}

/// Convenience builder for dictionaries with `&str` keys.
#[derive(Default)]
pub struct DictBuilder(BTreeMap<Vec<u8>, Value>);

impl DictBuilder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn int(mut self, k: &str, v: i64) -> Self {
        self.0.insert(k.as_bytes().to_vec(), Value::Int(v));
        self
    }
    pub fn bytes(mut self, k: &str, v: impl Into<Vec<u8>>) -> Self {
        self.0.insert(k.as_bytes().to_vec(), Value::Bytes(v.into()));
        self
    }
    pub fn value(mut self, k: &str, v: Value) -> Self {
        self.0.insert(k.as_bytes().to_vec(), v);
        self
    }
    pub fn build(self) -> Value {
        Value::Dict(self.0)
    }
}

/// Decode exactly one value occupying the whole buffer.
pub fn decode(data: &[u8]) -> Result<Value> {
    let (v, used) = decode_prefix(data)?;
    if used != data.len() {
        return Err(Error::Bencode("trailing bytes after value".into()));
    }
    Ok(v)
}

/// Decode one value from the front of `data`, returning it and the bytes consumed.
/// Used by ut_metadata, where a raw payload follows the bencoded header.
pub fn decode_prefix(data: &[u8]) -> Result<(Value, usize)> {
    let mut p = Parser { data, pos: 0 };
    let v = p.value(0)?;
    Ok((v, p.pos))
}

/// Locate the raw byte span of a top-level dictionary key's value.
/// The info-hash must be computed over the exact original bytes.
pub fn raw_value_span<'a>(data: &'a [u8], key: &[u8]) -> Result<&'a [u8]> {
    let mut p = Parser { data, pos: 0 };
    if p.peek()? != b'd' {
        return Err(Error::Bencode("expected top-level dict".into()));
    }
    p.pos += 1;
    while p.peek()? != b'e' {
        let k = p.bytes()?;
        let start = p.pos;
        p.skip(1)?;
        if k == key {
            return Ok(&data[start..p.pos]);
        }
    }
    Err(Error::Bencode(format!("key '{}' not found", String::from_utf8_lossy(key))))
}

struct Parser<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Result<u8> {
        self.data
            .get(self.pos)
            .copied()
            .ok_or_else(|| Error::Bencode("unexpected end of input".into()))
    }

    fn value(&mut self, depth: usize) -> Result<Value> {
        if depth > MAX_DEPTH {
            return Err(Error::Bencode("nesting too deep".into()));
        }
        match self.peek()? {
            b'i' => Ok(Value::Int(self.int()?)),
            b'l' => {
                self.pos += 1;
                let mut items = Vec::new();
                while self.peek()? != b'e' {
                    items.push(self.value(depth + 1)?);
                }
                self.pos += 1;
                Ok(Value::List(items))
            }
            b'd' => {
                self.pos += 1;
                let mut map = BTreeMap::new();
                while self.peek()? != b'e' {
                    let k = self.bytes()?.to_vec();
                    let v = self.value(depth + 1)?;
                    map.insert(k, v);
                }
                self.pos += 1;
                Ok(Value::Dict(map))
            }
            b'0'..=b'9' => Ok(Value::Bytes(self.bytes()?.to_vec())),
            c => Err(Error::Bencode(format!("unexpected byte 0x{c:02x} at {}", self.pos))),
        }
    }

    fn skip(&mut self, depth: usize) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(Error::Bencode("nesting too deep".into()));
        }
        match self.peek()? {
            b'i' => {
                self.int()?;
            }
            b'l' | b'd' => {
                let is_dict = self.peek()? == b'd';
                self.pos += 1;
                while self.peek()? != b'e' {
                    if is_dict {
                        self.bytes()?;
                    }
                    self.skip(depth + 1)?;
                }
                self.pos += 1;
            }
            b'0'..=b'9' => {
                self.bytes()?;
            }
            c => return Err(Error::Bencode(format!("unexpected byte 0x{c:02x}"))),
        }
        Ok(())
    }

    fn int(&mut self) -> Result<i64> {
        self.pos += 1; // 'i'
        let end = self.data[self.pos..]
            .iter()
            .position(|&b| b == b'e')
            .ok_or_else(|| Error::Bencode("unterminated int".into()))?;
        let s = std::str::from_utf8(&self.data[self.pos..self.pos + end])
            .map_err(|_| Error::Bencode("non-ascii int".into()))?;
        let v = s.parse::<i64>().map_err(|_| Error::Bencode(format!("bad int '{s}'")))?;
        self.pos += end + 1;
        Ok(v)
    }

    fn bytes(&mut self) -> Result<&'a [u8]> {
        let colon = self.data[self.pos..]
            .iter()
            .take(20)
            .position(|&b| b == b':')
            .ok_or_else(|| Error::Bencode("bad string length".into()))?;
        let len: usize = std::str::from_utf8(&self.data[self.pos..self.pos + colon])
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| Error::Bencode("bad string length".into()))?;
        let start = self.pos + colon + 1;
        let end = start
            .checked_add(len)
            .filter(|&e| e <= self.data.len())
            .ok_or_else(|| Error::Bencode("string overruns buffer".into()))?;
        self.pos = end;
        Ok(&self.data[start..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let raw = b"d3:cow3:moo3:numi-42e4:spaml1:a1:bee";
        let v = decode(raw).unwrap();
        assert_eq!(v.get("cow").unwrap().as_str(), Some("moo"));
        assert_eq!(v.encode(), raw);
    }

    #[test]
    fn raw_span() {
        let raw = b"d8:announce3:url4:infod4:name1:xee";
        assert_eq!(raw_value_span(raw, b"info").unwrap(), b"d4:name1:xe");
    }

    #[test]
    fn rejects_hostile() {
        assert!(decode(b"99999999999:x").is_err());
        let deep = "l".repeat(500) + &"e".repeat(500);
        assert!(decode(deep.as_bytes()).is_err());
        assert!(decode(b"i12").is_err());
    }

    #[test]
    fn prefix_with_payload() {
        let raw = b"d8:msg_typei1e5:piecei0eeRAWDATA";
        let (v, used) = decode_prefix(raw).unwrap();
        assert_eq!(v.get("msg_type").unwrap().as_int(), Some(1));
        assert_eq!(&raw[used..], b"RAWDATA");
    }
}
