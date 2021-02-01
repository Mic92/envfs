#![feature(step_by)]
extern crate concurrent_hashmap;

use std::io::Read;
use std::io;
use std::cmp;
use std::thread;
use std::default::Default;
use std::sync::Arc;
use concurrent_hashmap::*;

fn main() {
    let words = Arc::new(read_words());
    let word_counts: Arc<ConcHashMap<String, u32>> = Default::default();
    count_words(words.clone(), word_counts.clone(), 4);
    let mut counts: Vec<(String, u32)> = word_counts.iter().map(|(s, &n)| (s.clone(), n)).collect();
    counts.sort_by(|&(_, a), &(_, b)| a.cmp(&b));
    for &(ref word, count) in counts.iter() {
        println!("{}\t{}", word, count);
    }
}

fn read_words() -> Vec<String> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).unwrap();
    input.split_whitespace()
        .map(|w| w.trim_matches(|c| ['.', '"', ':', ';', ',', '!', '?', ')', '(', '_']
                  .contains(&c)))
        .map(|w| w.to_lowercase())
        .filter(|w| !w.is_empty())
        .collect()
}

fn count_words(words: Arc<Vec<String>>, word_counts: Arc<ConcHashMap<String, u32>>, nthreads: usize) {
    let mut threads = Vec::with_capacity(nthreads);
    let chunk_size = words.len() / nthreads;
    for chunk_index in (0..words.len()).step_by(chunk_size) {
        let words = words.clone();
        let word_counts = word_counts.clone();
        threads.push(thread::spawn(move || {
            for word in &words[chunk_index..cmp::min(words.len(), chunk_index + chunk_size)] {
                // It would be nice to be able to pass a &K to .upsert()
                // and have it clone as needed instead of passing a K.
                word_counts.upsert(word.to_owned(), 1, &|count| *count += 1);
            }
        }));
    }
    for thread in threads {
        thread.join().unwrap();
    }
}
