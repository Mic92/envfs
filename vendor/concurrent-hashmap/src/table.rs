use std::hash::Hash;
use spin::MutexGuard;
use std::ptr::{self, drop_in_place};
use std::mem;
use std::cmp::max;
use std::mem::size_of;
use std::marker::{Send, Sync};

// This is the actual hash table implementation.
// The Table struct does not have any synchronization; that is handled by the ConHashMap wrapper.
// It uses open addressing with quadratic probing, with a bitmap for tracking bucket occupancy,
// and uses tombstones to track deleted entries.

// Minimum size of table when resizing.
// Initially, zero-sized tables are allowed to avoid allocation.
// When they need to reallocate, this is the smallest size used.
const MIN_CAPACITY: usize = 1 << 5;

// Largest number of elements in a table.
// We want to be able to use the top 16 bits of the hash for choosing the partition.
// If we limit the size of the partition to 47 bits, elements will never change partition.
// Thus we can resize each partition individually.
const MAX_CAPACITY: u64 = (1 << 48) - 1;

// This masks out the metadata bits of the hash field.
const HASH_MASK: u64 = 0x0000FFFFFFFFFFFF;

// If this bit is in a stored hash, the entry entry has been removed.
const TOMBSTONE: u64 = 0x0001000000000000;

// If this bit is in a stored hash, the entry entry is present.
const PRESENT: u64 = 0x1000000000000000;

// The proper heap API is only available in nightlies
unsafe fn alloc<T>(count: usize, zero: bool) -> *mut T {
    let mut dummy: Vec<T> = Vec::with_capacity(count);
    let ptr = dummy.as_mut_ptr();
    if zero {
        ptr::write_bytes(ptr, 0, count);
    }
    mem::forget(dummy);
    return ptr;
}

unsafe fn dealloc<T>(p: *mut T, count: usize) {
    let _dummy: Vec<T> = Vec::from_raw_parts(p, 0, count);
    // Dummy is dropped and the memory is freed
}

pub struct Table<K, V> {
    hashes: *mut u64,
    keys: *mut K,
    values: *mut V,
    capacity: usize,
    len: usize,
}

/// A handle to a particular mapping.
///
/// Note that this acts as a lock guard to a part of the map.
pub struct Accessor<'a, K: 'a, V: 'a> {
    table: MutexGuard<'a, Table<K, V>>,
    idx: usize
}

/// A mutable handle to a particular mapping.
///
/// Note that this acts as a lock guard to a part of the map.
pub struct MutAccessor<'a, K: 'a, V: 'a> {
    table: MutexGuard<'a, Table<K, V>>,
    idx: usize
}

impl <'a, K, V> Accessor<'a, K, V> {
    pub fn new(table: MutexGuard<'a, Table<K, V>>, idx: usize) -> Accessor<'a, K, V> {
        Accessor {
            table: table,
            idx: idx
        }
    }

    pub fn get(&self) -> &'a V {
        debug_assert!(self.table.is_present(self.idx));
        unsafe {
            &*self.table.values.offset(self.idx as isize)
        }
    }
}

impl <'a, K, V> MutAccessor<'a, K, V> {
    pub fn new(table: MutexGuard<'a, Table<K, V>>, idx: usize) -> MutAccessor<'a, K, V> {
        MutAccessor {
            table: table,
            idx: idx
        }
    }

    pub fn get(&mut self) -> &'a mut V {
        debug_assert!(self.table.is_present(self.idx));
        unsafe {
            &mut *self.table.values.offset(self.idx as isize)
        }
    }
}

impl <K, V> Table<K, V> where K: Hash + Eq {
    pub fn new(capacity: usize) -> Table<K, V> {
        assert!(size_of::<K>() > 0 && size_of::<V>() > 0, "zero-size types not yet supported");
        let capacity = if capacity == 0 { 0 } else { capacity.next_power_of_two() };
        Table {
            capacity: capacity,
            len: 0,
            hashes: unsafe { alloc(capacity, true) },
            keys: unsafe { alloc(capacity, false) },
            values: unsafe { alloc(capacity, false) }
        }
    }

    pub fn lookup<C>(&self, hash: u64, eq: C) -> Option<usize> where C: Fn(&K) -> bool {
        let len = self.capacity;
        if len == 0 {
            return None;
        }
        let mask = len - 1;
        let hash = hash & HASH_MASK;
        let mut i = hash as usize & mask;
        let mut j = 0;
        loop {
            if self.is_present(i) && self.compare_key_at(&eq, i) {
                return Some(i);
            }
            if !self.is_present(i) && !self.is_deleted(i) {
                // The key we're searching for would have been placed here if it existed
                return None;
            }
            if i == len - 1 { return None; }
            j += 1;
            i = (i + j) & mask;
        }
    }

    pub fn put<T, U: Fn(&mut V, V)-> T>(&mut self, key: K, value: V, hash: u64, update: U) -> Option<T> {
        if self.capacity == 0 {
            self.resize();
        }
        loop {
            let len = self.capacity;
            let hash = hash & HASH_MASK;
            let mask = len - 1;
            let mut i = (hash as usize) & mask;
            let mut j = 0;
            loop {
                if !self.is_present(i) {
                    unsafe { self.put_at_empty(i, key, value, hash); }
                    self.len += 1;
                    return None;
                } else if self.compare_key_at(&|k| k == &key, i) {
                    let old_value = unsafe { &mut *self.values.offset(i as isize) };
                    return Some(update(old_value, value));
                }
                if i == len - 1 { break; }
                j += 1;
                i = (i + j) & mask;
            }
            self.resize();
        }
    }

