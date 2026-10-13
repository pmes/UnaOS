//! Ordered property storage for ordinary objects: insertion order preserved, hashed lookup above a few
//! entries, tombstones on delete with periodic compaction.
//!
//! AETHERPERF (SR118) hidden classes: a map seeded with the VM's root `Shape` (`Vm::alloc`) is SHAPED — its
//! entries are dense, in insertion order, and the key -> slot lookup is the shape's, shared by every object that
//! added the same keys in the same order. Adding a key follows (or creates) the shape's transition for that key.
//! A delete, an integer-index key, or more than `MAX_SHAPED` keys turns the map into DICTIONARY mode: the old
//! storage (own hash table, tombstones), unchanged. An unseeded map (`PropMap::new()`) starts in dictionary mode.
//! A shape is immutable once made, so "same `Rc<Shape>`" means "same keys at the same slots" — what the inline
//! caches (vm/ic.rs) compare.

use super::value::{Obj, PropertyKey, Value};
use alloc::rc::{Rc, Weak};
use alloc::vec::Vec;
use core::cell::RefCell;

/// Past this many keys a shaped map becomes a dictionary.
pub const MAX_SHAPED: usize = 64;

/// The key list a chain of shapes shares: a shape with `n` keys owns `keys[..n]`. A transition from the shape
/// at the tip appends here instead of copying; a transition from a non-tip shape copies its prefix.
struct KeyTable {
    keys: Vec<PropertyKey>,
    /// Open-addressing index over `keys` (positions), built once there are more than 8 keys.
    index: Vec<u32>,
}

impl KeyTable {
    fn find(&self, key: &PropertyKey, n: usize) -> Option<usize> {
        if self.index.is_empty() {
            return self.keys[..n].iter().position(|k| k == key);
        }
        let mask = self.index.len() - 1;
        let mut h = key.hash32() as usize & mask;
        loop {
            let t = self.index[h];
            if t == EMPTY {
                return None;
            }
            if self.keys[t as usize] == *key {
                // Keys in a table are distinct; a hit past `n` belongs to a longer shape.
                return if (t as usize) < n { Some(t as usize) } else { None };
            }
            h = (h + 1) & mask;
        }
    }
    fn push(&mut self, key: PropertyKey) {
        let i = self.keys.len();
        self.keys.push(key);
        if self.keys.len() <= 8 {
            return;
        }
        if self.index.is_empty() || self.keys.len() * 2 > self.index.len() {
            let mut cap = 32;
            while cap < self.keys.len() * 2 {
                cap *= 2;
            }
            self.index.clear();
            self.index.resize(cap, EMPTY);
            for j in 0..self.keys.len() {
                self.index_put(j);
            }
        } else {
            self.index_put(i);
        }
    }
    fn index_put(&mut self, j: usize) {
        let mask = self.index.len() - 1;
        let mut h = self.keys[j].hash32() as usize & mask;
        while self.index[h] != EMPTY {
            h = (h + 1) & mask;
        }
        self.index[h] = j as u32;
    }
}

/// A hidden class: the ordered key set of a shaped property map.
pub struct Shape {
    table: Rc<RefCell<KeyTable>>,
    n: usize,
    /// Kept alive by its children so a chain re-walked from the root finds the same shapes.
    _parent: Option<Rc<Shape>>,
    /// (key hash, key, child) for each key added from this shape.
    transitions: RefCell<Vec<(u32, PropertyKey, Weak<Shape>)>>,
}

impl Shape {
    /// The empty root shape (one per VM).
    pub fn root() -> Rc<Shape> {
        Rc::new(Shape { table: Rc::new(RefCell::new(KeyTable { keys: Vec::new(), index: Vec::new() })), n: 0, _parent: None, transitions: RefCell::new(Vec::new()) })
    }
    pub fn len(&self) -> usize {
        self.n
    }
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
    /// Slot of `key` in objects of this shape.
    pub fn lookup(&self, key: &PropertyKey) -> Option<usize> {
        if self.n == 0 {
            return None;
        }
        self.table.borrow().find(key, self.n)
    }
    /// The shape after adding `key` (absent from this shape).
    pub fn transition(self: &Rc<Shape>, key: &PropertyKey) -> Rc<Shape> {
        let h = key.hash32();
        {
            let t = self.transitions.borrow();
            for (kh, k, w) in t.iter() {
                if *kh == h && k == key {
                    if let Some(s) = w.upgrade() {
                        return s;
                    }
                }
            }
        }
        let table = {
            let mut kt = self.table.borrow_mut();
            if kt.keys.len() == self.n {
                kt.push(key.clone());
                Some(self.table.clone())
            } else if kt.keys[self.n] == *key {
                Some(self.table.clone())
            } else {
                None
            }
        };
        let table = match table {
            Some(t) => t,
            None => {
                let mut kt = KeyTable { keys: Vec::with_capacity(self.n + 1), index: Vec::new() };
                for k in self.table.borrow().keys[..self.n].iter() {
                    kt.push(k.clone());
                }
                kt.push(key.clone());
                Rc::new(RefCell::new(kt))
            }
        };
        let child = Rc::new(Shape { table, n: self.n + 1, _parent: Some(self.clone()), transitions: RefCell::new(Vec::new()) });
        let mut t = self.transitions.borrow_mut();
        t.retain(|(_, _, w)| w.strong_count() > 0);
        t.push((h, key.clone(), Rc::downgrade(&child)));
        child
    }
}

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
    /// Some: shaped mode (entries dense, lookups through the shape, `table` unused). None: dictionary mode.
    shape: Option<Rc<Shape>>,
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
    /// The hidden class, when the map is in shaped mode.
    #[inline]
    pub fn shape(&self) -> Option<&Rc<Shape>> {
        self.shape.as_ref()
    }
    /// Seed an empty, never-used map with the root shape (shaped mode from its first key).
    pub fn seed(&mut self, root: &Rc<Shape>) {
        if self.entries.is_empty() && self.shape.is_none() {
            self.shape = Some(root.clone());
        }
    }
    /// Inline-cache add: append `key` (absent) whose transition from the current shape is `next`.
    pub fn push_shaped(&mut self, next: Rc<Shape>, key: PropertyKey, p: Prop) {
        if self.shape.is_some() && next.n == self.entries.len() + 1 && self.entries.len() == self.live {
            self.entries.push(Some((key, p)));
            self.live += 1;
            self.shape = Some(next);
        } else {
            self.insert(key, p);
        }
    }
    /// Leave shaped mode: the same entries, in the same order, with the dictionary's own hash table.
    fn to_dictionary(&mut self) {
        if self.shape.take().is_some() {
            self.table.clear();
            self.nindex = 0;
            self.rebuild();
        }
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
        if let Some(s) = &self.shape {
            return s.lookup(key);
        }
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
        if let Some(s) = &self.shape {
            if !matches!(key, PropertyKey::Index(_)) && self.entries.len() < MAX_SHAPED {
                let next = s.transition(&key);
                self.shape = Some(next);
                self.entries.push(Some((key, p)));
                self.live += 1;
                return;
            }
            self.to_dictionary();
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
        self.find(key)?;
        self.to_dictionary();
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
