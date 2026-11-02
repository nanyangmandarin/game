//! Rewind buffer: periodic save states stored as XOR deltas between neighbours with
//! zero-run-length encoding. Only the newest state is kept whole, so memory use stays
//! small (consecutive states differ in few bytes) and capturing is cheap.

use std::collections::VecDeque;

pub struct Rewind {
    pub enabled: bool,
    pub interval: u32,
    pub max: usize,
    counter: u32,
    latest: Option<Vec<u8>>,
    /// deltas[k] turns snapshot k+1 back into snapshot k (newest delta at the back)
    deltas: VecDeque<Vec<u8>>,
}

impl Default for Rewind {
    fn default() -> Self {
        Rewind { enabled: false, interval: 6, max: 120, counter: 0, latest: None, deltas: VecDeque::new() }
    }
}

/// Encode `prev` relative to `cur`: [prev_len][(zero_run, literal_len, literal bytes)...]
fn encode(cur: &[u8], prev: &[u8]) -> Vec<u8> {
    let n = prev.len();
    let mut out = Vec::with_capacity(4096);
    out.extend_from_slice(&(n as u32).to_le_bytes());
    let x = |i: usize| prev[i] ^ cur.get(i).copied().unwrap_or(0);
    let mut i = 0;
    while i < n {
        // zero run (8 bytes at a time when possible)
        let start = i;
        while i + 8 <= n && i + 8 <= cur.len() && prev[i..i + 8] == cur[i..i + 8] {
            i += 8;
        }
        while i < n && x(i) == 0 {
            i += 1;
        }
        let zeros = i - start;
        // literal run until at least 8 zero bytes follow
        let lit_start = i;
        while i < n {
            if x(i) == 0 {
                let mut k = i;
                while k < n && k - i < 8 && x(k) == 0 {
                    k += 1;
                }
                if k - i >= 8 || k == n {
                    break;
                }
                i = k;
            } else {
                i += 1;
            }
        }
        out.extend_from_slice(&(zeros as u32).to_le_bytes());
        out.extend_from_slice(&((i - lit_start) as u32).to_le_bytes());
        for k in lit_start..i {
            out.push(x(k));
        }
    }
    out
}

fn decode(cur: &[u8], d: &[u8]) -> Vec<u8> {
    let rd = |o: usize| u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]) as usize;
    let n = rd(0);
    let mut out = vec![0u8; n];
    let m = n.min(cur.len());
    out[..m].copy_from_slice(&cur[..m]);
    let mut p = 4;
    let mut i = 0;
    while p < d.len() {
        let zeros = rd(p);
        let lits = rd(p + 4);
        p += 8;
        i += zeros;
        for k in 0..lits {
            out[i + k] ^= d[p + k];
        }
        i += lits;
        p += lits;
    }
    out
}

impl Rewind {
    pub fn clear(&mut self) {
        self.latest = None;
        self.deltas.clear();
        self.counter = 0;
    }

    /// Called every frame; returns true when a snapshot should be taken now.
    pub fn due(&mut self) -> bool {
        if !self.enabled {
            return false;
        }
        self.counter += 1;
        if self.counter >= self.interval {
            self.counter = 0;
            true
        } else {
            false
        }
    }

    pub fn push(&mut self, state: Vec<u8>) {
        if let Some(prev) = self.latest.take() {
            self.deltas.push_back(encode(&state, &prev));
            while self.deltas.len() > self.max {
                self.deltas.pop_front();
            }
        }
        self.latest = Some(state);
    }

    /// Step one snapshot back; returns the state to load.
    pub fn pop(&mut self) -> Option<&[u8]> {
        let d = self.deltas.pop_back()?;
        let cur = self.latest.take()?;
        self.latest = Some(decode(&cur, &d));
        self.counter = 0;
        self.latest.as_deref()
    }

    pub fn len(&self) -> usize {
        self.deltas.len()
    }

    pub fn is_empty(&self) -> bool {
        self.deltas.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.deltas.iter().map(|d| d.len()).sum::<usize>() + self.latest.as_ref().map(|l| l.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_roundtrip() {
        let mut a = vec![0u8; 100_000];
        for (i, b) in a.iter_mut().enumerate() {
            *b = (i * 7 % 251) as u8;
        }
        let mut b = a.clone();
        b[10] ^= 1;
        b[5000..5100].fill(9);
        b.push(42); // length change
        let d = encode(&b, &a);
        assert!(d.len() < 1000, "delta should be small: {}", d.len());
        assert_eq!(decode(&b, &d), a);
        let d2 = encode(&a, &b);
        assert_eq!(decode(&a, &d2), b);
    }
}
