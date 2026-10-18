//! The generational arena, against `libs/foundation/core/test/arena_test.cpp`.
//!
//! Each test is named for the property it asserts rather than for the C++ case
//! it came from, so a name that stops being true is visible without opening the
//! body. The tests account for each of the original's 29 cases.

use std::collections::HashSet;

use bc_core::{Arena, ArenaRef};

// --- ArenaRef ---

#[test]
fn a_default_handle_is_index_zero_generation_zero() {
    let handle = ArenaRef::default();
    assert_eq!(handle.index(), 0);
    assert_eq!(handle.generation(), 0);
}

#[test]
fn a_handle_reports_the_parts_it_was_built_from() {
    let handle = ArenaRef::new(42, 7);
    assert_eq!(handle.index(), 42);
    assert_eq!(handle.generation(), 7);
}

#[test]
fn handles_differ_when_either_part_differs() {
    assert_eq!(ArenaRef::new(1, 2), ArenaRef::new(1, 2));
    assert_ne!(ArenaRef::new(1, 2), ArenaRef::new(1, 3));
    assert_ne!(ArenaRef::new(1, 2), ArenaRef::new(2, 2));
}

#[test]
fn a_handle_survives_a_round_trip_through_one_integer() {
    let original = ArenaRef::new(12_345, 67_890);
    assert_eq!(ArenaRef::from_u64(original.to_u64()), original);
}

#[test]
fn the_integer_round_trip_holds_at_the_top_of_both_halves() {
    // The halves must not bleed into each other: this is the case that catches a
    // sign extension or a mask that stops at 31 bits.
    let original = ArenaRef::new(u32::MAX, u32::MAX);
    assert_eq!(ArenaRef::from_u64(original.to_u64()), original);
}

#[test]
fn equal_handles_hash_together_and_unequal_ones_do_not() {
    let mut set = HashSet::new();
    set.insert(ArenaRef::new(1, 1));
    set.insert(ArenaRef::new(2, 1));
    set.insert(ArenaRef::new(1, 1));
    assert_eq!(set.len(), 2);
}

// fidelity-compile-proof: `ArenaRefTest.TriviallyCopyable` and
// `ArenaRefTest.SizeIs8Bytes` are `static_assert`s in the canonical source, so
// they are proved the same way here — by the compiler, not at run time.
const _: () = {
    assert!(size_of::<ArenaRef>() == 8);
};

fn _a_handle_is_copy(handle: ArenaRef) -> (ArenaRef, ArenaRef) {
    (handle, handle)
}

// --- Arena ---

#[test]
fn a_new_arena_holds_nothing_and_has_taken_no_slots() {
    let arena: Arena<i32> = Arena::new();
    assert!(arena.is_empty());
    assert_eq!(arena.len(), 0);
    assert_eq!(arena.capacity(), 0);
}

#[test]
fn a_stored_value_reads_back_through_its_handle() {
    let mut arena = Arena::new();
    let handle = arena.insert(42);
    assert_eq!(arena.get(handle), Some(&42));
}

#[test]
fn each_of_several_values_reads_back_through_its_own_handle() {
    let mut arena = Arena::new();
    let first = arena.insert("hello".to_owned());
    let second = arena.insert("world".to_owned());
    let third = arena.insert("foo".to_owned());

    assert_eq!(arena.get(first).map(String::as_str), Some("hello"));
    assert_eq!(arena.get(second).map(String::as_str), Some("world"));
    assert_eq!(arena.get(third).map(String::as_str), Some("foo"));
    assert_eq!(arena.len(), 3);
}

#[test]
fn removing_a_value_hands_it_back() {
    let mut arena = Arena::new();
    let handle = arena.insert(42);
    assert_eq!(arena.remove(handle), Some(42));
    assert_eq!(arena.len(), 0);
}

#[test]
fn a_handle_stops_resolving_once_its_value_is_removed() {
    let mut arena = Arena::new();
    let handle = arena.insert(42);
    arena.remove(handle);

    assert_eq!(arena.get(handle), None);
    assert!(!arena.contains(handle));
}

