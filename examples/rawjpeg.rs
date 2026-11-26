//! Real-file raw JPEG check: `rawjpeg [-max N] <out-a> <out-b> <report.md> <slide>...`.
//!
//! For every JPEG page (compression 7, 1 or 3 samples) of every slide and EVERY chunk, the stream
//! of `Page::raw_jpeg_into` is decoded by one reused `jpeg::Decoder` into the layout of the tiff
//! rows, and the result is compared byte for byte with `Page::decode_chunk_into` (one reused
//! `tiff::Decoder`). Both run with one thread. Up to N (default 150) chunks of each page, chunk
//! `floor(j * chunks / m)` for `j < m`, are written as PGM or PPM, the tiff decode to `<out-a>`
//! and the jpeg decode of the raw stream to `<out-b>`, under the same name,
//! so the two trees can be compared.
//! The report has a table per slide, with the time per tile of the raw call alone, of the raw
//! call plus the jpeg decode, and of the tiff decode.
//!
//! A chunk whose decoded size is over `BAND` (an NDPI level that is one strip) is not decoded
//! whole: the tiff side comes from `Page::rows` and the jpeg side from jpeg-rs region decodes of
//! the same bands of rows. Such a chunk is counted as compared, but is not timed and is not
//! written out. A chunk that cannot be compared is listed in the report with its reason: a
//! limit in the banded path, a page with several chunks of that size, or a chunk that fails to
//! decode on both sides with the same class of error. Any other error, and any difference, is a
//! failure: the exit status is 1.

use std::fs;
use std::io::{self, BufWriter, Write};
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};
use tiff::{Decoder, Error, Limits};

/// Decoded bytes of a chunk above which it is compared in bands. `max_band_bytes` of
/// `decode_chunk_into` is this, and a JPEG chunk is budgeted four times it, which must fit in
/// `max_alloc` (2 GiB) with the output.
const BAND: u64 = 256 << 20;

/// Limits of the whole-chunk decode. `max_pixels` caps the page (Philips-4 level 0 is over 2^32
/// pixels); each decode holds one chunk, which `max_band_bytes` caps.
fn limits() -> Limits {
    Limits {
        max_pixels: 1 << 40,
        max_alloc: 2 << 30,
        max_band_bytes: BAND,
        max_threads: 1,
        ..Limits::default()
    }
}

/// Limits of the banded tiff decode (`Page::rows`): the default 64 MiB band.
fn rows_limits() -> Limits {
    Limits {
        max_pixels: 1 << 32,
        max_alloc: 2 << 30,
        max_threads: 1,
        ..Limits::default()
    }
}

fn jlimits() -> jpeg::Limits {
    jpeg::Limits {
        max_pixels: 1 << 32,
        max_alloc: 2 << 30,
        max_threads: 1,
    }
}

/// A coarse class of a tiff error, to compare with the class of a jpeg-rs error.
fn class(e: &Error) -> String {
    match e {
        Error::Limit { .. } => "limit".into(),
        Error::Corrupt(_) => "corrupt".into(),
        Error::Unsupported(_) => "unsupported".into(),
        e => format!("other:{e}"),
    }
}

/// The class of a jpeg-rs error as tiff-rs maps it: a refused limit is a limit, the rest is
/// corruption.
fn jclass(e: &jpeg::Error) -> String {
    match e {
        jpeg::Error::PixelLimit { .. } | jpeg::Error::AllocLimit { .. } => "limit".into(),
        jpeg::Error::OutOfMemory | jpeg::Error::Cancelled => format!("other:{e:?}"),
        _ => "corrupt".into(),
    }
}

