#![feature(test)]

extern crate rand;
extern crate test;
extern crate concurrent_hashmap;

use std::default::Default;
use std::cmp::max;
use test::Bencher;
use rand::{Rng, weak_rng, XorShiftRng};
use concurrent_hashmap::*;

const INTEGERS: u32 = 100_000;

macro_rules! new_map (
    ($typ: ty) => ({
        let mut options: Options<::std::collections::hash_map::RandomState> = Default::default();
        options.concurrency = 4;
        ConcHashMap::<$typ, usize, _>::with_options(options)
    })
);

#[bench]
#[inline(never)]
fn insert_sequential_integers(b: &mut Bencher) {
    b.iter(|| {
        let map = new_map!(u32);
        for i in 0..INTEGERS {
            map.insert(i, 0);
        }
        map
    });
    b.bytes = INTEGERS as u64;
}

#[bench]
#[inline(never)]
fn insert_random_integers(b: &mut Bencher) {
    let mut integers: Vec<_> = (0..INTEGERS).collect();
    weak_rng().shuffle(&mut integers);
    b.iter(|| {
        let map = new_map!(u32);
        for &i in integers.iter() {
            map.insert(i, 0);
        }
        map
    });
    b.bytes = INTEGERS as u64;
}

#[bench]
#[inline(never)]
fn insert_sequential_strings(b: &mut Bencher) {
    let strings: Vec<_> = (0..INTEGERS as u64).map(|i| (i * i).to_string()).collect();
    b.iter(|| {
        let map = new_map!(&str);
        for i in strings.iter() {
            map.insert(i, 0);
        }
        map
    });
    b.bytes = INTEGERS as u64;
}

#[bench]
#[inline(never)]
fn insert_random_strings(b: &mut Bencher) {
    let mut strings: Vec<_> = (0..INTEGERS as u64).map(|i| (i * i).to_string()).collect();
    weak_rng().shuffle(&mut strings);
    b.iter(|| {
        let map = new_map!(&str);
        for i in strings.iter() {
            map.insert(i, 0);
        }
        map
    });
    b.bytes = INTEGERS as u64;
}

#[bench]
#[inline(never)]
fn insert_sequential_integers_std(b: &mut Bencher) {
    b.iter(|| {
        let mut map = ::std::collections::HashMap::<u32, i8>::new();
        for i in 0..INTEGERS {
            map.insert(i, 0);
        }
        map
    });
    b.bytes = INTEGERS as u64;
}

#[bench]
#[inline(never)]
fn insert_random_integers_std(b: &mut Bencher) {
    let mut integers: Vec<_> = (0..INTEGERS).collect();
    weak_rng().shuffle(&mut integers);
    b.iter(|| {
        let mut map = ::std::collections::HashMap::<u32, i8>::new();
        for &i in integers.iter() {
            map.insert(i, 0);
        }
        map
    });
    b.bytes = INTEGERS as u64;
}

#[bench]
#[inline(never)]
fn insert_sequential_strings_std(b: &mut Bencher) {
    let strings: Vec<_> = (0..INTEGERS as u64).map(|i| (i * i).to_string()).collect();
    b.iter(|| {
        let mut map = ::std::collections::HashMap::<String, i8>::new();
        for i in strings.iter() {
            map.insert(i.clone(), 0);
        }
        map
    });
    b.bytes = INTEGERS as u64;
}

#[bench]
#[inline(never)]
fn insert_random_strings_std(b: &mut Bencher) {
    let mut strings: Vec<_> = (0..INTEGERS as u64).map(|i| (i * i).to_string()).collect();
    weak_rng().shuffle(&mut strings);
    b.iter(|| {
        let mut map = ::std::collections::HashMap::<String, i8>::new();
        for i in strings.iter() {
            map.insert(i.clone(), 0);
        }
        map
    });
    b.bytes = INTEGERS as u64;
}

#[ignore]
#[bench]
#[inline(never)]
fn random_integer_lookup_50_large(b: &mut Bencher) {
    let map = new_map!(u64);
    let len = 1000_000;
    for i in 0..len {
        map.insert(i, 0);
    }
    let mut nums: Vec<_> = (0..2 * len).collect();
    XorShiftRng::new_unseeded().shuffle(&mut nums);
    b.iter(|| {
        for _ in 0..1 {
            for i in nums.iter() {
                test::black_box(map.find(i));
            }
        }
    });
    b.bytes = nums.len() as u64;
}

