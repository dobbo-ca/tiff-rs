//! The Rust side of the parity check: `oracle [-threads N] <inputs> <outputs>` decodes every input like
//! `tests/parity/go/oracle` and writes, per limits configuration `<cfg>`, `<name>.<cfg>.info` (the page
//! count and every page's `Info`, or its error class) and, per page and operation,
//! `<name>.<cfg>.p<i>.<op>.out` (the bytes of all bands), or `.err` (the error class) and
//! `.partial.out` (the bytes before the error), and `.bands` (one `y len` line per band). `-threads N` sets
//! `Limits::max_threads`; the output must not depend on it. The class table is in `docs/parity.md`.

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{env, fs};
use tiff::{Error, Limits, TagEntry};

/// What jpeg-go's process limit is in the Go oracle.
const MEM_LIMIT: u64 = 256 << 20;
/// How many pages of one file an input decodes.
const MAX_PAGES: usize = 64;

struct Config {
    name: &'static str,
    lim: Limits,
    ops: &'static [&'static str],
}

/// The same three configurations as the Go oracle: `d`, `s` (a small band and metadata budget) and
/// `g` (each side's defaults).
fn configs(max_threads: usize) -> [Config; 3] {
    let d = Limits {
        max_ifds: 10000,
        max_pixels: 1 << 22,
        max_band_bytes: 1 << 20,
        max_metadata_bytes: 4 << 20,
        max_alloc: MEM_LIMIT,
        max_threads,
    };
    [
        Config {
            name: "d",
            lim: d,
            ops: &["rows", "c1", "k1"],
        },
        Config {
            name: "s",
            lim: Limits {
                max_band_bytes: 2000,
                max_metadata_bytes: 8,
                ..d
            },
            ops: &["rows"],
        },
        Config {
            name: "g",
            lim: Limits {
                max_threads,
                ..Limits::default()
            },
            ops: &["rows"],
        },
    ]
}

/// The names tiff-go itself uses, and the first word of the others.
fn limit_name(n: &str) -> String {
    match n {
        "ifds" | "pixels" | "jpeg tables" | "ifd entries" | "band bytes" => n.replace(' ', "-"),
        _ => n.split(' ').next().unwrap_or(n).to_string(),
    }
}

fn class(e: &Error) -> String {
    match e {
        Error::Limit { name, .. } => format!("limit:{}", limit_name(name)),
        Error::Corrupt(_) => "corrupt".into(),
        Error::Unsupported(_) => "unsupported".into(),
        Error::Cancelled => "cancelled".into(),
        Error::Stopped => "stopped".into(),
        e => format!("other:{e}"),
    }
}

fn fnv64(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf29ce484222325, |h, &c| {
        (h ^ u64::from(c)).wrapping_mul(0x100000001b3)
    })
}

/// Writes the bands of one operation to `<base>.partial.out` and renames it to `<base>.out` when
/// the operation succeeds.
fn run<R: tiff::ReadAt>(base: &str, op: &str, p: &tiff::Page<'_, R>) -> io::Result<()> {
    let base = format!("{base}.{op}");
    let part = format!("{base}.partial.out");
    let mut w = io::BufWriter::with_capacity(1 << 16, fs::File::create(&part)?);
    let cancel = AtomicBool::new(false);
    let (mut n, mut bands, mut werr) = (0usize, 0, None);
    let mut layout = String::new();
    let res = p.rows(&cancel, |y, band| {
        writeln!(layout, "{y} {}", band.len()).ok();
        if let Err(e) = w.write_all(band) {
            werr = Some(e);
            return ControlFlow::Break(());
        }
        n += band.len();
        bands += 1;
        if bands == 1 {
            match op {
                "c1" => cancel.store(true, Ordering::Relaxed),
                "k1" => return ControlFlow::Break(()),
                _ => {}
            }
        }
        ControlFlow::Continue(())
    });
    if let Some(e) = werr {
        return Err(e);
    }
    w.flush()?;
    drop(w);
    fs::write(format!("{base}.bands"), layout)?;
    match res {
        Ok(()) => fs::rename(part, format!("{base}.out")),
        Err(e) => {
            if n == 0 {
                fs::remove_file(part)?;
            }
            fs::write(format!("{base}.err"), format!("{}\n", class(&e)))
        }
    }
}

