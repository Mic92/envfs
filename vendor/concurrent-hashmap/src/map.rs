use std::hash::{Hasher, Hash};
use std::hash::BuildHasher;
use std::collections::hash_map::RandomState;
use spin::{Mutex, MutexGuard};
use std::default::Default;
use std::mem::swap;
use std::cmp::min;
use std::u16;
use std::borrow::Borrow;
use std::iter::{FromIterator, IntoIterator};
use table::*;

// This is the user-facing part of the implementation.
// ConcHashMap wraps a couple of actual hash tables (Table) with locks around them.
// It uses the top bits of the hash to decide which Table to access for a given key.
// The size of an invidual Table is limited (to a still unreasonably large value) so
// that it will never use the forementioned to bits of the hash.
// That means that resizing a Table will never cause a key to cross between Tables.
// Therefore each table can be resized independently.

/// A concurrent hashmap using sharding
pub struct ConcHashMap<K, V, H=RandomState> where K: Send + Sync, V: Send + Sync {
    tables: Vec<Mutex<Table<K, V>>>,
    hasher_factory: H,
    table_shift: u64,
    table_mask: u64,
}

impl <K, V, H> ConcHashMap<K, V, H>
        where K: Hash + Eq + Send + Sync, V: Send + Sync, H: BuildHasher {

    /// Creates a new hashmap using default options.
    pub fn new() -> ConcHashMap<K, V> {
        Default::default()
    }

    /// Creates a new hashmap with custom options.
    pub fn with_options(opts: Options<H>) -> ConcHashMap<K, V, H> {
        let conc = opts.concurrency as usize;
        let partitions = conc.checked_next_power_of_two().unwrap_or((conc / 2).next_power_of_two());
        let capacity = f64_to_usize(opts.capacity as f64 / 0.92).expect("capacity overflow");
        let reserve = div_ceil(capacity, partitions);
        let mut tables = Vec::with_capacity(partitions);
        for _ in 0..partitions {
            tables.push(Mutex::new(Table::new(reserve)));
        }
        ConcHashMap {
            tables: tables,
            hasher_factory: opts.hasher_factory,
            table_shift: if partitions == 1 { 0 } else { 64 - partitions.trailing_zeros() as u64 },
            table_mask: partitions as u64 - 1
        }
    }

    /// Searches for a key, returning an accessor to the mapped values (or `None` if no mapping
    /// exists).
    ///
    /// Note that as long as the `Accessor` lives, a lock is held.
    ///
    /// # Examples
    ///
    /// Printing a value if it exists:
    ///
    /// ```
    /// # use concurrent_hashmap::*;
    /// # let map = ConcHashMap::<u32, u32>::new();
    /// map.insert(100, 1);
    /// if let Some(val) = map.find(&100) {
    ///     println!("100 => {}", val.get());
    /// }
    /// # println!("workaround");
    /// ```
    #[inline(never)]
    pub fn find<'a, Q: ?Sized>(&'a self, key: &Q) -> Option<Accessor<'a, K, V>>
            where K: Borrow<Q> + Hash + Eq + Send + Sync, Q: Hash + Eq + Sync {
        let hash = self.hash(key);
        let table_idx = self.table_for(hash);
        let table = self.tables[table_idx].lock();
        match table.lookup(hash, |k| k.borrow() == key) {
            Some(idx) => Some(Accessor::new(table, idx)),
            None      => None
        }
    }

    /// Searches for a key, returning a mutable accessor to the mapped value
    /// (or `None` if no mapping exists).
    ///
    /// Note that as long as the `MutAccessor` lives, a lock is held.
    ///
    /// # Examples
    ///
    /// Adding 2 to a value if it exists:
    ///
    /// ```
    /// # use concurrent_hashmap::*;
    /// # let map = ConcHashMap::<u32, u32>::new();
    /// map.insert(100, 1);
    /// if let Some(mut val) = map.find_mut(&100) {
    ///     *val.get() += 2;
    /// }
    /// # println!("workaround");
    /// ```
    #[inline(never)]
    pub fn find_mut<'a, Q: ?Sized>(&'a self, key: &Q) -> Option<MutAccessor<'a, K, V>>
            where K: Borrow<Q> + Hash + Eq + Send + Sync, Q: Hash + Eq + Sync {
        let hash = self.hash(key);
        let table_idx = self.table_for(hash);
        let table = self.tables[table_idx].lock();
        match table.lookup(hash, |k| k.borrow() == key) {
            Some(idx) => Some(MutAccessor::new(table, idx)),
            None      => None
        }
    }

    /// Inserts a new mapping from `key` to `value`.
    /// If a previous mapping existed for `key`, it is returned.
    #[inline(never)]
    pub fn insert(&self, key: K, value: V) -> Option<V> {
        let hash = self.hash(&key);
        let table_idx = self.table_for(hash);
        let mut table = self.tables[table_idx].lock();
        table.put(key, value, hash, |old, mut new| { swap(old, &mut new); new })
    }

    /// Performs on "upsert" operation:
    /// Updates the value currently mapped to `key` using `updater`,
    /// or maps `key` to `value` if no previous mapping existed.
    ///
    /// # Examples
    /// ```
    /// # use concurrent_hashmap::*;
    /// # use std::string::String;
    /// let word_counts = ConcHashMap::<String, u32>::new();
    /// let words = ["a", "car", "is", "a", "thing"];
    /// for word in words.iter().map(|s| s.to_string()) {
    ///     word_counts.upsert(word, 1, &|count| *count += 1);
    /// }
    /// // Map is now "a"=>2, "car"=>1, "thing"=>1
    /// ```
    pub fn upsert<U: Fn(&mut V)>(&self, key: K, value: V, updater: &U) {
        let hash = self.hash(&key);
        let table_idx = self.table_for(hash);
        let mut table = self.tables[table_idx].lock();
        table.put(key, value, hash, |old, _| { updater(old); });
    }

    /// Removes any mapping associated with `key`.
    ///
    /// If a mapping was removed, the mapped values is returned.
    pub fn remove<'a, Q: ?Sized>(&'a self, key: &Q) -> Option<V>
            where K: Borrow<Q> + Hash + Eq + Send + Sync, Q: Hash + Eq + Sync {
        let hash = self.hash(key);
        let table_idx = self.table_for(hash);
        let mut table = self.tables[table_idx].lock();
        table.remove(hash, |k| k.borrow() == key)
    }

    fn table_for(&self, hash: u64) -> usize {
        ((hash >> self.table_shift) & self.table_mask) as usize
    }

    fn hash<Q: ?Sized>(&self, key: &Q) -> u64
            where K: Borrow<Q> + Hash + Eq + Send + Sync, Q: Hash + Eq + Sync {
        let mut hasher = self.hasher_factory.build_hasher();
        key.hash(&mut hasher);
        hasher.finish()
    }
}

