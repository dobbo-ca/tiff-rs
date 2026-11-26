//! The Rust side of the real-file comparison: `quality [-max N] [-log FILE] <out-dir> <slide>...`
//! does what `tests/parity/go/oracle -qtiles` does with this crate. Every page of every slide is cut
//! into a fixed grid of 256 x 256 cells (edge cells cropped), `min(cells, N)` of them are sampled
//! evenly (cell `floor(j * cells / m)` for `j < m`), and each is written as
//! `<slide file name with . as _>.ifd<k>.<cell:06>.ppm` (3 samples) or `.pgm` (1 sample), 8 bit.
//! A page with another sample layout, or that fails, gets one line `<slide> ifd<k> <class>` in the
//! log (stderr without `-log`); a page that fails half way keeps the cells already written. The
//! page decode stops after the last sampled cell's row band. See `docs/parity.md`.

use std::fs;
use std::io::{self, BufWriter, Write};
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use tiff::{Error, Limits};

/// The side of the grid cells.
const CELL: usize = 256;
/// The most bytes of the one cell row that is kept in memory.
const MAX_RING: u64 = 1 << 30;

/// Fits a 51200 x 38144 page; the other caps are the defaults.
fn limits() -> Limits {
    Limits {
        max_pixels: 1 << 32,
        max_alloc: 2 << 30,
        ..Limits::default()
    }
}

/// The class tokens of `examples/oracle.rs`.
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

/// Writes the cells of cell row `r` that are in `cols` from `ring`, which holds that row's rows.
#[allow(clippy::too_many_arguments)]
fn flush(
    base: &str,
    ext: &str,
    magic: &str,
    ring: &[u8],
    (w, h, spp): (usize, usize, usize),
    r: usize,
    cols: &[usize],
) -> io::Result<()> {
    let rb = w * spp;
    let cx = w.div_ceil(CELL);
    let ch = (h - r * CELL).min(CELL);
    for &c in cols {
        let x0 = c * CELL;
        let cw = (w - x0).min(CELL);
        let mut f = BufWriter::new(fs::File::create(format!("{base}.{:06}{ext}", r * cx + c))?);
        write!(f, "{magic}\n{cw} {ch}\n255\n")?;
        for i in 0..ch {
            f.write_all(&ring[i * rb + x0 * spp..i * rb + (x0 + cw) * spp])?;
        }
        f.flush()?;
    }
    Ok(())
}

/// Decodes page `p` and writes its sampled cells to `<base>.<cell:06>.ppm` or `.pgm`. Returns the
/// class of the reason when the page is skipped or its decode fails, and an error only for a failed
/// write.
fn page<R: tiff::ReadAt>(
    base: &str,
    max: usize,
    p: &tiff::Page<'_, R>,
) -> io::Result<Option<String>> {
    let n = p.info();
    let (w, h, spp) = (n.width, n.height, n.samples_per_pixel);
    let rb = w.saturating_mul(spp);
    if n.bits_per_sample != 8
        || (spp != 1 && spp != 3)
        || !n.extra_samples.is_empty()
        || n.photometric == tiff::PALETTE
        || n.row_bytes != rb as u64
    {
        return Ok(Some("layout".into()));
    }
    let (cx, cy) = (w.div_ceil(CELL), h.div_ceil(CELL));
    let cells = cx * cy;
    let m = cells.min(max);
    if m == 0 {
        return Ok(None);
    }
    // sel[r] lists the grid columns sampled in cell row r: cell floor(j * cells / m) for j < m.
    let mut sel: Vec<Vec<usize>> = vec![Vec::new(); cy];
    for j in 0..m {
        let c = j * cells / m;
        sel[c / cx].push(c % cx);
    }
    let last = (m - 1) * cells / m / cx;
    let ring_rows = h.min(CELL);
    if (rb as u64).saturating_mul(ring_rows as u64) > MAX_RING {
        return Ok(Some("limit:alloc".into()));
    }
    let mut ring = vec![0u8; rb * ring_rows];
    let (magic, ext) = if spp == 1 {
        ("P5", ".pgm")
    } else {
        ("P6", ".ppm")
    };
    let mut werr = None;
    let mut done = false;
    let cancel = AtomicBool::new(false);
    let res = p.rows(&cancel, |y, band| {
        for (i, src) in band.chunks_exact(rb).enumerate() {
            let row = y + i;
            if row >= h {
                break;
            }
            let r = row / CELL;
            if sel[r].is_empty() {
                continue;
            }
            let at = (row % CELL) * rb;
            ring[at..at + rb].copy_from_slice(src);
            if row == ((r + 1) * CELL).min(h) - 1 {
                if let Err(e) = flush(base, ext, magic, &ring, (w, h, spp), r, &sel[r]) {
                    werr = Some(e);
                    return ControlFlow::Break(());
                }
                if r == last {
                    done = true;
                    return ControlFlow::Break(());
                }
            }
        }
        ControlFlow::Continue(())
    });
    if let Some(e) = werr {
        return Err(e);
    }
    Ok(match res {
        Err(e) if !done => Some(class(&e)),
        _ => None,
    })
}

fn slide(out: &str, path: &str, max: usize, log: &mut dyn Write) -> io::Result<()> {
    let file = fs::File::open(path)?;
    let size = file.metadata()?.len();
    let f = match tiff::File::open(file, size, &limits()) {
        Ok(f) => f,
        Err(e) => return writeln!(log, "{path} open {}", class(&e)),
    };
    let name = Path::new(path)
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let stem = name.replace(['.', '/'], "_");
    for k in 0..f.num_pages() {
        let p = match f.page(k) {
            Ok(p) => p,
            Err(e) => {
                writeln!(log, "{path} ifd{k} {}", class(&e))?;
                continue;
            }
        };
        if let Some(reason) = page(&format!("{out}/{stem}.ifd{k}"), max, &p)? {
            writeln!(log, "{path} ifd{k} {reason}")?;
        }
    }
    Ok(())
}

fn usage() -> ! {
    eprintln!("usage: quality [-max N] [-log FILE] <out-dir> <slide>...");
    std::process::exit(2);
}

fn main() -> io::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let (mut max, mut log) = (150usize, None);
    while args.first().is_some_and(|a| a == "-max" || a == "-log") {
        let Some(v) = args.get(1) else { usage() };
        if args[0] == "-max" {
            max = v.parse().unwrap_or_else(|_| usage());
        } else {
            log = Some(v.clone());
        }
        args.drain(..2);
    }
    if args.len() < 2 {
        usage();
    }
    fs::create_dir_all(&args[0])?;
    let mut log: Box<dyn Write> = match log {
        Some(p) => Box::new(BufWriter::new(fs::File::create(p)?)),
        None => Box::new(io::stderr()),
    };
    for s in &args[1..] {
        slide(&args[0], s, max, &mut *log)?;
    }
    log.flush()
}