fn entries(sb: &mut String, tag: &str, es: &[TagEntry]) {
    for (i, e) in es.iter().enumerate() {
        let data = match &e.data {
            None => "nil".to_string(),
            Some(d) => format!("{}:{:016x}", d.len(), fnv64(d)),
        };
        writeln!(
            sb,
            "{tag}{i} typ={} count={} val={} oor={} bad={} over={} dup={} data={data}",
            e.typ, e.count, e.value, e.out_of_range, e.bad_type, e.over_limit, e.duplicate
        )
        .ok();
    }
}

fn u16s(v: &[u16]) -> String {
    v.iter().map(|x| format!("{x},")).collect()
}

fn info<R: tiff::ReadAt>(sb: &mut String, i: usize, p: &tiff::Page<'_, R>) {
    let n = p.info();
    let cm: Vec<u8> = n.color_map.iter().flat_map(|c| c.to_le_bytes()).collect();
    writeln!(
        sb,
        "p{i} w={} h={} spp={} bps={} photo={} comp={} orient={} rowbytes={} resunit={} xres={:016x} yres={:016x}",
        n.width,
        n.height,
        n.samples_per_pixel,
        n.bits_per_sample,
        n.photometric,
        n.compression,
        n.orientation as i64,
        n.row_bytes,
        n.resolution_unit,
        n.x_resolution.to_bits(),
        n.y_resolution.to_bits()
    )
    .ok();
    writeln!(
        sb,
        "p{i} cmap={}:{:016x} extra={} skipped={}",
        n.color_map.len(),
        fnv64(&cm),
        u16s(&n.extra_samples),
        u16s(&n.skipped)
    )
    .ok();
    entries(sb, &format!("p{i} or"), &p.orientations);
    entries(sb, &format!("p{i} xmp"), &p.xmps);
}

fn decode_one(base: &str, b: &[u8], c: &Config) -> io::Result<()> {
    let mut sb = String::new();
    let f = match tiff::File::open(b, b.len() as u64, &c.lim) {
        Ok(f) => f,
        Err(e) => {
            writeln!(sb, "open err={}", class(&e)).ok();
            return fs::write(format!("{base}.info"), sb);
        }
    };
    writeln!(sb, "pages={}", f.num_pages()).ok();
    let mut pages = Vec::new();
    for i in 0..f.num_pages().min(MAX_PAGES) {
        match f.page(i) {
            Ok(p) => {
                info(&mut sb, i, &p);
                pages.push((i, p));
            }
            Err(e) => {
                writeln!(sb, "p{i} err={}", class(&e)).ok();
            }
        }
    }
    fs::write(format!("{base}.info"), sb)?;
    for (i, p) in &pages {
        for op in c.ops {
            run(&format!("{base}.p{i}"), op, p)?;
        }
    }
    Ok(())
}

fn usage() -> ! {
    eprintln!("usage: oracle [-threads N] <inputs> <outputs>");
    std::process::exit(2);
}

fn main() -> io::Result<()> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    let mut threads = Limits::default().max_threads;
    if args.first().is_some_and(|a| a == "-threads") {
        threads = args
            .get(1)
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| usage());
        args.drain(..2);
    }
    let [input, output] = args.as_slice() else {
        usage();
    };
    fs::create_dir_all(output)?;
    let mut names = Vec::new();
    for e in fs::read_dir(input)? {
        let e = e?;
        if e.file_type()?.is_file() {
            names.push(e.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    let configs = configs(threads);
    for n in &names {
        let b = fs::read(format!("{input}/{n}"))?;
        for c in &configs {
            decode_one(&format!("{output}/{n}.{}", c.name), &b, c)?;
        }
    }
    Ok(())
}
