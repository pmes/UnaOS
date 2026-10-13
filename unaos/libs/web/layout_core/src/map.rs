//! [`NodeMap`]: the box-keyed map the layout tree carries (`node_map`, `paint_map`, `inline`). It was a
//! `std::collections::HashMap<taffy::NodeId, _>`; taffy's `NodeId` is `Hash` but not `Ord`, so the map is a
//! `BTreeMap` keyed by the id's `u64` that keeps the `NodeId` beside the value — the HashMap surface the
//! renderer and the cascade call (`get`, `entry().or_default()`, `iter`, `keys`, `values`, `&map` iteration)
//! is kept, with a deterministic order.

use crate::prelude::*;
use alloc::collections::btree_map;
use taffy::NodeId;

#[derive(Clone, Debug)]
pub struct NodeMap<V> {
    m: BTreeMap<u64, (NodeId, V)>,
}

impl<V> Default for NodeMap<V> {
    fn default() -> Self {
        NodeMap { m: BTreeMap::new() }
    }
}

impl<V> NodeMap<V> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        self.m.len()
    }
    pub fn is_empty(&self) -> bool {
        self.m.is_empty()
    }
    pub fn clear(&mut self) {
        self.m.clear();
    }
    pub fn get(&self, k: &NodeId) -> Option<&V> {
        self.m.get(&u64::from(*k)).map(|(_, v)| v)
    }
    pub fn get_mut(&mut self, k: &NodeId) -> Option<&mut V> {
        self.m.get_mut(&u64::from(*k)).map(|(_, v)| v)
    }
    pub fn contains_key(&self, k: &NodeId) -> bool {
        self.m.contains_key(&u64::from(*k))
    }
    pub fn insert(&mut self, k: NodeId, v: V) -> Option<V> {
        self.m.insert(u64::from(k), (k, v)).map(|(_, v)| v)
    }
    pub fn remove(&mut self, k: &NodeId) -> Option<V> {
        self.m.remove(&u64::from(*k)).map(|(_, v)| v)
    }
    pub fn entry(&mut self, k: NodeId) -> Entry<'_, V> {
        Entry { k, e: self.m.entry(u64::from(k)) }
    }
    pub fn iter(&self) -> impl Iterator<Item = (&NodeId, &V)> + '_ {
        self.m.values().map(|(k, v)| (k, v))
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&NodeId, &mut V)> + '_ {
        self.m.values_mut().map(|(k, v)| (&*k, v))
    }
    pub fn keys(&self) -> impl Iterator<Item = &NodeId> + '_ {
        self.m.values().map(|(k, _)| k)
    }
    pub fn values(&self) -> impl Iterator<Item = &V> + '_ {
        self.m.values().map(|(_, v)| v)
    }
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> + '_ {
        self.m.values_mut().map(|(_, v)| v)
    }
}

impl<'a, V> IntoIterator for &'a NodeMap<V> {
    type Item = (&'a NodeId, &'a V);
    type IntoIter = core::iter::Map<btree_map::Values<'a, u64, (NodeId, V)>, fn(&'a (NodeId, V)) -> (&'a NodeId, &'a V)>;
    fn into_iter(self) -> Self::IntoIter {
        fn pair<V>(e: &(NodeId, V)) -> (&NodeId, &V) {
            (&e.0, &e.1)
        }
        self.m.values().map(pair::<V> as fn(&'a (NodeId, V)) -> (&'a NodeId, &'a V))
    }
}

impl<V> FromIterator<(NodeId, V)> for NodeMap<V> {
    fn from_iter<I: IntoIterator<Item = (NodeId, V)>>(it: I) -> Self {
        let mut m = NodeMap::new();
        for (k, v) in it {
            m.insert(k, v);
        }
        m
    }
}

impl<V> Extend<(NodeId, V)> for NodeMap<V> {
    fn extend<I: IntoIterator<Item = (NodeId, V)>>(&mut self, it: I) {
        for (k, v) in it {
            self.insert(k, v);
        }
    }
}

/// `entry(k)`: the HashMap entry calls the layout makes.
pub struct Entry<'a, V> {
    k: NodeId,
    e: btree_map::Entry<'a, u64, (NodeId, V)>,
}

impl<'a, V> Entry<'a, V> {
    pub fn or_insert(self, v: V) -> &'a mut V {
        let k = self.k;
        &mut self.e.or_insert((k, v)).1
    }
    pub fn or_insert_with(self, f: impl FnOnce() -> V) -> &'a mut V {
        let k = self.k;
        &mut self.e.or_insert_with(|| (k, f())).1
    }
}

impl<'a, V: Default> Entry<'a, V> {
    pub fn or_default(self) -> &'a mut V {
        let k = self.k;
        &mut self.e.or_insert_with(|| (k, V::default())).1
    }
}