impl <K, V, H> Clone for ConcHashMap<K, V, H>
        where K: Hash + Eq + Send + Sync + Clone, V: Send + Sync + Clone, H: BuildHasher + Clone {
    /// Clones the hashmap, returning a new map with the same mappings and hasher.
    ///
    /// If a consistent snapshot is desired, external synchronization is required.
    /// In the absence of external synchronization, this method has the same consistency guarantees
    /// as .iter().
    fn clone(&self) -> ConcHashMap<K, V, H> {
        let clone = ConcHashMap::<K, V, H>::with_options(Options {
            capacity: 16,  // TODO
            hasher_factory: self.hasher_factory.clone(),
            concurrency: min(u16::MAX as usize, self.tables.len()) as u16
        });
        for (k, v) in self.iter() {
            clone.insert(k.clone(), v.clone());
        }
        return clone;
    }
}

impl <K, V, H> FromIterator<(K, V)> for ConcHashMap<K, V, H>
        where K: Eq + Hash + Send + Sync, V: Send + Sync, H: BuildHasher + Default {
    fn from_iter<T>(iterator: T) -> Self where T: IntoIterator<Item=(K, V)> {
        let iterator = iterator.into_iter();
        let mut options: Options<H> = Default::default();
        if let (_, Some(bound)) = iterator.size_hint() {
            options.capacity = bound;
        }
        let map = ConcHashMap::with_options(options);
        for (k, v) in iterator {
            map.insert(k, v);
        }
        return map;
    }
}