#[test]
fn a_stale_handle_cannot_remove_the_value_that_took_its_slot() {
    let mut arena = Arena::new();
    let stale = arena.insert(42);
    arena.remove(stale);
    let live = arena.insert(99);

    // The slot is the same one; only the generation separates the two handles.
    assert_eq!(stale.index(), live.index());
    assert_eq!(arena.remove(stale), None);
    assert_eq!(arena.get(live), Some(&99));
}

#[test]
fn a_freed_slot_is_reused_rather_than_the_arena_growing() {
    let mut arena = Arena::new();
    let first = arena.insert(1);
    let second = arena.insert(2);
    let third = arena.insert(3);
    let slots_before = arena.capacity();

    arena.remove(second);
    let fourth = arena.insert(4);

    assert_eq!(arena.capacity(), slots_before, "the arena took a new slot");
    assert_eq!(fourth.index(), second.index());
    assert!(fourth.generation() > second.generation());
    assert_eq!(arena.get(fourth), Some(&4));
    assert_eq!(arena.get(first), Some(&1));
    assert_eq!(arena.get(third), Some(&3));
}

#[test]
fn a_slots_generation_counts_up_by_one_each_time_it_is_reused() {
    let mut arena = Arena::new();
    let first = arena.insert(1);
    assert_eq!(first.generation(), 1);

    arena.remove(first);
    let second = arena.insert(2);
    assert_eq!(second.generation(), 2);

    arena.remove(second);
    let third = arena.insert(3);
    assert_eq!(third.generation(), 3);
}

#[test]
fn the_length_tracks_inserts_and_removes() {
    let mut arena = Arena::new();
    assert_eq!(arena.len(), 0);
    assert!(arena.is_empty());

    let first = arena.insert(1);
    arena.insert(2);
    assert_eq!(arena.len(), 2);
    assert!(!arena.is_empty());

    arena.remove(first);
    assert_eq!(arena.len(), 1);
}

#[test]
fn clearing_releases_every_slot_and_invalidates_every_handle() {
    let mut arena = Arena::new();
    let first = arena.insert(1);
    let second = arena.insert(2);
    let third = arena.insert(3);

    arena.clear();

    assert_eq!(arena.len(), 0);
    assert!(arena.is_empty());
    assert_eq!(arena.get(first), None);
    assert_eq!(arena.get(second), None);
    assert_eq!(arena.get(third), None);
    // Capacity is *retained*, as `Vec::clear` retains it. It used to be
    // released, and that was the bug: releasing the slots released the
    // generations with them, so the next handle issued was equal to a handle
    // this test had just watched stop resolving — see the test below.
    assert!(arena.capacity() >= 3);
}

/// The ABA the generations exist to stop, on the operation that used to throw
/// them away.
///
/// `clear` reset the slot vector, so the next `insert` pushed a fresh slot at
/// index 0 with the first generation — byte-for-byte the handle the first
/// `insert` before the clear had returned. Every assertion in the test above
/// still passed, because it never inserted after clearing.
#[test]
fn a_handle_from_before_a_clear_never_resolves_to_a_value_inserted_after_it() {
    let mut arena = Arena::new();
    let stale = arena.insert(1);

    arena.clear();
    let fresh = arena.insert(2);

    assert_ne!(
        stale, fresh,
        "a handle issued after a clear is not one issued before it"
    );
    assert_eq!(
        arena.get(stale),
        None,
        "the stale handle names nothing, not the value that took its slot"
    );
    assert_eq!(arena.get(fresh), Some(&2));
    assert_eq!(
        arena.remove(stale),
        None,
        "and it cannot take the new value out either"
    );
}

/// Clearing twice, and clearing after a remove, keep the invariant.
///
/// The free list is rebuilt from scratch by `clear`, so a slot already on it
/// must not be retired twice or enter the list twice — either would hand out a
/// slot that is still linked, or burn a generation for nothing.
#[test]
fn clearing_an_arena_that_already_had_holes_still_invalidates_everything() {
    let mut arena = Arena::new();
    let first = arena.insert(1);
    let second = arena.insert(2);
    assert_eq!(arena.remove(first), Some(1));

    arena.clear();
    assert_eq!(arena.get(second), None);

    let a = arena.insert(10);
    let b = arena.insert(20);
    assert_ne!(a, b, "two live handles are never equal");
    assert_eq!(arena.get(a), Some(&10));
    assert_eq!(arena.get(b), Some(&20));
    assert_eq!(arena.len(), 2);
}