/// Decodes `stream` with `dec` (the whole frame, or `region` of it) and packs the rows of the
/// result as tiff-rs delivers them: grey, or RGB with the alpha dropped.
fn jpeg_rows(
    dec: &mut jpeg::Decoder,
    stream: &[u8],
    spp: usize,
    region: Option<jpeg::Rect>,
    cancel: &AtomicBool,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    let o = jpeg::DecodeOptions {
        format: if spp == 1 {
            jpeg::Format::Gray
        } else {
            jpeg::Format::Rgba
        },
        region,
        ..Default::default()
    };
    let d = dec
        .decode(stream, &o, &jlimits(), cancel)
        .map_err(|e| jclass(&e))?;
    if let Some(e) = &d.error {
        return Err(jclass(e));
    }
    out.clear();
    let bad = || "corrupt".to_string();
    match &d.image {
        jpeg::Image::Gray(g) => {
            let w = g.rect.width() as usize;
            for y in g.rect.min_y..g.rect.max_y {
                let o = g.pix_offset(g.rect.min_x, y);
                out.extend_from_slice(g.pix.get(o..o + w).ok_or_else(bad)?);
            }
        }
        jpeg::Image::Rgba(m) => {
            let w = m.rect.width() as usize;
            for y in m.rect.min_y..m.rect.max_y {
                let o = m.pix_offset(m.rect.min_x, y);
                let row = m.pix.get(o..o + 4 * w).ok_or_else(bad)?;
                for px in row.as_chunks::<4>().0 {
                    out.extend_from_slice(&px[..3]);
                }
            }
        }
        _ => return Err(bad()),
    }
    Ok(())
}

/// How the banded comparison of one chunk ended.
enum Band {
    Same,
    Differs,
    /// Not compared, with the reason.
    Skip(String),
    Fail(String),
}

/// Compares the one chunk of page `k` of `path`, whose raw stream is `raw`, band by band: the
/// tiff rows of `Page::rows` against a jpeg-rs region decode of the same rows. Returns the time
/// of the jpeg decodes and of the whole tiff pass.
fn banded(
    path: &str,
    k: usize,
    spp: usize,
    raw: &[u8],
    jdec: &mut jpeg::Decoder,
    cancel: &AtomicBool,
) -> (Band, Duration, Duration) {
    let zero = Duration::ZERO;
    let open = || -> Result<(fs::File, u64), io::Error> {
        let f = fs::File::open(path)?;
        let n = f.metadata()?.len();
        Ok((f, n))
    };
    let Ok((file, size)) = open() else {
        return (Band::Fail("other:reopen".into()), zero, zero);
    };
    let f = match tiff::File::open(file, size, &rows_limits()) {
        Ok(f) => f,
        Err(e) => return (Band::Fail(class(&e)), zero, zero),
    };
    let p = match f.page(k) {
        Ok(p) => p,
        Err(e) => return (Band::Fail(class(&e)), zero, zero),
    };
    if p.layout().chunks != 1 {
        return (
            Band::Skip("a chunk over the band size in a page of several chunks".into()),
            zero,
            zero,
        );
    }
    let w = p.info().width;
    let mut tmp = Vec::new();
    let (mut differs, mut fail) = (false, None::<String>);
    let mut tj = zero;
    let t0 = Instant::now();
    let res = p.rows(cancel, |y, band| {
        let n = band.len() / (w * spp);
        let region = jpeg::Rect::new(0, y as i32, w as i32, (y + n) as i32);
        let t = Instant::now();
        let r = jpeg_rows(jdec, raw, spp, Some(region), cancel, &mut tmp);
        tj += t.elapsed();
        match r {
            Err(e) => fail = Some(e),
            Ok(()) => differs = tmp.as_slice() != band,
        }
        if differs || fail.is_some() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    });
    let tt = t0.elapsed().saturating_sub(tj);
    let band = if let Some(e) = fail {
        // A limit of jpeg-rs on a frame this size is a skip; any other error is a failure.
        if e == "limit" {
            Band::Skip("jpeg-rs limit in a region decode".into())
        } else {
            Band::Fail(format!("raw side {e}"))
        }
    } else if differs {
        Band::Differs
    } else {
        match res {
            Ok(()) => Band::Same,
            Err(Error::Limit { .. }) => Band::Skip("tiff-rs limit in Page::rows".into()),
            Err(e) => Band::Fail(format!("tiff side {}", class(&e))),
        }
    };
    (band, tj, tt)
}

/// What one page (IFD) of a slide counted.
#[derive(Default, Clone)]
struct Stats {
    chunks: usize,
    compared: usize,
    different: usize,
    /// Not compared, with a reason in the report.
    skipped: usize,
    /// Of `compared`, those compared in bands.
    banded: usize,
    /// Per timed chunk: raw, raw and jpeg decode, tiff decode, in ns.
    ns: Vec<[u64; 3]>,
}

