// Command oracle is the Go side of the parity check (see ../../../../docs/parity.md).
//
//	oracle -gen <testdata> <inputs>   write the test inputs
//	oracle <inputs> <outputs>         decode every input
//	oracle -genbench <testdata> <dir> write the benchmark inputs
//	oracle -bench N <dir>             time the decode of every file in dir
//	oracle -benchraw N <file>         time the raw JPEG decode of a JPEG strip file's strips
//	oracle -qtiles -max N -log <logfile> <out-dir> <slide>...
//	                                  write sampled 256x256 cells of real slides (qtiles.go)
//
// Decode mode writes, per input and limits configuration <cfg>, <name>.<cfg>.info (page count
// and every page's Info, or its error class) and, per page and operation, <name>.<cfg>.p<i>.<op>.out
// (the bytes of all bands) or .err (an error class), .partial.out (the bytes before the error) and
// .bands (one "y len" line per band).
// examples/oracle.rs does the same with the Rust crate.
package main

import (
	"bufio"
	"bytes"
	"context"
	"errors"
	"flag"
	"fmt"
	"hash/fnv"
	"math"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/dobbo-ca/jpeg-go"
	tiff "github.com/dobbo-ca/tiff-go"
)

// memLimit bounds jpeg-go's allocations (the oracle runs inside BuildKit); the Rust oracle gives
// max_alloc the same value.
const memLimit = 256 << 20

// maxPages is how many pages of one file an input decodes.
const maxPages = 64

type config struct {
	name string
	lim  tiff.Limits
	ops  []string // rows: all bands; c1: cancel after the first band; k1: the callback fails after it
}

// The d limits are the FuzzTIFF ones with the default IFD cap; s has a small band and a small
// metadata budget; g is Go's defaults (the zero Limits).
var configs = []config{
	{"d", tiff.Limits{MaxIFDs: 10000, MaxPixels: 1 << 22, MaxBandBytes: 1 << 20, MaxMetadataBytes: 4 << 20}, []string{"rows", "c1", "k1"}},
	{"s", tiff.Limits{MaxIFDs: 10000, MaxPixels: 1 << 22, MaxBandBytes: 2000, MaxMetadataBytes: 8}, []string{"rows"}},
	{"g", tiff.Limits{}, []string{"rows"}},
}

var errStop = errors.New("stop")

func main() {
	gen := flag.Bool("gen", false, "write the inputs: -gen <testdata> <inputs>")
	genBench := flag.Bool("genbench", false, "write the benchmark inputs: -genbench <testdata> <dir>")
	bench := flag.Int("bench", 0, "decode every file N times: -bench N <dir>")
	benchRawN := flag.Int("benchraw", 0, "raw JPEG decode of each strip against the full decode, best of N: -benchraw N <file>")
	qt := flag.Bool("qtiles", false, "write sampled cells of real slides: -qtiles [-max N] [-log FILE] <out-dir> <slide>...")
	qmax := flag.Int("max", 150, "with -qtiles: the most cells written per page")
	qlog := flag.String("log", "", "with -qtiles: the file for the skipped and failed pages (stderr when empty)")
	flag.Parse()
	a := flag.Args()
	switch {
	case *qt && len(a) >= 2:
		check(qtiles(a[0], *qlog, *qmax, a[1:]))
	case *benchRawN > 0 && len(a) == 1:
		check(benchRaw(a[0], *benchRawN))
	case *bench > 0 && len(a) == 1:
		check(benchDir(a[0], *bench))
	case *gen && len(a) == 2:
		check(generate(a[0], a[1]))
	case *genBench && len(a) == 2:
		check(generateBench(a[0], a[1]))
	case !*gen && !*genBench && *bench == 0 && len(a) == 2:
		check(decodeDir(a[0], a[1]))
	default:
		flag.Usage()
		os.Exit(2)
	}
}

