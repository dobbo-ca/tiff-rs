//! `bench <dir> <n> [threads]`: prints name, output bytes and ns per decode (every page of the file) for
//! each `.tif` in `dir` that decodes, as `oracle -bench` does for Go. `threads` sets
//! `Limits::max_threads` (the default when omitted). See `tests/parity/bench.sh`.
//! `bench --rawtile <file> <n>` times `Page::raw_jpeg_into` against `Page::decode_chunk_into` on every
//! JPEG chunk of the file (best of `n` each, one thread); it is a report, not a gate.

use std::ops::ControlFlow;
use std::sync::atomic::AtomicBool;
use std::time::Instant;
use std::{env, fs, io};
use tiff::Limits;

fn decode(b: &[u8], lim: &Limits, cancel: &AtomicBool) -> Result<usize, tiff::Error> {
    let f = tiff::File::open(b, b.len() as u64, lim)?;
    let mut total = 0;
    for i in 0..f.num_pages() {
        f.page(i)?.rows(cancel, |_, rows| {
            total += rows.len();
            ControlFlow::Continue(())
        })?;
    }
    Ok(total)
}

type Chunks<'a> = (Vec<&'a [u8]>, Option<&'a [u8]>);

/// The strips and the JPEGTables (`None` when absent) of a classic TIFF (either byte order) with
/// LONG strip offsets.
fn jpeg_chunks(b: &[u8]) -> Option<Chunks<'_>> {
    let be = b.get(..2)? == b"MM";
    let u32_at = |o: usize| -> Option<usize> {
        let v: [u8; 4] = b.get(o..o + 4)?.try_into().ok()?;
        Some(if be {
            u32::from_be_bytes(v)
        } else {
            u32::from_le_bytes(v)
        } as usize)
    };
    let ifd = u32_at(4)?;
    let count = {
        let v: [u8; 2] = b.get(ifd..ifd + 2)?.try_into().ok()?;
        if be {
            u16::from_be_bytes(v)
        } else {
            u16::from_le_bytes(v)
        }
    };
    let (mut offs, mut cnts, mut n, mut tab) = (0, 0, 0, (0, 0));
    for i in 0..count as usize {
        let e = ifd + 2 + 12 * i;
        let v: [u8; 2] = b.get(e..e + 2)?.try_into().ok()?;
        match if be {
            u16::from_be_bytes(v)
        } else {
            u16::from_le_bytes(v)
        } {
            273 => (offs, n) = (u32_at(e + 8)?, u32_at(e + 4)?),
            279 => cnts = u32_at(e + 8)?,
            347 => tab = (u32_at(e + 8)?, u32_at(e + 4)?),
            _ => {}
        }
    }
    let chunks = (0..n)
        .map(|i| {
            let (o, c) = (u32_at(offs + 4 * i)?, u32_at(cnts + 4 * i)?);
            b.get(o..o + c)
        })
        .collect::<Option<Vec<_>>>()?;
    Some((
        chunks,
        (tab.0 != 0).then(|| b.get(tab.0..tab.0 + tab.1)).flatten(),
    ))
}

fn best(n: u32, mut f: impl FnMut()) -> f64 {
    (0..n)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed().as_nanos() as f64
        })
        .fold(f64::MAX, f64::min)
}

fn raw(path: &str, n: u32) -> io::Result<()> {
    let b = fs::read(path)?;
    let (chunks, tables) = jpeg_chunks(&b).expect("a classic TIFF with LONG strips");
    let o = jpeg::DecodeOptions {
        format: jpeg::Format::Gray,
        tables,
        ..Default::default()
    };
    let lim = jpeg::Limits {
        max_threads: 1,
        ..Default::default()
    };
    let cancel = AtomicBool::new(false);
    let mut dec = jpeg::Decoder::new();
    let tl = Limits {
        max_threads: 1,
        ..Limits::default()
    };
    let size = decode(&b, &tl, &cancel).expect("decodes") as f64;
    let tr = best(n, || {
        for c in &chunks {
            std::hint::black_box(dec.decode(c, &o, &lim, &cancel).expect("decodes"));
        }
    });
    let tf = best(n, || {
        std::hint::black_box(decode(&b, &tl, &cancel).is_ok());
    });
    println!(
        "raw rust: {:.3} ms/MB  full rust: {:.3} ms/MB  ({} strips, {size} bytes, best of {n})",
        tr / size,
        tf / size,
        chunks.len()
    );
    Ok(())
}

fn rawtile(path: &str, n: u32) -> io::Result<()> {
    let b = fs::read(path)?;
    let lim = Limits {
        max_threads: 1,
        ..Limits::default()
    };
    let f = tiff::File::open(b.as_slice(), b.len() as u64, &lim).expect("opens");
    let cancel = AtomicBool::new(false);
    let mut dec = tiff::Decoder::new();
    let (mut raw, mut out) = (Vec::new(), Vec::new());
    let (mut tiles, mut tr, mut td) = (0usize, 0.0f64, 0.0f64);
    for pi in 0..f.num_pages() {
        let Ok(p) = f.page(pi) else { continue };
        let info = p.info();
        if info.compression != 7 || !matches!(info.samples_per_pixel, 1 | 3) {
            continue;
        }
        for i in 0..p.layout().chunks {
            let r = best(n, || {
                std::hint::black_box(p.raw_jpeg_into(i, &mut raw).is_ok());
            });
            let d = best(n, || {
                std::hint::black_box(p.decode_chunk_into(i, &mut dec, &mut out, &cancel).is_ok());
            });
            tiles += 1;
            tr += r;
            td += d;
        }
    }
    let (x, y) = (
        tr / tiles.max(1) as f64 / 1e3,
        td / tiles.max(1) as f64 / 1e3,
    );
    println!(
        "rawtile: {path} tiles={tiles} raw us/tile={x:.2} decode us/tile={y:.2} ratio={:.1}",
        y / x
    );
    Ok(())
}

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    if let [_, flag, file, n] = args.as_slice()
        && flag == "--raw"
    {
        return raw(file, n.parse().unwrap_or(1).max(1));
    }
    if let [_, flag, file, n] = args.as_slice()
        && flag == "--rawtile"
    {
        return rawtile(file, n.parse().unwrap_or(1).max(1));
    }
    let (dir, n, threads) = match args.as_slice() {
        [_, dir, n] => (dir, n, None),
        [_, dir, n, t] => (dir, n, t.parse().ok()),
        _ => {
            eprintln!("usage: bench <dir> <n> [threads]");
            std::process::exit(2);
        }
    };
    let n: u32 = n.parse().unwrap_or(1).max(1);
    let mut lim = Limits::default();
    if let Some(t) = threads {
        lim.max_threads = t;
    }
    let cancel = AtomicBool::new(false);
    let mut names: Vec<String> = fs::read_dir(dir)?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".tif"))
        .collect();
    names.sort();
    for name in names {
        let b = fs::read(format!("{dir}/{name}"))?;
        let Ok(size) = decode(&b, &lim, &cancel) else {
            continue;
        };
        let t = Instant::now();
        for _ in 0..n {
            std::hint::black_box(decode(&b, &lim, &cancel).is_ok());
        }
        println!("{name}\t{size}\t{}", t.elapsed().as_nanos() / u128::from(n));
    }
    Ok(())
}
