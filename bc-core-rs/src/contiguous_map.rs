//! A hash map whose values live in one contiguous run.

use std::collections::HashMap;
use std::hash::Hash;

/// A map with hashed lookup and contiguous key and value storage.
///
/// Lookup goes through a hash index, as any map's does. What this adds is that
/// the keys and values themselves sit in two dense slices, so iterating is a
/// linear walk and [`values`](Self::values) can be handed to anything that wants
/// a slice — neither of which a bucketed map can offer.
///
/// The price is that **removal does not preserve order**: the last entry is
/// swapped into the hole so the run stays dense. Use this where iteration and
/// slice access are worth more than a stable order.
///
/// ```
/// use bc_core::ContiguousMap;
///
/// let mut map = ContiguousMap::new();
/// map.insert("alpha", 1);
/// map.insert("beta", 2);
///
/// assert_eq!(map.get(&"beta"), Some(&2));
/// assert_eq!(map.values(), &[1, 2]);
///
/// assert_eq!(map.remove(&"alpha"), Some(1));
/// assert_eq!(map.get(&"alpha"), None);
/// ```
#[derive(Debug, Clone)]
pub struct ContiguousMap<K, V> {
    /// Where each key's value sits in the dense runs below.
    index: HashMap<K, usize>,
    keys: Vec<K>,
    values: Vec<V>,
}

impl<K, V> ContiguousMap<K, V>
where
    K: Eq + Hash + Clone,
{
    /// Creates an empty map.
    #[must_use]
    pub fn new() -> Self {
        Self {
            index: HashMap::new(),
            keys: Vec::new(),
            values: Vec::new(),
        }
    }

    /// Creates an empty map with room for `capacity` entries.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            index: HashMap::with_capacity(capacity),
            keys: Vec::with_capacity(capacity),
            values: Vec::with_capacity(capacity),
        }
    }

    /// Stores a value, replacing whatever the key already named.
    pub fn insert(&mut self, key: K, value: V) {
        if let Some(&position) = self.index.get(&key) {
            self.values[position] = value;
            return;
        }
        self.index.insert(key.clone(), self.values.len());
        self.keys.push(key);
        self.values.push(value);
    }

    /// Takes a value out, or returns `None` if the key names nothing.
    ///
    /// The last entry is swapped into the hole, so the remaining entries stay
    /// contiguous and **their order changes**.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        let position = self.index.remove(key)?;

        let removed = self.values.swap_remove(position);
        self.keys.swap_remove(position);
        // `swap_remove` moved the last entry here, unless the hole *was* last.
        if let Some(moved) = self.keys.get(position) {
            self.index.insert(moved.clone(), position);
        }
        Some(removed)
    }

    /// Borrows the value a key names, if it names one.
    #[must_use]
    pub fn get(&self, key: &K) -> Option<&V> {
        self.values.get(*self.index.get(key)?)
    }

    /// Mutably borrows the value a key names, if it names one.
    #[must_use]
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        let position = *self.index.get(key)?;
        self.values.get_mut(position)
    }

    /// Whether a key names a value.
    #[must_use]
    pub fn contains(&self, key: &K) -> bool {
        self.index.contains_key(key)
    }

    /// How many entries the map holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether the map holds no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// How many entries the map can hold before it reallocates.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.values.capacity()
    }

    /// Drops every entry.
    pub fn clear(&mut self) {
        self.index.clear();
        self.keys.clear();
        self.values.clear();
    }

    /// Makes room for `additional` more entries.
    pub fn reserve(&mut self, additional: usize) {
        self.index.reserve(additional);
        self.keys.reserve(additional);
        self.values.reserve(additional);
    }

    /// Gives back whatever room the map is not using.
    pub fn shrink_to_fit(&mut self) {
        self.index.shrink_to_fit();
        self.keys.shrink_to_fit();
        self.values.shrink_to_fit();
    }

    /// Drops every entry the predicate declines.
    ///
    /// Unlike [`remove`](Self::remove) this **keeps the surviving entries in
    /// order**: it compacts in one pass rather than swapping from the end.
    pub fn retain(&mut self, mut predicate: impl FnMut(&K, &V) -> bool) {
        let mut write = 0;
        for read in 0..self.values.len() {
            if predicate(&self.keys[read], &self.values[read]) {
                if write != read {
                    self.keys.swap(write, read);
                    self.values.swap(write, read);
                    self.index.insert(self.keys[write].clone(), write);
                }
                write += 1;
            } else {
                self.index.remove(&self.keys[read]);
            }
        }
        self.keys.truncate(write);
        self.values.truncate(write);
    }

    /// Every key, in storage order.
    #[must_use]
    pub fn keys(&self) -> &[K] {
        &self.keys
    }

    /// Every value, in storage order — the same order as [`keys`](Self::keys).
    #[must_use]
    pub fn values(&self) -> &[V] {
        &self.values
    }

    /// Every value, mutably, in the same order.
    #[must_use]
    pub fn values_mut(&mut self) -> &mut [V] {
        &mut self.values
    }

    /// Borrows every entry, in storage order.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.keys.iter().zip(self.values.iter())
    }

    /// Borrows every entry with its value mutable, in storage order.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&K, &mut V)> {
        self.keys.iter().zip(self.values.iter_mut())
    }
}

impl<K, V> Default for ContiguousMap<K, V>
where
    K: Eq + Hash + Clone,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> PartialEq for ContiguousMap<K, V>
where
    K: Eq + Hash + Clone,
    V: PartialEq,
{
    /// Compares by content, not by storage order — two maps holding the same
    /// entries are equal however removal happened to arrange them.
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .all(|(key, value)| other.get(key) == Some(value))
    }
}

impl<K, V> Eq for ContiguousMap<K, V>
where
    K: Eq + Hash + Clone,
    V: Eq,
{
}
