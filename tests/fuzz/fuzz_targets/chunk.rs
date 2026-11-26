#![no_main]
use libfuzzer_sys::fuzz_target;
use std::sync::atomic::AtomicBool;
use tiff::Decoder;

#[path = "../common.rs"]
mod common;
use common::{MAX_PAGES, TIGHT, budget};

/// Chunks of one page that an input decodes.
const MAX_CHUNKS: usize = 2;

// Arbitrary bytes as a TIFF: the first chunks of the first pages through `Page::decode_chunk_into`,
// under the FuzzTIFF limits, all with one `Decoder`. A chunk that decodes has the size its layout
// says, `out` is empty after an error, and the same chunk on a new `Decoder` gives the same bytes
// or the same error, so what a `Decoder` keeps from one chunk to the next never changes a result.
fuzz_target!(|data: &[u8]| {
    let Ok(f) = tiff::File::open(data, data.len() as u64, &TIGHT) else {
        return;
    };
    budget(|| {
        let cancel = AtomicBool::new(false);
        let mut dec = Decoder::new();
        let (mut out, mut fresh_out) = (Vec::new(), Vec::new());
        for pi in 0..f.num_pages().min(MAX_PAGES) {
            let Ok(p) = f.page(pi) else { continue };
            let (info, lay) = (p.info(), p.layout());
            assert_eq!(lay.chunks, lay.across as usize * lay.down as usize);
            let crb = (lay.chunk_width as u64
                * info.samples_per_pixel as u64
                * info.bits_per_sample as u64)
                .div_ceil(8);
            for i in (0..lay.chunks).take(MAX_CHUNKS) {
                let r = p.decode_chunk_into(i, &mut dec, &mut out, &cancel);
                let r2 = p.decode_chunk_into(i, &mut Decoder::new(), &mut fresh_out, &cancel);
                assert!(
                    r == r2,
                    "a reused Decoder gives another result: {r:?} {r2:?}"
                );
                if r.is_ok() {
                    assert!(out == fresh_out, "a reused Decoder gives other bytes");
                    // Every row of a tile as stored; a strip has the rows that remain.
                    let rows = if lay.tiled {
                        u64::from(lay.chunk_height)
                    } else {
                        (info.height as u64 - i as u64 * u64::from(lay.chunk_height))
                            .min(u64::from(lay.chunk_height))
                    };
                    assert_eq!(out.len() as u64, crb * rows, "chunk {i} has the wrong size");
                } else {
                    assert!(out.is_empty(), "out is not empty after an error");
                }
            }
        }
    });
});
