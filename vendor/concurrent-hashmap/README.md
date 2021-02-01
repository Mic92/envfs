# rust-concurrent-hashmap

[![Crates.io](https://img.shields.io/crates/v/concurrent-hashmap.svg)](https://crates.io/crates/concurrent-hashmap)

[Documentation](https://veddan.github.io/rustdoc/concurrent-hashmap/concurrent_hashmap/index.html)

This is a Rust implementing a concurrent hashmap.

The crate works on stable Rust if default features are disabled:
```toml
[depdencies.concurrent-hashmap]
version = "0.2.1"
default-features = false
```
However, performance is better with nightly rustc due to use of unstable `#![feature]`s.

## Usage
```rust
extern crate concurrent_hashmap;

use concurrent_hashmap::*;

fn main() {
    // Create a table mapping u32 to u32, using defaults
    let map = ConcHashMap::<u32, u32>::new();
    map.insert(1, 2);
    map.insert(30, 12);
    if let Some(mut val) = map.find_mut(&30) {
        // Update a value in-place if it exists
        // This mapping can not be modified while we have a reference to it
        *val.get() += 3;
    }
    // Update the value with key 129, or insert a default (3)
    map.upsert(129, 3, &|x| *x *= 3);  // 129 => 3
    map.upsert(129, 3, &|x| *x *= 3);  // 129 => 9
    map.remove(&1);
    for (&k, &v) in map.iter() {
        println!("{} => {}", k, v);
    }
}
```

For sharing a map between thread, you typically want to put it in an `Arc`.
A less artificial (and actually multi-threaded) examples can be found in `examples/wordcount.rs`.

## Implementation
This hashtable works by partitioning the keys between several independent hashtable based on
 the initial bits of their hash values.
Each of these partitions is protected by its own lock, so accessing a key in one partition
 does not block access to kes in other partitions.
Under the assumption that the hash function uniformly distributes keys across paritions,
 contention is reduced by a factor equal to the number of partitions.
A key will never move between partitions, so they can be resized independently and without
 locking other partitions.

Each partition is an open-addressed hashtable, using quadratic probing.
Deletion is handled by tombstones and bucket occupancy is tracked by a bitmap.

Single-threaded insertion performance is similar to or better than `std::collections::HashMap`,
 while read performance is worse.

## Concurrency notes
This is not a lock-free hashtable.
To achieve good performance, minimal work should be done while holding locks.
Cases where locks are held include using the result of `.find()`/`.find_mut()`,
 running the updating closure in `.upsert()`, and iterating over the map.
To reduce contention, the `ConcHashMap::with_options()` constructor can be used
 to set the `concurrency` parameter to the expected number of threads concurrently
 accessing the table.

Iterating does not provide a consistent snapshot of the table's contents.
Updates performed while iterating over the table may or may not be reflected in the iteration.
Iterating works by locking a one partition at a time.

