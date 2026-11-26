#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../common.rs"]
mod common;
use common::{TIGHT, raw_entries, run_file};

const TAGS: [u16; 16] = [
    274, 700, 282, 283, 296, 338, 339, 347, 320, 292, 293, 324, 325, 266, 278, 259,
];

// data = [BigTIFF][extra entries] with each extra entry 1 tag selector + 1 type + 8 count + 8
// value bytes: a 1x1 grey TIFF plus up to four tags whose type, count and value field are
// arbitrary, as tiff-go's oobTIFF. Whatever the entries say, a page that parses lists them
// consistently: an out-of-range or wrongly typed entry carries no value or data.
fuzz_target!(|data: &[u8]| {
    let Some((&big, rest)) = data.split_first() else {
        return;
    };
    let extra: Vec<[u64; 4]> = rest
        .chunks_exact(18)
        .take(4)
        .map(|c| {
            let u = |b: &[u8]| u64::from_le_bytes(b.try_into().unwrap_or([0; 8]));
            [
                u64::from(TAGS[usize::from(c[0]) % 16]),
                u64::from(c[1] % 19),
                u(&c[2..10]),
                u(&c[10..18]),
            ]
        })
        .collect();
    let b = raw_entries(big & 1 != 0, &extra);
    let cancel = std::sync::atomic::AtomicBool::new(false);
    if let Ok(f) = tiff::File::open(&b[..], b.len() as u64, &TIGHT)
        && let Ok(p) = f.page(0)
    {
        for e in p.orientations.iter().chain(&p.xmps) {
            if e.out_of_range || e.bad_type {
                assert!(
                    e.data.is_none() && e.value == 0,
                    "a refused entry has a value"
                );
            }
            if e.over_limit {
                assert!(e.data.is_none());
            }
        }
        common::rows_of(&p, &TIGHT, &cancel, usize::MAX);
    }
    run_file(&b[..], b.len() as u64, &TIGHT);
});
