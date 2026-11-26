#![no_main]
use libfuzzer_sys::fuzz_target;
use tiff::Limits;

#[path = "../common.rs"]
mod common;
use common::{PageRes, Res, TIGHT, run_file};

// data = [file][5 limit bytes]; the file is all of `data`, so a plain TIFF is a seed. Each of the
// last five bytes shifts one of TIGHT's fields right by `byte % 24` (0 keeps it): max_ifds,
// max_pixels, max_band_bytes, max_metadata_bytes, max_alloc.
// Limits only refuse: a file that decodes under random limits decodes, to the same bytes, under
// looser ones, and never delivers a band over its cap. (CMYK JPEG is left out of the
// comparison: tiff-go keeps a restart-interval quirk that depends on the band split.)
fuzz_target!(|data: &[u8]| {
    let Some(&[ifds, px, band, meta, alloc]) = data.last_chunk::<5>() else {
        return;
    };
    let file = data;
    let s = |v: u8| u32::from(v) % 24;
    let lim = Limits {
        max_ifds: TIGHT.max_ifds >> s(ifds),
        max_pixels: TIGHT.max_pixels >> s(px),
        max_band_bytes: TIGHT.max_band_bytes >> s(band),
        max_metadata_bytes: TIGHT.max_metadata_bytes >> s(meta),
        max_alloc: TIGHT.max_alloc >> s(alloc),
        max_threads: TIGHT.max_threads,
    };
    let small = run_file(file, file.len() as u64, &lim);
    let Res::Pages(n, pages) = small else { return };
    let Res::Pages(m, loose) = run_file(file, file.len() as u64, &TIGHT) else {
        return;
    };
    // TIGHT caps the IFDs at 64: compare only what both read
    if n != m {
        return;
    }
    for (a, b) in pages.iter().zip(&loose) {
        if let (PageRes::Ok(ia, oa), PageRes::Ok(ib, ob)) = (a, b)
            && oa.err.is_none()
            && ob.err.is_none()
            && ia.samples_per_pixel != 4
        {
            assert!(
                ia == ib && oa.digest() == ob.digest(),
                "limits changed the output"
            );
        }
    }
});