// TODO Replace these with a macro when #12249 is solved
#[bench]
#[inline(never)]
fn random_integer_lookup_100(b: &mut Bencher) {
    random_integer_lookup(100.0, b, INTEGERS);
}

#[bench]
#[inline(never)]
fn random_integer_lookup_95(b: &mut Bencher) {
    random_integer_lookup(95.0, b, INTEGERS);
}

#[bench]
#[inline(never)]
fn random_integer_lookup_50(b: &mut Bencher) {
    random_integer_lookup(50.0, b, INTEGERS);
}

#[bench]
#[inline(never)]
fn random_integer_lookup_5(b: &mut Bencher) {
    random_integer_lookup(5.0, b, INTEGERS);
}

#[bench]
#[inline(never)]
fn random_integer_lookup_0(b: &mut Bencher) {
    random_integer_lookup(0.0, b, INTEGERS);
}

#[bench]
#[inline(never)]
fn random_integer_lookup_95_huge(b: &mut Bencher) {
    random_integer_lookup(95.0, b, INTEGERS * 100);
}

#[bench]
#[inline(never)]
fn random_string_lookup_95_huge(b: &mut Bencher) {
    random_string_lookup(95.0, b, INTEGERS * 100);
}

fn random_integer_lookup(hit_rate: f64, b: &mut Bencher, count: u32) {
    let mut rng = weak_rng();
    let map = new_map!(u32);
    for i in 0..count {
        map.insert(i, 0);
    }
    let base_n = 1000;
    let n = max(1, base_n - (0.99 * base_n as f64 * (1.0 - hit_rate / 100.0)) as u32);
    let (min, max) = if hit_rate > 0.0 {
        (0, (count as f64 / (hit_rate / 100.0)) as u32)
    } else {
        (count, 2 * count)
    };
    let keys: Vec<_> = (0..n).map(|_| rng.gen_range(min, max)).collect();
    b.iter(||
        for key in keys.iter() {
            test::black_box(map.find(key));
        }
    );
    b.bytes = n as u64 as u64;
}

fn random_string_lookup(hit_rate: f64, b: &mut Bencher, count: u32) {
    let mut rng = weak_rng();
    let map = new_map!(String);
    for i in 0..count {
        map.insert(format!("____{}____", i), 0);
    }
    let keys: Vec<_> = map.iter()
        .map(|(k, _)| if rng.gen::<f64>() < hit_rate { k.to_string() } else { "miss".to_string() })
        .collect();
    b.iter(||
        for key in keys.iter() {
            test::black_box(map.find(key));
        }
    );
    b.bytes = count as u64 as u64;
}

#[bench]
#[inline(never)]
fn random_integer_lookup_100_std(b: &mut Bencher) {
    random_integer_lookup_std(100.0, b);
}

#[bench]
#[inline(never)]
fn random_integer_lookup_95_std(b: &mut Bencher) {
    random_integer_lookup_std(95.0, b);
}

#[bench]
#[inline(never)]
fn random_integer_lookup_50_std(b: &mut Bencher) {
    random_integer_lookup_std(50.0, b);
}

#[bench]
#[inline(never)]
fn random_integer_lookup_5_std(b: &mut Bencher) {
    random_integer_lookup_std(5.0, b);
}

#[ignore]
#[bench]
#[inline(never)]
fn random_integer_lookup_0_std(b: &mut Bencher) {
    random_integer_lookup_std(0.0, b);
}

fn random_integer_lookup_std(hit_rate: f64, b: &mut Bencher) {
    let mut rng = weak_rng();
    let mut map = ::std::collections::HashMap::new();
    for i in 0..INTEGERS {
        map.insert(i, 0);
    }
    let base_n = 1000;
    let n = max(1, base_n - (0.99 * base_n as f64 * (1.0 - hit_rate / 100.0)) as u32);
    let (min, max) = if hit_rate > 0.0 {
        (0, (INTEGERS as f64 / (hit_rate / 100.0)) as u32)
    } else {
        (INTEGERS, 2 * INTEGERS)
    };
    let keys: Vec<_> = (0..n).map(|_| rng.gen_range(min, max)).collect();
    b.iter(||
        for key in keys.iter() {
            test::black_box(map.get(key));
        }
    );
    b.bytes = n as u64 as u64;
}
