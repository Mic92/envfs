#![feature(test)]

extern crate test;
extern crate rand;
extern crate concurrent_hashmap;
use std::thread;
use std::sync::{Barrier, Arc};
use test::Bencher;
use rand::{Rng, SeedableRng, XorShiftRng};
use concurrent_hashmap::*;

const OPS: u32 = 10000;

#[bench]
fn concurrent_ops_50_reads_2_threads(b: &mut Bencher) {
    bench(b, 0.50, 2);
}

#[bench]
fn concurrent_ops_50_reads_4_threads(b: &mut Bencher) {
    bench(b, 0.50, 4);
}

#[bench]
fn concurrent_ops_50_reads_8_threads(b: &mut Bencher) {
    bench(b, 0.50, 8);
}

#[bench]
fn concurrent_ops_50_reads_16_threads(b: &mut Bencher) {
    bench(b, 0.50, 16);
}

#[bench]
fn concurrent_ops_50_reads_32_threads(b: &mut Bencher) {
    bench(b, 0.50, 32);
}

#[ignore]
#[bench]
fn concurrent_ops_50_reads_64_threads(b: &mut Bencher) {
    bench(b, 0.950, 64);
}

#[bench]
fn concurrent_ops_95_reads_2_threads(b: &mut Bencher) {
    bench(b, 0.95, 2);
}

#[bench]
fn concurrent_ops_95_reads_4_threads(b: &mut Bencher) {
    bench(b, 0.95, 4);
}

#[bench]
fn concurrent_ops_95_reads_8_threads(b: &mut Bencher) {
    bench(b, 0.95, 8);
}

#[bench]
fn concurrent_ops_95_reads_16_threads(b: &mut Bencher) {
    bench(b, 0.95, 16);
}

#[bench]
fn concurrent_ops_95_reads_32_threads(b: &mut Bencher) {
    bench(b, 0.95, 32);
}

#[ignore]
#[bench]
fn concurrent_ops_95_reads_64_threads(b: &mut Bencher) {
    bench(b, 0.95, 64);
}

#[bench]
fn concurrent_ops_100_reads_2_threads(b: &mut Bencher) {
    bench(b, 1.00, 2);
}

#[bench]
fn concurrent_ops_100_reads_4_threads(b: &mut Bencher) {
    bench(b, 1.00, 4);
}

#[bench]
fn concurrent_ops_100_reads_8_threads(b: &mut Bencher) {
    bench(b, 1.00, 8);
}

#[bench]
fn concurrent_ops_100_reads_16_threads(b: &mut Bencher) {
    bench(b, 1.00, 16);
}

#[bench]
fn concurrent_ops_100_reads_32_threads(b: &mut Bencher) {
    bench(b, 1.00, 32);
}

#[bench]
fn concurrent_ops_100_reads_64_threads(b: &mut Bencher) {
    bench(b, 1.00, 64);
}

fn bench(b: &mut Bencher, reads: f64, nthreads: u32) {
    b.iter(|| do_bench(reads, nthreads));
    b.bytes = nthreads as u64 * OPS as u64;
}

fn do_bench(reads: f64, nthreads: u32) {
    assert!(reads >= 0.0 && reads <= 1.0);
    let map: Arc<ConcHashMap<u32, u32>> = Arc::new(Default::default());
    let nthreads = nthreads as usize;
    {
        let mut threads = Vec::new();
        let start_barrier = Arc::new(Barrier::new(nthreads));
        for _ in 0..nthreads {
            let map = map.clone();
            let start_barrier = start_barrier.clone();
            threads.push(thread::spawn(move || {
                let mut rng: XorShiftRng = SeedableRng::from_seed([1, 2, 3, 4]);
                let mut read = 0;
                start_barrier.wait();
                for i in 0..OPS {
                    if rng.gen::<f64>() < reads {
                        map.find(&i).map(|x| read += *x.get());
                    } else {
                        map.insert(i, i * i);
                    }
                }
            }));
        }
        for thread in threads {
            thread.join().unwrap();
        }
    }
}