#[test]
fn a_handle_past_the_end_resolves_to_nothing_rather_than_panicking() {
    let mut arena = Arena::new();
    arena.insert(1);

    let past_the_end = ArenaRef::new(999, 1);
    assert_eq!(arena.get(past_the_end), None);
    assert!(!arena.contains(past_the_end));
    assert_eq!(arena.remove(past_the_end), None);
}

#[test]
fn a_default_handle_never_matches_a_live_value() {
    let mut arena = Arena::new();
    arena.insert(42);

    // This is what lets a default handle stand in for a null one: generations
    // start at 1, so generation 0 cannot name anything the arena issued.
    let null = ArenaRef::default();
    assert_eq!(arena.get(null), None);
    assert!(!arena.contains(null));
}

#[test]
fn a_value_can_be_changed_through_its_handle() {
    let mut arena = Arena::new();
    let handle = arena.insert(42);

    *arena.get_mut(handle).expect("the value is live") = 99;

    assert_eq!(arena.get(handle), Some(&99));
}

#[test]
fn a_value_built_in_place_reads_back_whole() {
    // The canonical `emplace(5, 'x')` forwards constructor arguments, which Rust
    // needs no separate entry point for: the value is built by the caller and
    // moved in.
    let mut arena = Arena::new();
    let handle = arena.insert("x".repeat(5));
    assert_eq!(arena.get(handle).map(String::as_str), Some("xxxxx"));
}

#[test]
fn a_move_only_value_goes_in_and_comes_back_out() {
    let mut arena = Arena::new();
    let handle = arena.insert(Box::new(42));

    assert_eq!(arena.get(handle).map(|boxed| **boxed), Some(42));
    assert_eq!(arena.remove(handle).map(|boxed| *boxed), Some(42));
}

#[test]
fn iteration_yields_every_live_value_with_the_handle_that_names_it() {
    let mut arena = Arena::new();
    let first = arena.insert(10);
    let second = arena.insert(20);
    let third = arena.insert(30);
    arena.remove(second);

    let items: Vec<(ArenaRef, i32)> = arena
        .iter()
        .map(|(handle, value)| (handle, *value))
        .collect();

    assert_eq!(items.len(), 2);
    assert_eq!(items.len(), arena.len());
    assert!(items.contains(&(first, 10)));
    assert!(items.contains(&(third, 30)));
}

#[test]
fn iteration_skips_the_slots_whose_values_were_removed() {
    let mut arena = Arena::new();
    arena.insert(1);
    let second = arena.insert(2);
    arena.insert(3);
    let fourth = arena.insert(4);
    arena.insert(5);

    arena.remove(second);
    arena.remove(fourth);

    let values: Vec<i32> = arena.iter().map(|(_, value)| *value).collect();

    assert_eq!(values.len(), 3);
    assert!(!values.contains(&2));
    assert!(!values.contains(&4));
}

#[test]
fn iterating_an_empty_arena_yields_nothing() {
    let arena: Arena<i32> = Arena::new();
    assert_eq!(arena.iter().count(), 0);
}

#[test]
fn a_handle_reports_whether_its_value_is_still_there() {
    let mut arena = Arena::new();
    let handle = arena.insert(42);
    assert!(arena.contains(handle));

    arena.remove(handle);
    assert!(!arena.contains(handle));
}

#[test]
fn a_preallocated_arena_takes_no_extra_slots_filling_to_its_capacity() {
    let mut arena = Arena::with_capacity(100);
    for value in 0..100 {
        arena.insert(value);
    }
    assert_eq!(arena.len(), 100);
    assert_eq!(arena.capacity(), 100);
}

#[test]
fn every_live_value_can_be_changed_through_iteration() {
    // No canonical counterpart: `iter_mut` is this port's own surface, so it
    // carries its own test rather than riding on the const-iteration case.
    let mut arena = Arena::new();
    arena.insert(1);
    let second = arena.insert(2);
    arena.remove(second);
    arena.insert(3);

    for (_, value) in arena.iter_mut() {
        *value *= 10;
    }

    let mut values: Vec<i32> = arena.iter().map(|(_, value)| *value).collect();
    values.sort_unstable();
    assert_eq!(values, [10, 30]);
}