impl Stats {
    fn add(&mut self, o: &Stats) {
        self.chunks += o.chunks;
        self.compared += o.compared;
        self.different += o.different;
        self.skipped += o.skipped;
        self.banded += o.banded;
        self.ns.extend_from_slice(&o.ns);
    }

    /// One table row: median and mean microseconds of each time, and the mean tiff decode over
    /// the mean raw call.
    fn row(&self, label: &str) -> String {
        let mut cols = String::new();
        let mut means = [0.0f64; 3];
        for (c, mean) in means.iter_mut().enumerate() {
            let mut v: Vec<u64> = self.ns.iter().map(|t| t[c]).collect();
            v.sort_unstable();
            let n = v.len();
            let med = match n {
                0 => 0.0,
                _ if n % 2 == 1 => v[n / 2] as f64,
                _ => (v[n / 2 - 1] + v[n / 2]) as f64 / 2.0,
            };
            *mean = if n == 0 {
                0.0
            } else {
                v.iter().sum::<u64>() as f64 / n as f64
            };
            cols += &format!(" {:.1} | {:.1} |", med / 1e3, *mean / 1e3);
        }
        let ratio = if means[0] > 0.0 {
            means[2] / means[0]
        } else {
            0.0
        };
        format!(
            "| {label} | {} | {} | {} | {} | {} |{cols} {ratio:.1} |\n",
            self.chunks, self.compared, self.different, self.skipped, self.banded
        )
    }
}

const HEAD: &str = "| ifd | chunks | compared | different | not compared | banded | raw median us | raw mean us | raw+jpeg median us | raw+jpeg mean us | tiff median us | tiff mean us | tiff/raw (mean) |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|\n";

/// Writes `data` as a PGM (1 sample) or PPM (3) of `w` pixels wide, as many rows as it holds.
fn write_pnm(dir: &str, base: &str, i: usize, w: usize, spp: usize, data: &[u8]) -> io::Result<()> {
    let (magic, ext) = if spp == 1 {
        ("P5", "pgm")
    } else {
        ("P6", "ppm")
    };
    let rb = w * spp;
    let h = data.len().checked_div(rb).unwrap_or(0);
    let mut f = BufWriter::new(fs::File::create(format!("{dir}/{base}.{i:06}.{ext}"))?);
    write!(f, "{magic}\n{w} {h}\n255\n")?;
    f.write_all(&data[..h * rb])?;
    f.flush()
}

struct Ctx {
    max: usize,
    out_a: String,
    out_b: String,
    cancel: AtomicBool,
    tdec: Decoder,
    jdec: jpeg::Decoder,
    raw: Vec<u8>,
    want: Vec<u8>,
    got: Vec<u8>,
    problems: Vec<String>,
    skips: Vec<String>,
    banded_times: Vec<String>,
}