    pub fn remove<C>(&mut self, hash: u64, eq: C) -> Option<V> where C: Fn(&K) -> bool {
        let i = match self.lookup(hash, eq) {
            Some(i) => i,
            None    => return None
        };
        unsafe {
            drop_in_place::<K>(self.keys.offset(i as isize));
            *self.hashes.offset(i as isize) = TOMBSTONE;
            self.len -= 1;
            let value = ptr::read(self.values.offset(i as isize));
            return Some(value);
        }
    }

    #[inline]
    fn compare_key_at<C>(&self, eq: &C, idx: usize) -> bool where C: Fn(&K) -> bool {
        assert!(self.is_present(idx));
        unsafe { eq(&*self.keys.offset(idx as isize)) }
    }

    unsafe fn put_at_empty(&mut self, idx: usize, key: K, value: V, hash: u64) {
        let i = idx as isize;
        *self.hashes.offset(i) = hash | PRESENT;
        ptr::write(self.keys.offset(i), key);
        ptr::write(self.values.offset(i), value);
    }

    fn resize(&mut self) {
        let new_capacity = max(self.capacity.checked_add(self.capacity).expect("size overflow"), MIN_CAPACITY);
        if new_capacity as u64 > MAX_CAPACITY {
            panic!("requested size: {}, max size: {}", new_capacity, MAX_CAPACITY);
        }
        let mut new_table = Table::new(new_capacity);
        unsafe {
            self.foreach_present_idx(|i| {
                let hash: u64 = *self.hashes.offset(i as isize);
                new_table.put(ptr::read(self.keys.offset(i as isize)),
                              ptr::read(self.values.offset(i as isize)),
                              hash, |_, _| { });
            });
            dealloc(self.hashes, self.capacity);
            dealloc(self.keys, self.capacity);
            dealloc(self.values, self.capacity);
            // This is checked in drop() to see that this instance is already "dropped"
            self.hashes = ptr::null_mut();
        }
        mem::swap(self, &mut new_table);
    }

//     fn _dump_table(&self) {
//         unsafe {
//             let table = ::std::slice::from_raw_parts(self.buckets, self.capacity);
//             for (i, e) in table.iter().enumerate() {
//                 if self.present[i] {
//                     println!("{}:\t{:?}\t=>\t{:?}",
//                             i, e.key, e.value,);
//                 } else {
//                     println!("{}:\tempty", i);
//                 }
//             }
//         }
//     }
}

impl <K, V> Table<K, V> {
    pub fn capacity(&self) -> usize { self.capacity }

    /// Used to implement iteration.
    /// Search for a present bucket >= idx.
    /// If one is found, Some(..) is returned and idx is set to a value
    /// that can be passed back to iter_advance to look for the next bucket.
    /// When all bucket have been scanned, idx is set to self.capacity.
    pub fn iter_advance<'a>(&'a self, idx: &mut usize) -> Option<(&'a K, &'a V)> {
        if *idx >= self.capacity {
            return None;
        }
        for i in *idx..self.capacity {
            if self.is_present(i) {
                *idx = i + 1;
                let entry = unsafe {
                    let key = self.keys.offset(i as isize);
                    let value = self.values.offset(i as isize);
                    (&*key, &*value)
                };
                return Some(entry);
            }
        }
        *idx = self.capacity;
        return None;
    }

    pub fn clear(&mut self) {
        self.foreach_present_idx(|i| {
            unsafe {
                drop_in_place::<K>(self.keys.offset(i as isize));
                drop_in_place::<V>(self.values.offset(i as isize));
            }
        });
        unsafe {
            ptr::write_bytes(self.hashes, 0, self.capacity);
        }
        self.len = 0;
    }

    fn is_present(&self, idx: usize) -> bool {
        assert!(idx < self.capacity);
        self.hash_at(idx) & PRESENT != 0
    }

    fn is_deleted(&self, idx: usize) -> bool {
        assert!(idx < self.capacity);
        !self.is_present(idx) && self.hash_at(idx) & TOMBSTONE != 0
    }

    fn hash_at(&self, idx: usize) -> u64 {
        assert!(idx < self.capacity);
        unsafe { *self.hashes.offset(idx as isize) }
    }

    fn foreach_present_idx<F>(&self, mut f: F) where F: FnMut(usize) {
        let mut seen = 0;
        for i in 0..self.capacity {
            if seen == self.len {
                return;
            }
            if self.is_present(i) {
                seen += 1;
                f(i);
            }
        }
    }
}

impl <K, V> Drop for Table<K, V> {
    fn drop(&mut self) {
        if self.hashes.is_null() {
            // "Dying" instance that has been resized
            return;
        }
        self.foreach_present_idx(|i| {
            unsafe {
                drop_in_place::<K>(self.keys.offset(i as isize));
                drop_in_place::<V>(self.values.offset(i as isize));
            }
        });
        unsafe {
            dealloc(self.hashes, self.capacity);
            dealloc(self.keys, self.capacity);
            dealloc(self.values, self.capacity);
        }
    }
}

unsafe impl <K, V> Sync for Table<K, V> where K: Send + Sync, V: Send + Sync { }

unsafe impl <K, V> Send for Table<K, V> where K: Send, V: Send { }