impl <K, V, H> ConcHashMap<K, V, H> where K: Send + Sync, V: Send + Sync {
    /// Iterates over all mappings.
    ///
    /// This method does not provide a consistent snapshot of the map.
    /// All mappings returned must have been in the map at some point, but updates performed during
    /// the iteration may or may not be reflected.
    ///
    /// Iterating may block writers.
    pub fn iter<'a>(&'a self) -> Entries<'a, K, V, H> {
       Entries {
           map: self,
           table: self.tables[0].lock(),
           table_idx: 0,
           bucket: 0
       }
    }

    /// Removes all mappings.
    ///
    /// In the absence of external synchronization, the map can not be guaranteed to have been empty
    /// at any point during or after the `.clear()` call.
    pub fn clear(&self) {
        for table in self.tables.iter() {
            table.lock().clear();
        }
    }
}

impl <K, V, H> Default for ConcHashMap<K, V, H>
        where K: Hash + Eq + Send + Sync, V: Send + Sync, H: BuildHasher + Default {
    /// Equivalent to `ConcHashMap::new()`.
    fn default() -> ConcHashMap<K, V, H> {
        ConcHashMap::with_options(Default::default())
    }
}

/// Iterator over the hashmap's mappings.
pub struct Entries<'a, K, V, H> where K: 'a + Send + Sync, V: 'a + Send + Sync, H: 'a {
    map: &'a ConcHashMap<K, V, H>,
    table: MutexGuard<'a, Table<K, V>>,
    table_idx: usize,
    bucket: usize,
}

impl <'a, K, V, H> Entries<'a, K, V, H> where K: Send + Sync, V: Send + Sync  {
    fn next_table(&mut self) {
        self.table_idx += 1;
        self.table = self.map.tables[self.table_idx].lock();
        self.bucket = 0;
    }
}

impl <'a, K, V, H> Iterator for Entries<'a, K, V, H> where K: Send + Sync, V: Send + Sync {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<(&'a K, &'a V)> {
        loop {
            if self.bucket == self.table.capacity() {
                if self.table_idx + 1 == self.map.tables.len() {
                    return None;
                }
                self.next_table();
            }
            let res: Option<(&'a K, &'a V)> = unsafe { ::std::mem::transmute(self.table.iter_advance(&mut self.bucket)) };
            match res {
                Some(e) => return Some(e),
                None    => {
                    if self.table_idx + 1 == self.map.tables.len() {
                        return None;
                    }
                    self.next_table()
                }
            }
        }
    }
}

/// Options used when creating a hashmap.
pub struct Options<H> {
    /// Number of mappings to preallocate space for.
    ///
    /// The map will always grow as needed, but preallocating space can improve performance.
    /// This value applies to the entire map.
    /// By default, no space is preallocated.
    pub capacity: usize,
    /// Factory for the hasher used for hashing keys.
    pub hasher_factory: H,
    /// Expected level of concurrency.
    ///
    /// This value controls the number of partitions used internally in the map.
    /// A higher value leads to less contention, but also greater memory overhead.
    /// The default value is 16.
    pub concurrency: u16,
}

impl <H> Default for Options<H> where H: BuildHasher+Default {
    fn default() -> Options<H> {
        Options {
            capacity: 0,
            hasher_factory: Default::default(),
            concurrency: 16
        }
    }
}