impl Ctx {
    /// Compares every chunk of a page.
    fn ifd(
        &mut self,
        path: &str,
        base: &str,
        k: usize,
        page: &tiff::Page<'_, fs::File>,
    ) -> io::Result<Stats> {
        let info = page.info();
        let spp = info.samples_per_pixel;
        let lay = page.layout();
        let mut st = Stats {
            chunks: lay.chunks,
            ..Default::default()
        };
        let m = lay.chunks.min(self.max);
        let sel: Vec<usize> = (0..m).map(|j| j * lay.chunks / m).collect();
        let crb = lay.chunk_width as usize * spp;
        let ch = lay.chunk_height as usize;
        for i in 0..lay.chunks {
            let tag = format!("{path} ifd{k} chunk {i}");
            let rows = if lay.tiled {
                ch
            } else {
                ch.min(info.height.saturating_sub(i * ch))
            };
            if (crb as u64).saturating_mul(rows as u64) > BAND {
                self.big_chunk(path, k, i, &tag, page, &mut st);
                continue;
            }
            // Untimed: the file is read once before the timed calls, which then all find it in
            // the page cache.
            let _ = page.raw_jpeg_into(i, &mut self.raw);
            let t = Instant::now();
            let r_raw = page.raw_jpeg_into(i, &mut self.raw);
            let t_raw = t.elapsed();
            std::hint::black_box(&r_raw);
            let t = Instant::now();
            let b = match page.raw_jpeg_into(i, &mut self.raw) {
                Err(e) => Err(class(&e)),
                Ok(()) => jpeg_rows(
                    &mut self.jdec,
                    &self.raw,
                    spp,
                    None,
                    &self.cancel,
                    &mut self.got,
                ),
            };
            let t_rj = t.elapsed();
            let t = Instant::now();
            let a = page
                .decode_chunk_into(i, &mut self.tdec, &mut self.want, &self.cancel)
                .map_err(|e| class(&e));
            let t_dec = t.elapsed();
            match (a, b) {
                (Ok(()), Ok(())) => {
                    st.compared += 1;
                    let same = self.want == self.got;
                    if !same {
                        st.different += 1;
                        self.problems.push(format!("{tag}: pixels differ"));
                    }
                    st.ns.push([
                        t_raw.as_nanos() as u64,
                        t_rj.as_nanos() as u64,
                        t_dec.as_nanos() as u64,
                    ]);
                    if sel.binary_search(&i).is_ok() {
                        fs::create_dir_all(&self.out_a)?;
                        fs::create_dir_all(&self.out_b)?;
                        write_pnm(
                            &self.out_a,
                            base,
                            i,
                            lay.chunk_width as usize,
                            spp,
                            &self.want,
                        )?;
                        write_pnm(
                            &self.out_b,
                            base,
                            i,
                            lay.chunk_width as usize,
                            spp,
                            &self.got,
                        )?;
                    }
                }
                (Err(x), Err(y)) if x == y => {
                    st.skipped += 1;
                    self.skips.push(format!("{tag}: both sides fail, {x}"));
                }
                (a, b) => {
                    st.different += 1;
                    self.problems.push(format!("{tag}: tiff {a:?}, raw {b:?}"));
                }
            }
        }
        Ok(st)
    }

    /// A chunk over `BAND` decoded bytes, compared in bands.
    fn big_chunk(
        &mut self,
        path: &str,
        k: usize,
        i: usize,
        tag: &str,
        page: &tiff::Page<'_, fs::File>,
        st: &mut Stats,
    ) {
        let t = Instant::now();
        if let Err(e) = page.raw_jpeg_into(i, &mut self.raw) {
            st.different += 1;
            self.problems
                .push(format!("{tag}: raw_jpeg_into {}", class(&e)));
            return;
        }
        let t_raw = t.elapsed();
        let spp = page.info().samples_per_pixel;
        let (b, tj, tt) = banded(path, k, spp, &self.raw, &mut self.jdec, &self.cancel);
        match b {
            Band::Same => {
                st.compared += 1;
                st.banded += 1;
            }
            Band::Differs => {
                st.compared += 1;
                st.banded += 1;
                st.different += 1;
                self.problems.push(format!("{tag}: pixels differ (banded)"));
            }
            Band::Skip(why) => {
                st.skipped += 1;
                self.skips.push(format!("{tag}: not compared, {why}"));
            }
            Band::Fail(why) => {
                st.different += 1;
                self.problems.push(format!("{tag}: {why} (banded)"));
            }
        }
        self.banded_times.push(format!(
            "{tag}: raw {:.1} ms, jpeg region decodes {:.1} ms, tiff Page::rows {:.1} ms",
            t_raw.as_secs_f64() * 1e3,
            tj.as_secs_f64() * 1e3,
            tt.as_secs_f64() * 1e3
        ));
    }

