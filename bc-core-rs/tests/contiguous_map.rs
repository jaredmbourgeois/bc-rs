//! `ContiguousMap`, against
//! `libs/foundation/core/test/contiguous_map_test.cpp`.
//!
//! Each test is named for the property it asserts rather than for the C++ case
//! it came from. The tests account for each of the original's 23 cases.

use std::collections::HashSet;

use bc_core::ContiguousMap;

/// A key that is neither an integer nor a string, to prove the bounds are the
/// only thing a key has to satisfy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CustomKey {
    id: i32,
}

#[test]
fn a_new_map_holds_nothing() {
    let map: ContiguousMap<i32, String> = ContiguousMap::new();
    assert!(map.is_empty());
    assert_eq!(map.len(), 0);
}

#[test]
fn a_stored_value_reads_back_through_its_key() {
    let mut map = ContiguousMap::new();
    map.insert(1, "hello".to_owned());
    assert_eq!(map.get(&1).map(String::as_str), Some("hello"));
}

#[test]
fn inserting_a_key_twice_replaces_the_value_rather_than_adding_an_entry() {
    let mut map = ContiguousMap::new();
    map.insert(1, "hello".to_owned());
    map.insert(1, "world".to_owned());

    assert_eq!(map.len(), 1);
    assert_eq!(map.get(&1).map(String::as_str), Some("world"));
}

#[test]
fn removing_a_value_hands_it_back() {
    let mut map = ContiguousMap::new();
    map.insert(1, "hello".to_owned());

    assert_eq!(map.remove(&1).as_deref(), Some("hello"));
    assert_eq!(map.len(), 0);
}

#[test]
fn removing_a_key_that_names_nothing_gives_nothing() {
    let mut map: ContiguousMap<i32, String> = ContiguousMap::new();
    assert_eq!(map.remove(&42), None);
}

#[test]
fn the_entry_swapped_into_a_hole_is_still_reachable_and_still_removable() {
    // This is the whole risk of keeping storage dense: removal moves an entry
    // that nobody asked to move, and its index has to move with it.
    let mut map = ContiguousMap::new();
    map.insert(1, "A".to_owned());
    map.insert(2, "B".to_owned());
    map.insert(3, "C".to_owned());

    map.remove(&1);
    assert_eq!(map.len(), 2);
    assert_eq!(map.get(&1), None);
    assert_eq!(map.get(&2).map(String::as_str), Some("B"));
    assert_eq!(map.get(&3).map(String::as_str), Some("C"));

    // "C" was swapped into the hole at index 0; removing it must still work.
    assert_eq!(map.remove(&3).as_deref(), Some("C"));
    assert_eq!(map.len(), 1);
    assert_eq!(map.get(&2).map(String::as_str), Some("B"));
}

#[test]
fn a_key_reports_whether_its_value_is_still_there() {
    let mut map = ContiguousMap::new();
    map.insert(1, "hello".to_owned());

    assert!(map.contains(&1));
    assert!(!map.contains(&2));

    map.remove(&1);
    assert!(!map.contains(&1));
}

#[test]
fn clearing_drops_every_entry() {
    let mut map = ContiguousMap::new();
    map.insert(1, "A".to_owned());
    map.insert(2, "B".to_owned());
    map.insert(3, "C".to_owned());

    map.clear();

    assert!(map.is_empty());
    assert_eq!(map.len(), 0);
    assert_eq!(map.get(&1), None);
    assert_eq!(map.get(&2), None);
    assert_eq!(map.get(&3), None);
}

#[test]
fn reserving_room_raises_the_capacity_to_at_least_what_was_asked() {
    let mut map: ContiguousMap<i32, i32> = ContiguousMap::new();
    map.reserve(100);
    assert!(map.capacity() >= 100);
}

#[test]
fn shrinking_gives_back_room_without_touching_the_entries() {
    let mut map = ContiguousMap::new();
    for value in 0..100 {
        map.insert(value, value);
    }
    for value in 10..100 {
        map.remove(&value);
    }

    let capacity_before = map.capacity();
    map.shrink_to_fit();

    assert!(map.capacity() <= capacity_before);
    assert_eq!(map.len(), 10);
}

#[test]
fn iteration_yields_every_entry() {
    let mut map = ContiguousMap::new();
    map.insert(1, "A".to_owned());
    map.insert(2, "B".to_owned());
    map.insert(3, "C".to_owned());

    let seen: HashSet<i32> = map.iter().map(|(key, _)| *key).collect();

    assert_eq!(map.iter().count(), 3);
    assert_eq!(seen, HashSet::from([1, 2, 3]));
}

#[test]
fn iteration_after_a_removal_yields_exactly_what_is_left() {
    let mut map = ContiguousMap::new();
    map.insert(1, "A".to_owned());
    map.insert(2, "B".to_owned());
    map.insert(3, "C".to_owned());

    map.remove(&1);

    let seen: HashSet<i32> = map.iter().map(|(key, _)| *key).collect();
    assert_eq!(seen, HashSet::from([2, 3]));
}

