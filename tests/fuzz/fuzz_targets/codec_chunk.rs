#![no_main]
use libfuzzer_sys::fuzz_target;
use std::sync::atomic::AtomicBool;

#[path = "../common.rs"]
mod common;
use common::{Entry, build, rows_of};

// data = [compression][width - 1][height - 1][samples][bits][flags][layout][rest]. A one-page TIFF
// is built around `rest` as the compressed bytes of every strip or tile, so each codec (none,
// LZW, Deflate, PackBits, CCITT, JPEG, CMYK JPEG) and the predictor see arbitrary input without
// the fuzzer first having to write a valid IFD. flags: 1 tiles, 2 predictor, 4 and 8 the CCITT
// options, 16 takes up to 31 bytes of `rest` as JPEGTables, 32 BigTIFF, and 64 and 128 (n = flags >> 6, 1 to
// 3) cap a band at 4 * n rows of the chunk, so strips and JPEG frames split into several bands.
fuzz_target!(|data: &[u8]| {
    let Some((&[sel, w, h, spp, bps, flags, layout], rest)) = data.split_first_chunk::<7>() else {
        return;
    };
    let comp = [1u64, 5, 8, 32946, 32773, 3, 4, 7, 7][usize::from(sel) % 9];
    let spp = [1u64, 3, 4][usize::from(spp) % 3];
    let bps = [8u64, 1, 4, 16, 8][usize::from(bps) % 5];
    let (w, h) = (u64::from(w) + 1, u64::from(h) + 1);
    let (tables, chunk) = if flags & 16 != 0 {
        rest.split_at((rest.first().map_or(0, |&n| usize::from(n) % 32)).min(rest.len()))
    } else {
        (&[][..], rest)
    };
    let big = flags & 32 != 0;
    let (tw, tl, rps) = (
        16 * (1 + u64::from(layout) % 4),
        16 * (1 + u64::from(layout >> 2) % 4),
        1 + u64::from(layout) % h,
    );
    let hdr = if big { 16 } else { 8 };
    let n = if flags & 1 != 0 {
        w.div_ceil(tw) * h.div_ceil(tl)
    } else {
        h.div_ceil(rps)
    };
    // sel / 9 picks the photometric: WhiteIsZero, BlackIsZero or palette (needs a ColorMap, 8 bits
    // at most) for 1 sample; RGB or YCbCr for 3; CMYK for 4.
    let photometric = match (spp, usize::from(sel / 9) % 3) {
        (1, 0) => 0,
        (1, 2) if bps <= 8 => 3,
        (1, _) => 1,
        (3, 2) => 6,
        (3, _) => 2,
        _ => 5,
    };
    let ent = |tag, typ, vals: Vec<u64>| Entry { tag, typ, vals };
    let ot = if big { 16 } else { 4 };
    let mut es = vec![
        ent(256, 4, vec![w]),
        ent(257, 4, vec![h]),
        ent(258, 3, vec![bps; spp as usize]),
        ent(259, 3, vec![comp]),
        ent(262, 3, vec![photometric]),
        ent(277, 3, vec![spp]),
    ];
    if flags & 1 != 0 {
        es.extend([
            ent(322, 4, vec![tw]),
            ent(323, 4, vec![tl]),
            ent(324, ot, vec![hdr; n as usize]),
            ent(325, ot, vec![chunk.len() as u64; n as usize]),
        ]);
    } else {
        es.extend([
            ent(273, ot, vec![hdr; n as usize]),
            ent(278, 4, vec![rps]),
            ent(279, ot, vec![chunk.len() as u64; n as usize]),
        ]);
    }
    if photometric == 3 {
        es.push(ent(320, 3, (0..3u64 << bps).collect()));
    }
    if flags & 2 != 0 {
        es.push(ent(317, 3, vec![2]));
    }
    if flags & 4 != 0 {
        es.push(ent(292, 4, vec![u64::from(layout & 7)]));
    }
    if flags & 8 != 0 {
        es.push(ent(293, 4, vec![u64::from(layout >> 5)]));
    }
    if flags & 16 != 0 {
        es.push(ent(347, 7, tables.iter().map(|&b| u64::from(b)).collect()));
    }
    let b = build(big, chunk, es);
    let mut lim = common::TIGHT;
    if flags >> 6 != 0 {
        let cw = if flags & 1 != 0 { tw } else { w };
        lim.max_band_bytes = 4 * u64::from(flags >> 6) * (cw * spp * bps).div_ceil(8);
    }
    let cancel = AtomicBool::new(false);
    let Ok(f) = tiff::File::open(&b[..], b.len() as u64, &lim) else {
        return;
    };
    if let Ok(p) = f.page(0) {
        rows_of(&p, &lim, &cancel, usize::MAX);
    }
});