fn div_ceil(n: usize, d: usize) -> usize {
    if n == 0 {
        0
    } else {
        n/d + if n % d == 0 { 1 } else { 0 }
    }
}

fn f64_to_usize(f: f64) -> Option<usize> {
    if f.is_nan() || f.is_sign_negative() || f > ::std::usize::MAX as f64 {
        None
    } else {
        Some(f as usize)
    }
}

#[cfg(test)]
mod test {
    use std::hash::Hash;
    use std::hash::{BuildHasher, Hasher, BuildHasherDefault};
    use std::default::Default;
    use std::fmt::Debug;
    use std::thread;
    use std::sync::Arc;
    use super::*;

    struct BadHasher;

    impl Hasher for BadHasher {
        fn write(&mut self, _: &[u8]) { }

        fn finish(&self) -> u64 { 0 }
    }

    impl Default for BadHasher {
        fn default() -> BadHasher { BadHasher }
    }

    struct OneAtATimeHasher {
        state: u64
    }

    impl Hasher for OneAtATimeHasher {
        fn write(&mut self, bytes: &[u8]) {
            for &b in bytes.iter() {
                self.state = self.state.wrapping_add(b as u64);
                self.state = self.state.wrapping_add(self.state << 10);
                self.state ^= self.state >> 6;
            }
        }

        fn finish(&self) -> u64 {
            let mut hash = self.state;
            hash = hash.wrapping_add(hash << 3);
            hash ^= hash >> 11;
            hash = hash.wrapping_add(hash << 15);
            hash
        }
    }

    impl Default for OneAtATimeHasher {
        fn default() -> OneAtATimeHasher {
            OneAtATimeHasher { state: 0x124C494467744825 }
        }
    }

    #[test]
    fn insert_is_found() {
        let map: ConcHashMap<i32, i32> = Default::default();
        assert!(map.find(&1).is_none());
        map.insert(1, 2);
        assert_eq!(map.find(&1).unwrap().get(), &2);
        assert!(map.find(&2).is_none());
        map.insert(2, 4);
        assert_eq!(map.find(&2).unwrap().get(), &4);
    }

    #[test]
    fn insert_replace() {
        let map: ConcHashMap<i32, &'static str> = Default::default();
        assert!(map.find(&1).is_none());
        map.insert(1, &"old");
        assert_eq!(map.find(&1).unwrap().get(), &"old");
        let old = map.insert(1, &"new");
        assert_eq!(Some("old"), old);
        assert_eq!(map.find(&1).unwrap().get(), &"new");
    }

    #[test]
    fn insert_lots() {
        let map: ConcHashMap<i32, i32, BuildHasherDefault<OneAtATimeHasher>> = Default::default();
        for i in 0..1000 {
            if i % 2 == 0 {
                map.insert(i, i * 2);
            }
        }
        for i in 0..1000 {
            if i % 2 == 0 {
                find_assert(&map, &i, &(i * 2));
            } else {
                assert!(map.find(&i).is_none());
            }
        }
    }

    #[test]
    fn insert_bad_hash_lots() {
        let map: ConcHashMap<i32, i32, BuildHasherDefault<BadHasher>> = Default::default();
        for i in 0..100 {
            if i % 2 == 0 {
                map.insert(i, i * 2);
            }
        }
        for i in 0..100 {
            if i % 2 == 0 {
                find_assert(&map, &i, &(i * 2));
            } else {
                assert!(map.find(&i).is_none());
            }
        }
    }

    #[test]
    fn find_none_on_empty() {
        let map: ConcHashMap<i32, i32> = Default::default();
        assert!(map.find(&1).is_none());
    }

    #[test]
    fn test_clone() {
        let orig: ConcHashMap<i32, i32> = Default::default();
        for i in 0..100 {
            orig.insert(i, i * i);
        }
        let clone = orig.clone();
        for i in 0..100 {
            assert_eq!(orig.find(&i).unwrap().get(), clone.find(&i).unwrap().get());
        }
    }

