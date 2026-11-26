#![no_main]
use libfuzzer_sys::fuzz_target;
use std::sync::atomic::AtomicBool;
use tiff::Decoder;

#[path = "../common.rs"]
mod common;
use common::{Entry, MAX_PAGES, TIGHT, budget, build};

/// Chunks of one page that an input serves.
const MAX_CHUNKS: usize = 4;

/// Decodes chunk `i` of `p` and, as a one-strip TIFF of its own, the standalone JPEG `raw` of it;
/// when both succeed their pixels are equal.
fn same_pixels<R: tiff::ReadAt>(
    p: &tiff::Page<'_, R>,
    i: usize,
    raw: &[u8],
    dec: &mut Decoder,
    (want, got): (&mut Vec<u8>, &mut Vec<u8>),
) {
    let (info, lay) = (p.info(), p.layout());
    if info.compression != 7
        || info.bits_per_sample != 8
        || !matches!(info.samples_per_pixel, 1 | 3)
    {
        return;
    }
    let cancel = AtomicBool::new(false);
    if p.decode_chunk_into(i, dec, want, &cancel).is_err() {
        return;
    }
    let spp = info.samples_per_pixel as u64;
    let rows = want.len() as u64 / (u64::from(lay.chunk_width) * spp).max(1);
    let e = |tag, typ, vals: Vec<u64>| Entry { tag, typ, vals };
    let t = build(
        false,
        raw,
        vec![
            e(256, 4, vec![u64::from(lay.chunk_width)]),
            e(257, 4, vec![rows]),
            e(258, 3, vec![8; info.samples_per_pixel]),
            e(259, 3, vec![7]),
            e(262, 3, vec![if spp == 1 { 1 } else { 2 }]),
            e(273, 4, vec![8]),
            e(277, 3, vec![spp]),
            e(278, 4, vec![rows]),
            e(279, 4, vec![raw.len() as u64]),
        ],
    );
    let Ok(f) = tiff::File::open(t.as_slice(), t.len() as u64, &TIGHT) else {
        return;
    };
    let Ok(q) = f.page(0) else { return };
    if q.decode_chunk_into(0, dec, got, &cancel).is_ok() {
        assert!(got == want, "the stream decodes to other pixels");
    }
}

// Arbitrary bytes as a TIFF: the first chunks of the first pages through `Page::raw_jpeg_into`
// under the FuzzTIFF limits. A stream that comes back starts with SOI and fits `max_alloc`, `out`
// is empty after an error, and a second call on a vector with other contents gives the same
// bytes or the same error. When the first chunks also decode through `decode_chunk_into`, the
// stream put in a TIFF of its own and decoded again (jpeg-rs, with no tables) gives the same
// pixels, if that decode succeeds too.
fuzz_target!(|data: &[u8]| {
    let Ok(f) = tiff::File::open(data, data.len() as u64, &TIGHT) else {
        return;
    };
    budget(|| {
        let (mut out, mut again) = (Vec::new(), vec![0xa5u8; 7]);
        let (mut dec, mut want, mut got) = (Decoder::new(), Vec::new(), Vec::new());
        for pi in 0..f.num_pages().min(MAX_PAGES) {
            let Ok(p) = f.page(pi) else { continue };
            for i in (0..p.layout().chunks).take(MAX_CHUNKS) {
                let r = p.raw_jpeg_into(i, &mut out);
                let r2 = p.raw_jpeg_into(i, &mut again);
                assert!(r == r2, "the same chunk gives another result: {r:?} {r2:?}");
                if r.is_ok() {
                    assert!(out == again, "the same chunk gives other bytes");
                    assert!(out.starts_with(&[0xff, 0xd8]), "no SOI");
                    assert!(out.len() as u64 <= TIGHT.max_alloc, "over max_alloc");
                    if i < 2 {
                        same_pixels(&p, i, &out, &mut dec, (&mut want, &mut got));
                    }
                } else {
                    assert!(out.is_empty(), "out is not empty after an error");
                }
            }
        }
    });
});