    /// Every JPEG page of one slide; returns its table and its totals.
    fn slide(&mut self, path: &str, total: &mut Stats) -> io::Result<String> {
        let file = fs::File::open(path)?;
        let size = file.metadata()?.len();
        let name = Path::new(path)
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let mut rep = format!("### {name}\n\n");
        let f = match tiff::File::open(file, size, &limits()) {
            Ok(f) => f,
            Err(e) => {
                self.problems.push(format!("{path}: open {}", class(&e)));
                return Ok(rep + "Not opened.\n\n");
            }
        };
        let stem = name.replace(['.', '/'], "_");
        let mut sum = Stats::default();
        let mut table = String::new();
        let mut notes = String::new();
        for k in 0..f.num_pages() {
            let page = match f.page(k) {
                Ok(p) => p,
                Err(e) => {
                    notes += &format!("- ifd{k}: not parsed, {}\n", class(&e));
                    continue;
                }
            };
            let info = page.info();
            if info.compression != 7 || !matches!(info.samples_per_pixel, 1 | 3) {
                notes += &format!(
                    "- ifd{k}: not JPEG with 1 or 3 samples (compression {}, {} samples)\n",
                    info.compression, info.samples_per_pixel
                );
                continue;
            }
            let st = self.ifd(path, &format!("{stem}.ifd{k}"), k, &page)?;
            eprintln!(
                "{name} ifd{k}: {} chunks, {} compared, {} different, {} not compared",
                st.chunks, st.compared, st.different, st.skipped
            );
            table += &st.row(&format!("ifd{k}"));
            sum.add(&st);
        }
        rep += &(HEAD.to_string() + &table + &sum.row("all"));
        rep += "\n";
        if !notes.is_empty() {
            rep += &(notes + "\n");
        }
        total.add(&sum);
        Ok(rep)
    }
}

fn usage() -> ! {
    eprintln!("usage: rawjpeg [-max N] <out-a> <out-b> <report.md> <slide>...");
    std::process::exit(2);
}

fn main() -> io::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut max = 150usize;
    while args.first().is_some_and(|a| a == "-max") {
        let Some(v) = args.get(1) else { usage() };
        max = v.parse().unwrap_or_else(|_| usage());
        args.drain(..2);
    }
    if args.len() < 4 {
        usage();
    }
    let mut c = Ctx {
        max,
        out_a: args[0].clone(),
        out_b: args[1].clone(),
        cancel: AtomicBool::new(false),
        tdec: Decoder::new(),
        jdec: jpeg::Decoder::new(),
        raw: Vec::new(),
        want: Vec::new(),
        got: Vec::new(),
        problems: Vec::new(),
        skips: Vec::new(),
        banded_times: Vec::new(),
    };
    fs::create_dir_all(&c.out_a)?;
    fs::create_dir_all(&c.out_b)?;
    let mut total = Stats::default();
    let mut body = String::new();
    for s in &args[3..] {
        match c.slide(s, &mut total) {
            Ok(t) => body += &t,
            Err(e) => c.problems.push(format!("{s}: {e}")),
        }
    }
    let mut rep = String::from("## Raw JPEG tiles against the tiff decode\n\n");
    rep += "`Page::raw_jpeg_into` decoded by jpeg-rs against `Page::decode_chunk_into`, every chunk of every JPEG page, one thread each. Times are per tile, microseconds, over the chunks decoded whole (banded chunks are not timed here, see below); `tiff/raw` is the mean tiff decode over the mean raw call.\n\n";
    rep += &body;
    rep += "### Total\n\n";
    rep += &(HEAD.replace("| ifd |", "| all slides |") + &total.row("all"));
    rep += &format!(
        "\n{} chunks, {} compared ({} in bands), {} different, {} not compared.\n\n",
        total.chunks, total.compared, total.banded, total.different, total.skipped
    );
    let lists = [
        (
            "Banded chunks (decoded size over the band size)",
            &c.banded_times,
        ),
        ("Chunks not compared", &c.skips),
        ("Errors and differences", &c.problems),
    ];
    for (title, list) in lists {
        if list.is_empty() {
            continue;
        }
        rep += &format!("### {title}\n\n");
        for l in list.iter().take(200) {
            rep += &format!("- {l}\n");
        }
        if list.len() > 200 {
            rep += &format!("- ... and {} more\n", list.len() - 200);
        }
        rep += "\n";
    }
    fs::write(&args[2], rep)?;
    for p in c.problems.iter().take(20) {
        eprintln!("{p}");
    }
    if !c.problems.is_empty() || total.different > 0 {
        std::process::exit(1);
    }
    Ok(())
}
