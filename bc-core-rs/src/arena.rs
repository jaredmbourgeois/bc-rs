//! Generational arena — a pool of reusable slots with stable handles.

use std::num::NonZeroU32;

/// A handle to a value in an [`Arena`].
///
/// Trivially copyable and eight bytes, so it is cheap to store beside the data
/// that references it. The generation is what makes the handle safe to keep:
/// a handle to a removed value does not start naming whichever value later
/// takes the slot, it stops resolving.
///
/// A default handle never matches a live value, because generations start at
/// one. That makes [`ArenaRef::default`] usable as a null handle without
/// wrapping it in an `Option`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArenaRef {
    index: u32,
    generation: u32,
}

impl ArenaRef {
    /// Builds a handle from its parts.
    ///
    /// Handles come from [`Arena::insert`]; this exists for the round trip
    /// through [`to_u64`](Self::to_u64) and for tests that need a handle the
    /// arena never issued.
    #[must_use]
    pub const fn new(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }

    /// The slot this handle names.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.index
    }

    /// Which occupancy of that slot this handle belongs to.
    #[must_use]
    pub const fn generation(self) -> u32 {
        self.generation
    }

    /// Packs the handle into one integer, generation in the high half.
    ///
    /// For carrying a handle through something that stores a single number — a
    /// serialized record, a foreign-language boundary — rather than for
    /// arithmetic. The round trip is exact over the whole `u32` range of both
    /// halves.
    #[must_use]
    pub const fn to_u64(self) -> u64 {
        ((self.generation as u64) << 32) | self.index as u64
    }

    /// Unpacks a handle written by [`to_u64`](Self::to_u64).
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the truncation is the unpacking: each half is a u32 by construction"
    )]
    pub const fn from_u64(packed: u64) -> Self {
        Self {
            index: packed as u32,
            generation: (packed >> 32) as u32,
        }
    }
}

/// One slot, holding a value or waiting on the free list.
struct Slot<T> {
    /// Bumped when a value is removed, which is what invalidates the handles
    /// that named it. Never zero, so a default [`ArenaRef`] matches nothing.
    generation: NonZeroU32,
    value: Option<T>,
    /// The next free slot, valid only while `value` is `None`.
    next_free: Option<u32>,
}

/// A pool of reusable slots addressed by stable, generation-checked handles.
///
/// Insert, remove, and lookup are all constant time. Removing a value returns
/// its slot to a free list for the next insert, so an arena is sized by the
/// values alive at once rather than by every value it has ever held.
///
/// The generation counter is the difference between this and a `Vec` of
/// `Option`: a handle whose value has been removed resolves to `None` instead
/// of silently naming whatever was inserted into the slot afterwards.
///
/// ```
/// use bc_core::Arena;
///
/// let mut arena = Arena::new();
/// let first = arena.insert("a");
/// let second = arena.insert("b");
///
/// assert_eq!(arena.remove(first), Some("a"));
///
/// // The slot is reused, but the handle that named it is not fooled.
/// let third = arena.insert("c");
/// assert_eq!(third.index(), first.index());
/// assert_eq!(arena.get(first), None);
/// assert_eq!(arena.get(third), Some(&"c"));
/// assert_eq!(arena.get(second), Some(&"b"));
/// ```
pub struct Arena<T> {
    slots: Vec<Slot<T>>,
    free_head: Option<u32>,
    length: usize,
}

/// The generation every slot starts at.
const FIRST_GENERATION: NonZeroU32 = NonZeroU32::new(1).expect("1 is not zero");