func check(err error) {
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

// limitName keeps the names tiff-go itself uses and the first word of the others (the jpeg and
// deflate parts name their own caps).
func limitName(n string) string {
	switch n {
	case "ifds", "pixels", "jpeg tables", "ifd entries", "band bytes":
		return strings.ReplaceAll(n, " ", "-")
	}
	if i := strings.IndexByte(n, ' '); i > 0 {
		return n[:i]
	}
	return n
}

// class is the stable token of an error; docs/parity.md has the table.
func class(err error) string {
	var le *tiff.LimitError
	switch {
	case errors.Is(err, errStop):
		return "stopped"
	case errors.Is(err, context.Canceled):
		return "cancelled"
	case errors.As(err, &le):
		return "limit:" + limitName(le.Name)
	case errors.Is(err, jpeg.ErrMemoryLimit):
		return "limit:jpeg"
	case errors.Is(err, tiff.ErrCorrupt):
		return "corrupt"
	case errors.Is(err, tiff.ErrUnsupported):
		return "unsupported"
	}
	return "other:" + err.Error()
}

func fnv64(b []byte) uint64 {
	h := fnv.New64a()
	h.Write(b)
	return h.Sum64()
}

func decodeDir(in, out string) error {
	jpeg.SetMemoryLimit(memLimit)
	files, err := os.ReadDir(in)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(out, 0o755); err != nil {
		return err
	}
	names := make([]string, 0, len(files))
	for _, f := range files {
		if !f.IsDir() {
			names = append(names, f.Name())
		}
	}
	sort.Strings(names)
	for _, n := range names {
		b, err := os.ReadFile(filepath.Join(in, n))
		if err != nil {
			return err
		}
		for _, c := range configs {
			if err := decodeOne(filepath.Join(out, n+"."+c.name), b, c); err != nil {
				return err
			}
		}
	}
	return nil
}

// sink writes the bands of one operation to <base>.partial.out and renames it to <base>.out when
// the operation succeeds.
type sink struct {
	f    *os.File
	w    *bufio.Writer
	base string
	n    int
}

func newSink(base string) (*sink, error) {
	f, err := os.Create(base + ".partial.out")
	if err != nil {
		return nil, err
	}
	return &sink{f: f, w: bufio.NewWriterSize(f, 1<<16), base: base}, nil
}

func (s *sink) write(b []byte) {
	s.n += len(b)
	s.w.Write(b)
}

func (s *sink) finish(err error) error {
	if e := s.w.Flush(); e != nil {
		return e
	}
	if e := s.f.Close(); e != nil {
		return e
	}
	if err == nil {
		return os.Rename(s.base+".partial.out", s.base+".out")
	}
	if s.n == 0 {
		if e := os.Remove(s.base + ".partial.out"); e != nil {
			return e
		}
	}
	return os.WriteFile(s.base+".err", []byte(class(err)+"\n"), 0o644)
}

// run decodes page p with op into <base>.<op>.*.
func run(base, op string, p *tiff.Page) error {
	s, err := newSink(base + "." + op)
	if err != nil {
		return err
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	bands := 0
	var layout strings.Builder
	err = p.Rows(ctx, func(y int, rows []byte) error {
		s.write(rows)
		fmt.Fprintf(&layout, "%d %d\n", y, len(rows))
		bands++
		if bands == 1 {
			switch op {
			case "c1":
				cancel()
			case "k1":
				return errStop
			}
		}
		return nil
	})
	if werr := os.WriteFile(base+"."+op+".bands", []byte(layout.String()), 0o644); werr != nil {
		return werr
	}
	return s.finish(err)
}

func entries(sb *strings.Builder, tag string, es []tiff.TagEntry) {
	for i, e := range es {
		data := "nil"
		if e.Data != nil {
			data = fmt.Sprintf("%d:%016x", len(e.Data), fnv64(e.Data))
		}
		fmt.Fprintf(sb, "%s%d typ=%d count=%d val=%d oor=%t bad=%t over=%t dup=%t data=%s\n",
			tag, i, e.Type, e.Count, e.Value, e.OutOfRange, e.BadType, e.OverLimit, e.Duplicate, data)
	}
}

func u16s(v []uint16) string {
	var b bytes.Buffer
	for _, x := range v {
		fmt.Fprintf(&b, "%d,", x)
	}
	return b.String()
}

func info(sb *strings.Builder, i int, p *tiff.Page) {
	in := p.Info()
	cm := make([]byte, 0, 2*len(in.ColorMap))
	for _, c := range in.ColorMap {
		cm = append(cm, byte(c), byte(c>>8))
	}
	fmt.Fprintf(sb, "p%d w=%d h=%d spp=%d bps=%d photo=%d comp=%d orient=%d rowbytes=%d resunit=%d xres=%016x yres=%016x\n",
		i, in.Width, in.Height, in.SamplesPerPixel, in.BitsPerSample, in.Photometric, in.Compression, in.Orientation,
		in.RowBytes, in.ResolutionUnit, math.Float64bits(in.XResolution), math.Float64bits(in.YResolution))
	fmt.Fprintf(sb, "p%d cmap=%d:%016x extra=%s skipped=%s\n", i, len(in.ColorMap), fnv64(cm), u16s(in.ExtraSamples), u16s(in.Skipped))
	entries(sb, fmt.Sprintf("p%d or", i), p.Orientations)
	entries(sb, fmt.Sprintf("p%d xmp", i), p.XMPs)
}

func decodeOne(base string, b []byte, c config) error {
	var sb strings.Builder
	f, err := tiff.Open(bytes.NewReader(b), int64(len(b)), c.lim)
	if err != nil {
		fmt.Fprintf(&sb, "open err=%s\n", class(err))
		return os.WriteFile(base+".info", []byte(sb.String()), 0o644)
	}
	fmt.Fprintf(&sb, "pages=%d\n", f.NumPages())
	var ok []int
	pages := map[int]*tiff.Page{}
	for i := 0; i < f.NumPages() && i < maxPages; i++ {
		p, err := f.Page(i)
		if err != nil {
			fmt.Fprintf(&sb, "p%d err=%s\n", i, class(err))
			continue
		}
		info(&sb, i, p)
		ok = append(ok, i)
		pages[i] = p
	}
	if err := os.WriteFile(base+".info", []byte(sb.String()), 0o644); err != nil {
		return err
	}
	for _, i := range ok {
		for _, op := range c.ops {
			if err := run(fmt.Sprintf("%s.p%d", base, i), op, pages[i]); err != nil {
				return err
			}
		}
	}
	return nil
}
