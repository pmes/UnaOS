//! Ordered property storage for ordinary objects: insertion order preserved, hashed lookup above a few
//! entries, tombstones on delete with periodic compaction.

use super::value::{Obj, PropertyKey, Value};
use alloc::vec::Vec;

pub const W: u8 = 1;
pub const E: u8 = 2;
pub const C: u8 = 4;
pub const WEC: u8 = W | E | C;
pub const WC: u8 = W | C;

#[derive(Clone, Debug)]
pub enum Slot {
    Data(Value),
    Accessor(Option<Obj>, Option<Obj>),
}

#[derive(Clone, Debug)]
pub struct Prop {
    pub slot: Slot,
    pub flags: u8,
}

impl Prop {
    pub fn data(v: Value, flags: u8) -> Prop {
        Prop { slot: Slot::Data(v), flags }
    }
    pub fn writable(&self) -> bool {
        self.flags & W != 0
    }
    pub fn enumerable(&self) -> bool {
        self.flags & E != 0
    }
    pub fn configurable(&self) -> bool {
        self.flags & C != 0
    }
    pub fn is_accessor(&self) -> bool {
        matches!(self.slot, Slot::Accessor(..))
    }
}

#[derive(Default, Clone)]
pub struct PropMap {
    entries: Vec<Option<(PropertyKey, Prop)>>,
    table: Vec<u32>,
    live: usize,
    /// Number of integer-index keys (lets array fast paths skip prototypes without indexed properties).
    nindex: u32,
}

const EMPTY: u32 = u32::MAX;
const TOMB: u32 = u32::MAX - 1;

impl PropMap {
    pub fn new() -> PropMap {
        PropMap::default()
    }
    pub fn len(&self) -> usize {
        self.live
    }
    pub fn has_index_keys(&self) -> bool {
        self.nindex > 0
    }
    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    fn rebuild(&mut self) {
        // Compact tombstones.
        if self.live != self.entries.len() {
            self.entries.retain(|e| e.is_some());
        }
        if self.live < 8 {
            self.table.clear();
            return;
        }
        let mut cap = 16;
        while cap < self.live * 2 {
            cap *= 2;
        }
        self.table.clear();
        self.table.resize(cap, EMPTY);
        for (i, e) in self.entries.iter().enumerate() {
            let k = &e.as_ref().unwrap().0;
            let mask = cap - 1;
            let mut h = k.hash32() as usize & mask;
            while self.table[h] != EMPTY {
                h = (h + 1) & mask;
            }
            self.table[h] = i as u32;
        }
    }

    pub fn find(&self, key: &PropertyKey) -> Option<usize> {
        if self.table.is_empty() {
            for (i, e) in self.entries.iter().enumerate() {
                if let Some((k, _)) = e {
                    if k == key {
                        return Some(i);
                    }
                }
            }
            return None;
        }
        let mask = self.table.len() - 1;
        let mut h = key.hash32() as usize & mask;
        loop {
            let t = self.table[h];
            if t == EMPTY {
                return None;
            }
            if t != TOMB {
                if let Some((k, _)) = &self.entries[t as usize] {
                    if k == key {
                        return Some(t as usize);
                    }
                }
            }
            h = (h + 1) & mask;
        }
    }

    pub fn get(&self, key: &PropertyKey) -> Option<&Prop> {
        self.find(key).map(|i| &self.entries[i].as_ref().unwrap().1)
    }
    pub fn get_mut(&mut self, key: &PropertyKey) -> Option<&mut Prop> {
        match self.find(key) {
            Some(i) => Some(&mut self.entries[i].as_mut().unwrap().1),
            None => None,
        }
    }
    pub fn at(&self, i: usize) -> &Prop {
        &self.entries[i].as_ref().unwrap().1
    }
    pub fn at_mut(&mut self, i: usize) -> &mut Prop {
        &mut self.entries[i].as_mut().unwrap().1
    }

    /// Insert a new key (caller guarantees absence) or overwrite an existing one.
    pub fn insert(&mut self, key: PropertyKey, p: Prop) {
        if let Some(i) = self.find(&key) {
            self.entries[i].as_mut().unwrap().1 = p;
            return;
        }
        if matches!(key, PropertyKey::Index(_)) {
            self.nindex += 1;
        }
        let idx = self.entries.len();
        if !self.table.is_empty() {
            if (self.entries.len() + 1) * 2 > self.table.len() {
                self.entries.push(Some((key, p)));
                self.live += 1;
                self.rebuild();
                return;
            }
            let mask = self.table.len() - 1;
            let mut h = key.hash32() as usize & mask;
            while self.table[h] != EMPTY && self.table[h] != TOMB {
                h = (h + 1) & mask;
            }
            self.table[h] = idx as u32;
            self.entries.push(Some((key, p)));
            self.live += 1;
            return;
        }
        self.entries.push(Some((key, p)));
        self.live += 1;
        if self.live >= 8 {
            self.rebuild();
        }
    }

    pub fn remove(&mut self, key: &PropertyKey) -> Option<Prop> {
        let i = self.find(key)?;
        let (k, p) = self.entries[i].take().unwrap();
        self.live -= 1;
        if matches!(k, PropertyKey::Index(_)) {
            self.nindex -= 1;
        }
        if !self.table.is_empty() {
            let mask = self.table.len() - 1;
            let mut h = key.hash32() as usize & mask;
            loop {
                if self.table[h] == i as u32 {
                    self.table[h] = TOMB;
                    break;
                }
                h = (h + 1) & mask;
            }
        }
        if self.entries.len() > 16 && self.live * 2 < self.entries.len() {
            self.rebuild();
        } else if self.table.is_empty() {
            // Small maps: keep entries dense.
            self.entries.retain(|e| e.is_some());
        }
        Some(p)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&PropertyKey, &Prop)> {
        self.entries.iter().filter_map(|e| e.as_ref().map(|(k, p)| (k, p)))
    }
    pub fn keys(&self) -> impl Iterator<Item = &PropertyKey> {
        self.iter().map(|(k, _)| k)
    }
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut Prop> {
        self.entries.iter_mut().filter_map(|e| e.as_mut().map(|(_, p)| p))
    }
}
