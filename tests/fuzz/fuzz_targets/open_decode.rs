#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../common.rs"]
mod common;

// Arbitrary bytes as a TIFF: the container and IFD parser, the page parser and every codec, under
// the FuzzTIFF limits. `run_file` checks the band invariants; a panic, an allocation outside the
// limits or a hang is the finding.
fuzz_target!(|data: &[u8]| {
    common::run_file(data, data.len() as u64, &common::TIGHT);
});