impl<T> Arena<T> {
    /// Creates an empty arena.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            slots: Vec::new(),
            free_head: None,
            length: 0,
        }
    }

    /// Creates an empty arena with room for `capacity` slots.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            slots: Vec::with_capacity(capacity),
            free_head: None,
            length: 0,
        }
    }

    /// Stores a value and returns the handle naming it.
    ///
    /// # Panics
    ///
    /// Panics if the arena would exceed `u32::MAX` slots, which the handle
    /// cannot address. Failing at the boundary beats wrapping into another
    /// value's slot.
    pub fn insert(&mut self, value: T) -> ArenaRef {
        if let Some(index) = self.free_head {
            let slot = &mut self.slots[index as usize];
            self.free_head = slot.next_free;
            slot.next_free = None;
            slot.value = Some(value);
            self.length += 1;
            return ArenaRef::new(index, slot.generation.get());
        }

        let index = u32::try_from(self.slots.len()).expect("an arena holds fewer than u32::MAX");
        self.slots.push(Slot {
            generation: FIRST_GENERATION,
            value: Some(value),
            next_free: None,
        });
        self.length += 1;
        ArenaRef::new(index, FIRST_GENERATION.get())
    }

    /// Takes a value out, freeing its slot and invalidating its handles.
    ///
    /// Returns `None` for a handle the arena never issued, or one whose value
    /// has already been removed.
    pub fn remove(&mut self, handle: ArenaRef) -> Option<T> {
        let slot = self.slots.get_mut(handle.index as usize)?;
        if slot.generation.get() != handle.generation {
            return None;
        }
        let value = slot.value.take()?;

        slot.generation = next_generation(slot.generation);
        slot.next_free = self.free_head;
        self.free_head = Some(handle.index);
        self.length -= 1;
        Some(value)
    }

    /// Borrows the value a handle names, if it is still there.
    #[must_use]
    pub fn get(&self, handle: ArenaRef) -> Option<&T> {
        let slot = self.slots.get(handle.index as usize)?;
        if slot.generation.get() == handle.generation {
            slot.value.as_ref()
        } else {
            None
        }
    }

    /// Mutably borrows the value a handle names, if it is still there.
    #[must_use]
    pub fn get_mut(&mut self, handle: ArenaRef) -> Option<&mut T> {
        let slot = self.slots.get_mut(handle.index as usize)?;
        if slot.generation.get() == handle.generation {
            slot.value.as_mut()
        } else {
            None
        }
    }

    /// Whether a handle still names a value.
    #[must_use]
    pub fn contains(&self, handle: ArenaRef) -> bool {
        self.get(handle).is_some()
    }

    /// How many values the arena holds.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.length
    }

    /// Whether the arena holds no values.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// How many slots the arena has taken, live or free.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Drops every value and returns every slot to the free list.
    ///
    /// Every outstanding handle stops resolving, by the **same mechanism**
    /// [`Self::remove`] uses: each occupied slot's generation is retired, so a
    /// handle naming it no longer matches.
    ///
    /// This used to `Vec::clear` the slots. That reads as the stronger
    /// guarantee — the slot is *gone*, not merely emptied — and is in fact the
    /// weaker one: the next `insert` pushes a fresh slot at index 0 carrying
    /// `FIRST_GENERATION`, which is precisely the handle the first `insert`
    /// before the clear had returned. `insert` → `clear` → `insert` therefore
    /// resolved a stale handle to the new value, which is the ABA this type
    /// exists to prevent, arriving through the one operation that claimed to
    /// prevent it hardest.
    ///
    /// Capacity is retained, as `Vec::clear` retains it.
    ///
    /// # Panics
    ///
    /// Panics if the arena holds more than `u32::MAX` slots, which `insert`
    /// already refuses to create.
    pub fn clear(&mut self) {
        self.free_head = None;
        // Reverse order so the free list comes out ascending, matching the
        // order a fresh arena would hand slots out in.
        for index in (0..self.slots.len()).rev() {
            let slot = &mut self.slots[index];
            // Only an occupied slot needs retiring: a slot already on the free
            // list had its generation bumped when its value was removed, and
            // bumping again would burn generations for nothing.
            if slot.value.take().is_some() {
                slot.generation = next_generation(slot.generation);
            }
            slot.next_free = self.free_head;
            self.free_head =
                Some(u32::try_from(index).expect("an arena holds fewer than u32::MAX"));
        }
        self.length = 0;
    }

    /// Borrows every live value with the handle that names it.
    ///
    /// # Panics
    ///
    /// Panics if the arena holds more than `u32::MAX` slots, which `insert`
    /// already refuses to create.
    pub fn iter(&self) -> impl Iterator<Item = (ArenaRef, &T)> {
        self.slots.iter().enumerate().filter_map(|(index, slot)| {
            let value = slot.value.as_ref()?;
            let index = u32::try_from(index).expect("an arena holds fewer than u32::MAX");
            Some((ArenaRef::new(index, slot.generation.get()), value))
        })
    }

    /// Mutably borrows every live value with the handle that names it.
    ///
    /// # Panics
    ///
    /// Panics if the arena holds more than `u32::MAX` slots, which `insert`
    /// already refuses to create.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (ArenaRef, &mut T)> {
        self.slots
            .iter_mut()
            .enumerate()
            .filter_map(|(index, slot)| {
                let generation = slot.generation.get();
                let value = slot.value.as_mut()?;
                let index = u32::try_from(index).expect("an arena holds fewer than u32::MAX");
                Some((ArenaRef::new(index, generation), value))
            })
    }
}

/// The generation a slot takes once the value in it is removed.
///
/// Wraps back to one rather than zero, so a wrapped slot still never matches a
/// default [`ArenaRef`]. The counter is a full `u32` here; the C++ original
/// spends its top bit on an occupancy flag that `Option` already carries, so
/// this has twice the generations before a wrap can alias a handle.
fn next_generation(generation: NonZeroU32) -> NonZeroU32 {
    generation.checked_add(1).unwrap_or(FIRST_GENERATION)
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> std::fmt::Debug for Arena<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The values are not printed: `T` is not required to be `Debug`, and the
        // free list is an internal detail of how the slots below are reused.
        formatter
            .debug_struct("Arena")
            .field("len", &self.length)
            .field("capacity", &self.slots.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::{Arena, ArenaRef, FIRST_GENERATION, next_generation};

    // The wrap is unreachable from outside the crate — it takes u32::MAX removes
    // of one slot to observe — so it is tested here, against the function that
    // owns it, rather than not at all.
    #[test]
    fn a_generation_at_the_top_of_its_range_wraps_to_one_and_never_to_zero() {
        let last = std::num::NonZeroU32::new(u32::MAX).expect("u32::MAX is not zero");
        assert_eq!(next_generation(last), FIRST_GENERATION);
    }

    #[test]
    fn a_generation_below_the_top_counts_up_by_one() {
        let generation = std::num::NonZeroU32::new(7).expect("7 is not zero");
        assert_eq!(next_generation(generation).get(), 8);
    }

    // A wrapped slot still has to reject the handles it issued in generation one,
    // which is the property the wrap-to-one rule exists to protect.
    #[test]
    fn a_wrapped_slot_still_refuses_a_default_handle() {
        let mut arena = Arena::new();
        let handle = arena.insert(1);
        arena.remove(handle);
        arena.slots[handle.index() as usize].generation =
            std::num::NonZeroU32::new(u32::MAX).expect("u32::MAX is not zero");

        let reused = arena.insert(2);
        assert_eq!(reused.generation(), u32::MAX);
        assert_eq!(arena.get(ArenaRef::default()), None);
    }
}
