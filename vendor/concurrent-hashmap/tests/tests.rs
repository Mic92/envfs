extern crate rand;
extern crate concurrent_hashmap;

use std::collections::HashMap;
use std::thread;
use std::default::Default;
use std::sync::Arc;
use rand::{Rng, weak_rng};
use concurrent_hashmap::*;

/// Spawn a lot of threads that update the map conccurently at different ranges.
/// Checks that random numbers in the total range are either empty or have correct values.
#[test]
fn many_threads() {
    let mut threads = Vec::new();
    let map: Arc<ConcHashMap<i32, i32>> = Arc::new(Default::default());
    let n = 1500;
    let nthreads = 30;
    let max = nthreads * n;
    for t in 0..nthreads {
        let map = map.clone();
        threads.push(thread::spawn(move || {
            let mut rng = weak_rng();
            let s = t * n;
            for i in s..s + n {
                map.insert(i, t);
                let x = rng.gen_range(0, max);
                match map.find(&x) {
                    Some(ref y) if x / n != *y.get() => return Err(format!("{} => {}", x, *y.get())),
                    _ => { }
                }
            }
            Ok(())
        }));
    }
    for thread in threads {
        assert_eq!(thread.join().unwrap(), Ok(()));
    }
}

/// Count elements in a list both sequentially and parallel, then verify that the results are the same.
#[test]
fn count_compare_with_sequential() {
    let n = 10000;
    let max = 100;
    let mut rng = weak_rng();
    let nums: Vec<_> = (0..n).map(|_| rng.gen_range(0, max)).collect();

    let seq = count_seq(&nums);
    let par = count_par(&nums);

    for k in 0..max {
        let seq_v = seq.get(&k);
        let par_v = par.find(&k);
        if seq_v.is_none() && par_v.is_none() {
            continue;
        }
        assert_eq!(seq_v.unwrap(), par_v.unwrap().get());
    }

    fn count_seq(nums: &[u32]) -> HashMap<u32, u32> {
        let mut map = HashMap::new();
        for &num in nums {
            *map.entry(num).or_insert(0) += 1;
        }
        return map;
    }

    fn count_par(nums: &[u32]) -> Arc<ConcHashMap<u32, u32>> {
        let map: Arc<ConcHashMap<u32, u32>> = Default::default();
        let mut threads = Vec::new();
        for ns in nums.chunks(nums.len() / 4) {
            let map = map.clone();
            let ns = ns.iter().cloned().collect::<Vec<_>>();
            threads.push(thread::spawn(move || {
                for &num in ns.iter() {
                    map.upsert(num, 1, &|count| *count += 1);
                }
            }));
        }
        for thread in threads {
            thread.join().unwrap();
        }
        map
    }
}