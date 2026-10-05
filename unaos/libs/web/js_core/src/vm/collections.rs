//! Insertion-ordered hash storage keyed by SameValueZero, shared by Map, Set, WeakMap and WeakSet.

use super::heap::MapData;
use super::value::Value;

pub fn hash_value(v: &Value) -> u32 {
    match v {
        Value::Undefined | Value::Empty => 0x1234_5678,
        Value::Null => 0x2345_6789,
        Value::Bool(b) => 0x3456_7890 ^ (*b as u32),
        Value::Number(n) => {
            let n = if *n == 0.0 { 0.0 } else { *n };
            let b = if n.is_nan() { 0x7FF8_0000_0000_0000u64 } else { n.to_bits() };
            let h = b.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            (h >> 32) as u32 ^ h as u32
        }
        Value::String(s) => s.hash32(),
        Value::Symbol(s) => (s.id() as u32).wrapping_mul(0x85EB_CA77),
        Value::BigInt(b) => {
            let mut h: u32 = if b.neg { 0x9E37 } else { 0x79B9 };
            for l in &b.mag.limbs {
                h = (h ^ l).wrapping_mul(0x0100_0193);
            }
            h
        }
        Value::Object(o) => o.0.wrapping_mul(0x9E37_79B1) ^ 0x5555,
    }
}

const EMPTY: u32 = u32::MAX;
const TOMB: u32 = u32::MAX - 1;

pub fn rebuild_map(m: &mut MapData) {
    // Drop leading tombstones (safe for live iterators, which hold absolute indices).
    let lead = m.entries.iter().take_while(|e| e.is_none()).count();
    if lead > 0 {
        m.entries.drain(..lead);
        m.offset += lead;
    }
    let mut cap = 8;
    while cap < m.entries.len() * 2 {
        cap *= 2;
    }
    m.table.clear();
    m.table.resize(cap, EMPTY);
    let mask = cap - 1;
    for (i, e) in m.entries.iter().enumerate() {
        if let Some((k, _)) = e {
            let mut h = hash_value(k) as usize & mask;
            while m.table[h] != EMPTY {
                h = (h + 1) & mask;
            }
            m.table[h] = i as u32;
        }
    }
}

impl MapData {
    pub fn find(&self, k: &Value) -> Option<usize> {
        if self.table.is_empty() {
            return None;
        }
        let mask = self.table.len() - 1;
        let mut h = hash_value(k) as usize & mask;
        loop {
            let t = self.table[h];
            if t == EMPTY {
                return None;
            }
            if t == TOMB {
                h = (h + 1) & mask;
                continue;
            }
            if let Some((kk, _)) = &self.entries[t as usize] {
                if kk.same_value_zero(k) {
                    return Some(t as usize);
                }
            }
            h = (h + 1) & mask;
        }
    }
    pub fn get(&self, k: &Value) -> Option<&Value> {
        self.find(k).map(|i| &self.entries[i].as_ref().unwrap().1)
    }
    pub fn has(&self, k: &Value) -> bool {
        self.find(k).is_some()
    }
    pub fn set(&mut self, k: Value, v: Value) {
        let k = match k {
            Value::Number(n) if n == 0.0 => Value::Number(0.0),
            other => other,
        };
        if let Some(i) = self.find(&k) {
            self.entries[i].as_mut().unwrap().1 = v;
            return;
        }
        self.entries.push(Some((k, v)));
        self.live += 1;
        if self.entries.len() * 2 > self.table.len() {
            rebuild_map(self);
        } else {
            let mask = self.table.len() - 1;
            let idx = self.entries.len() - 1;
            let mut h = hash_value(&self.entries[idx].as_ref().unwrap().0) as usize & mask;
            while self.table[h] != EMPTY {
                h = (h + 1) & mask;
            }
            self.table[h] = idx as u32;
        }
    }
    pub fn delete(&mut self, k: &Value) -> bool {
        match self.find(k) {
            Some(i) => {
                let mask = self.table.len() - 1;
                let mut h = hash_value(k) as usize & mask;
                while self.table[h] != i as u32 {
                    h = (h + 1) & mask;
                }
                self.table[h] = TOMB;
                self.entries[i] = None;
                self.live -= 1;
                if self.entries.len() > 32 && self.live * 4 < self.entries.len() {
                    rebuild_map(self);
                }
                true
            }
            None => false,
        }
    }
    pub fn clear(&mut self) {
        for e in self.entries.iter_mut() {
            *e = None;
        }
        self.live = 0;
        rebuild_map(self);
    }
    /// Entry at an absolute index.
    pub fn at(&self, abs: usize) -> Option<&(Value, Value)> {
        if abs < self.offset {
            return None;
        }
        self.entries.get(abs - self.offset).and_then(|e| e.as_ref())
    }
    pub fn end(&self) -> usize {
        self.offset + self.entries.len()
    }
}