#[test]
fn retaining_keeps_the_survivors_in_the_order_they_were_inserted() {
    // The contrast with `remove`, which swaps from the end: `retain` compacts,
    // so what it keeps stays in order.
    let mut map = ContiguousMap::new();
    for (key, value) in [(1, "A"), (2, "B"), (3, "C"), (4, "D")] {
        map.insert(key, value.to_owned());
    }

    map.retain(|key, _| key % 2 != 0);

    assert_eq!(map.len(), 2);
    assert_eq!(map.keys(), &[1, 3]);
    assert_eq!(map.values(), &["A".to_owned(), "C".to_owned()]);
}

#[test]
fn retaining_leaves_the_index_agreeing_with_the_entries_it_kept() {
    let mut map = ContiguousMap::new();
    for (key, value) in [(1, "A"), (2, "B"), (3, "C")] {
        map.insert(key, value.to_owned());
    }

    map.retain(|key, _| *key != 2);

    assert!(!map.contains(&2));
    assert_eq!(map.get(&2), None);
    assert!(map.contains(&1));
    assert!(map.contains(&3));
    assert_eq!(map.get(&1).map(String::as_str), Some("A"));
    assert_eq!(map.get(&3).map(String::as_str), Some("C"));
}

#[test]
fn a_value_can_be_changed_through_its_key() {
    let mut map = ContiguousMap::new();
    map.insert(1, "hello".to_owned());

    *map.get_mut(&1).expect("the value is there") = "modified".to_owned();

    assert_eq!(map.get(&1).map(String::as_str), Some("modified"));
}

#[test]
fn a_value_built_in_place_reads_back_whole() {
    // The canonical `emplace(1, 5, 'x')` forwards constructor arguments, which
    // Rust needs no separate entry point for: the caller builds and moves in.
    let mut map = ContiguousMap::new();
    map.insert(1, "x".repeat(5));
    assert_eq!(map.get(&1).map(String::as_str), Some("xxxxx"));
}

#[test]
fn the_values_are_one_slice_that_can_be_written_through() {
    let mut map = ContiguousMap::new();
    map.insert(1, 10);
    map.insert(2, 20);
    map.insert(3, 30);

    assert_eq!(map.values(), &[10, 20, 30]);

    map.values_mut()[0] = 100;
    assert_eq!(map.get(&1), Some(&100));
}

#[test]
fn the_keys_and_values_slices_line_up_entry_for_entry() {
    let mut map = ContiguousMap::new();
    map.insert(10, "A".to_owned());
    map.insert(20, "B".to_owned());
    map.insert(30, "C".to_owned());

    assert_eq!(map.keys().len(), map.values().len());
    for (position, key) in map.keys().iter().enumerate() {
        assert_eq!(map.get(key), Some(&map.values()[position]));
    }
}

#[test]
fn a_move_only_value_goes_in_and_comes_back_out() {
    let mut map = ContiguousMap::new();
    map.insert(1, Box::new(42));

    assert_eq!(map.get(&1).map(|boxed| **boxed), Some(42));
    assert_eq!(map.remove(&1).map(|boxed| *boxed), Some(42));
}

#[test]
fn string_keys_work_like_any_other() {
    let mut map = ContiguousMap::new();
    map.insert("alpha".to_owned(), 1);
    map.insert("beta".to_owned(), 2);
    map.insert("gamma".to_owned(), 3);

    assert_eq!(map.get(&"beta".to_owned()), Some(&2));
    assert!(map.contains(&"gamma".to_owned()));
    assert!(!map.contains(&"delta".to_owned()));
}

#[test]
fn a_caller_defined_key_type_works_like_any_other() {
    let mut map = ContiguousMap::new();
    map.insert(CustomKey { id: 1 }, "one".to_owned());
    map.insert(CustomKey { id: 2 }, "two".to_owned());

    assert_eq!(
        map.get(&CustomKey { id: 1 }).map(String::as_str),
        Some("one")
    );
    assert!(map.contains(&CustomKey { id: 2 }));
    assert!(!map.contains(&CustomKey { id: 3 }));
}

#[test]
fn shared_iteration_yields_every_entry_without_letting_it_be_changed() {
    // The C++ counterpart asserts the reference is const with a `static_assert`;
    // here that is `iter`'s signature, so what is left to check is the walk.
    let mut map = ContiguousMap::new();
    map.insert(1, 10);
    map.insert(2, 20);

    let shared = &map;
    assert_eq!(shared.iter().count(), 2);
    assert_eq!(shared.iter().map(|(_, value)| *value).sum::<i32>(), 30);
}

#[test]
fn iterating_an_empty_map_yields_nothing() {
    let map: ContiguousMap<i32, i32> = ContiguousMap::new();
    assert_eq!(map.iter().count(), 0);
}