    #[test]
    fn test_clear() {
        let map: ConcHashMap<i32, i32> = Default::default();
        for i in 0..100 {
            map.insert(i, i * i);
        }
        map.clear();
        for i in 0..100 {
            assert!(map.find(&i).is_none());
        }
    }

    #[test]
    fn test_remove() {
        let map: ConcHashMap<i32, String> = Default::default();
        map.insert(1, "one".to_string());
        map.insert(2, "two".to_string());
        map.insert(3, "three".to_string());
        assert_eq!(Some("two".to_string()), map.remove(&2));
        assert_eq!("one", map.find(&1).unwrap().get());
        assert!(map.find(&2).is_none());
        assert_eq!("three", map.find(&3).unwrap().get());
    }

    #[test]
    fn test_remove_many() {
        let map: ConcHashMap<i32, String> = Default::default();
        for i in 0..100 {
            map.insert(i, (i * i).to_string());
        }
        for i in 0..100 {
            if i % 2 == 0 {
                assert_eq!(Some((i * i).to_string()), map.remove(&i));
            }
        }
        for i in 0..100 {
            let x = map.find(&i);
            if i % 2 == 0 {
                assert!(x.is_none());
            } else {
                assert_eq!(&(i * i).to_string(), x.unwrap().get());
            }
        }
    }

    #[test]
    fn test_remove_insert() {
        let map: ConcHashMap<i32, String> = Default::default();
        for i in 0..100 {
            map.insert(i, (i * i).to_string());
        }
        for i in 0..100 {
            if i % 2 == 0 {
                assert_eq!(Some((i * i).to_string()), map.remove(&i));
            }
        }
        for i in 0..100 {
            if i % 4 == 0 {
                map.insert(i, i.to_string());
            }
        }
        for i in 0..100 {
            let x = map.find(&i);
            if i % 4 == 0 {
                assert_eq!(&i.to_string(), x.unwrap().get());
            } else if i % 2 == 0 {
                assert!(x.is_none());
            } else {
                assert_eq!(&(i * i).to_string(), x.unwrap().get());
            }
        }
    }

    #[test]
    fn test_from_iterator() {
        let vec: Vec<(u32, u32)> = (0..100).map(|i| (i, i * i)).collect();
        let map: ConcHashMap<u32, u32> = vec.iter().map(|x| *x).collect();
        for &(k, v) in vec.iter() {
            find_assert(&map, &k, &v);
        }
    }

    #[test]
    fn mut_modify() {
        let map: ConcHashMap<u32, u32> = Default::default();
        map.insert(1, 0);
        let mut e = map.find_mut(&1).unwrap().get();
        *e += 1;
        assert_eq!(&1, map.find(&1).unwrap().get());
    }

    #[test]
    fn conc_mut_modify() {
        let mmap: Arc<ConcHashMap<u32, u32>> = Arc::new(Default::default());
        let map = mmap.clone();
        let range = 10000;
        for i in 0..range {
            map.insert(i, i*i);
        }

        let tl_map = mmap.clone();
        let reader = thread::spawn(move || {
            for i in 0..range {
                tl_map.find(&i).unwrap().get();
            }
        });

        let tl_map = mmap.clone();
        let writer = thread::spawn(move || {
            for i in 0..range {
                let mut e = tl_map.find_mut(&i).unwrap().get();
                *e += 1;
            }
        });

        reader.join().unwrap();
        writer.join().unwrap();
        for i in 0..range {
            assert_eq!(map.find(&i).unwrap().get(), &(i*i+1));
        }
    }

    fn find_assert<K, V, H> (map: &ConcHashMap<K, V, H>, key: &K,  expected_val: &V)
            where K: Eq + Hash + Debug + Send + Sync, V: Eq + Debug + Send + Sync, H: BuildHasher {
        match map.find(key) {
            None    => panic!("missing key {:?} should map to {:?}", key, expected_val),
            Some(v) => assert_eq!(*v.get(), *expected_val)
        }
    }
}
