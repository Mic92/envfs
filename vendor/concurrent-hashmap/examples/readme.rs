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
