package main

import (
	"bytes"
	"context"
	"encoding/binary"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/dobbo-ca/jpeg-go"
	tiff "github.com/dobbo-ca/tiff-go"

	"tiff-parity-oracle/fixture"
)

// generateBench writes the benchmark inputs: one large image per compression and layout, named
// so that tests/parity/bench.sh can group them (none, lzw, packbits, deflate, jpeg, ccitt), and the
// Go testdata's CCITT file.
func generateBench(testdata, dir string) error {
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	const w, h = 1536, 1024
	var err error
	write := func(name string, b []byte) {
		if err == nil {
			err = os.WriteFile(filepath.Join(dir, name), b, 0o644)
		}
	}
	for _, c := range []struct {
		name         string
		comp, pred   int
		spp, bps     int
		layout       string
		littleEndian bool
	}{
		{"none-rgb-strip", 1, 0, 3, 8, "strip", false},
		{"none-rgb-tile", 1, 0, 3, 8, "tile", false},
		{"none-g16-strip", 1, 0, 1, 16, "strip", true},
		{"lzw-rgb-strip", 5, 0, 3, 8, "strip", false},
		{"lzw-rgb-tile", 5, 0, 3, 8, "tile", false},
		{"lzw-pred-rgb-strip", 5, 2, 3, 8, "strip", false},
		{"packbits-rgb-strip", 32773, 0, 3, 8, "strip", false},
		{"packbits-rgb-tile", 32773, 0, 3, 8, "tile", false},
		{"deflate-rgb-strip", 8, 0, 3, 8, "strip", false},
		{"deflate-rgb-tile", 8, 0, 3, 8, "tile", false},
		{"deflate-pred-g16-strip", 8, 2, 1, 16, "strip", true},
		{"jpeg-rgb-strip", 7, 0, 3, 8, "strip", false},
		{"jpeg-rgb-tile", 7, 0, 3, 8, "tile", false},
		{"jpeg-grey-strip", 7, 0, 1, 8, "strip", false},
	} {
		// Photometric 6 for RGB JPEG: the stream is YCbCr; tiff-rs reads JPEG under Photometric
		// RGB as RGB as stored (docs/go-differences.md).
		p := fixture.Page{W: w, H: h, SPP: c.spp, BPS: c.bps, Photometric: photometricOf(c.spp, c.comp), Compression: c.comp, Predictor: c.pred, RowsPerStrip: 64}
		if c.layout == "tile" {
			p.RowsPerStrip, p.TileW, p.TileL = 0, 256, 256
		}
		if c.comp == 7 {
			p.Pix = smooth(w, h, c.spp)
		}
		write(c.name+".tif", fx(c.littleEndian, false, p))
	}
	b, rerr := os.ReadFile(filepath.Join(testdata, "golden", "bw_g4_r16.tif"))
	if rerr != nil {
		return rerr
	}
	write("ccitt-g4.tif", b)
	// The same data at a size worth timing: the first nine strips (16 rows each, independent
	// CCITT data) of the golden files, repeated 1000 times, 203 x 144000 (3.7 MB of output).
	for _, c := range []struct {
		name, file string
		comp, t4   int
	}{
		{"ccitt-g4-large", "bw_g4_r16.tif", 4, 0},
		{"ccitt-g3-1d-large", "bw_g3pfill_r16.tif", 3, 4},
		{"ccitt-g3-2d-large", "bw_g3p2d_r16.tif", 3, 1},
	} {
		src, rerr := os.ReadFile(filepath.Join(testdata, "golden", c.file))
		if rerr != nil {
			return rerr
		}
		strips, rerr := stripsOf(src, 9)
		if rerr != nil {
			return rerr
		}
		const reps = 1000
		chunks := make([][]byte, 0, reps*len(strips))
		for i := 0; i < reps; i++ {
			chunks = append(chunks, strips...)
		}
		write(c.name+".tif", fx(true, false, fixture.Page{W: 203, H: 16 * len(chunks), SPP: 1, BPS: 1, Photometric: 1, Compression: c.comp, RowsPerStrip: 16, T4: uint32(c.t4), Chunks: chunks}))
	}
	// The row-reader paths: 400 strips of the 40x72 progressive JPEG (jpeg-go's RowReader), and
	// the 4-component JPEG file of the synthetic testdata (the 4-sample decoder).
	pj, rerr := os.ReadFile(filepath.Join(testdata, "synthetic", "jpeg_progressive_420.jpg"))
	if rerr != nil {
		return rerr
	}
	const strips = 400
	chunks := make([][]byte, strips)
	for i := range chunks {
		chunks[i] = pj
	}
	// Photometric 6: the stream is YCbCr (and 4:2:0, which tiff-rs refuses under Photometric RGB);
	// tiff-rs reads JPEG under Photometric RGB as RGB as stored (docs/go-differences.md).
	write("jpeg-prog-rgb-strip.tif", fx(false, false, fixture.Page{W: 40, H: 72 * strips, SPP: 3, BPS: 8, Photometric: 6, Compression: 7, RowsPerStrip: 72, Chunks: chunks}))
	c4, rerr := os.ReadFile(filepath.Join(testdata, "synthetic", "cmyk_jpeg_bs.tif"))
	if rerr != nil {
		return rerr
	}
	write("jpeg4-cmyk-strip.tif", c4)
	return err
}

// stripsOf returns the first n strips of a classic little-endian TIFF.
func stripsOf(b []byte, n int) ([][]byte, error) {
	le := binary.LittleEndian
	if len(b) < 8 || string(b[:4]) != "II*\x00" {
		return nil, fmt.Errorf("not a classic little-endian TIFF")
	}
	ifd := int(le.Uint32(b[4:]))
	var offs, cnts int // the offsets of the StripOffsets and StripByteCounts arrays
	for i, c := 0, int(le.Uint16(b[ifd:])); i < c; i++ {
		e := b[ifd+2+12*i:]
		switch le.Uint16(e) {
		case 273:
			offs = int(le.Uint32(e[8:]))
		case 279:
			cnts = int(le.Uint32(e[8:]))
		}
	}
	if offs == 0 || cnts == 0 {
		return nil, fmt.Errorf("no strips")
	}
	strips := make([][]byte, n)
	for i := range strips {
		o, c := int(le.Uint32(b[offs+4*i:])), int(le.Uint32(b[cnts+4*i:]))
		strips[i] = b[o : o+c]
	}
	return strips, nil
}

// benchDir decodes every page of every file in dir n times and prints name, output bytes and ns
// per decode, as examples/bench.rs does; a file that does not decode is skipped.
func benchDir(dir string, n int) error {
	files, err := os.ReadDir(dir)
	if err != nil {
		return err
	}
	var names []string
	for _, f := range files {
		if strings.HasSuffix(f.Name(), ".tif") {
			names = append(names, f.Name())
		}
	}
	sort.Strings(names)
	for _, name := range names {
		b, err := os.ReadFile(filepath.Join(dir, name))
		if err != nil {
			return err
		}
		decode := func() (int, error) {
			f, err := tiff.Open(bytes.NewReader(b), int64(len(b)), tiff.Limits{})
			if err != nil {
				return 0, err
			}
			total := 0
			for i := 0; i < f.NumPages(); i++ {
				p, err := f.Page(i)
				if err != nil {
					return 0, err
				}
				if err := p.Rows(context.Background(), func(_ int, rows []byte) error { total += len(rows); return nil }); err != nil {
					return 0, err
				}
			}
			return total, nil
		}
		size, err := decode()
		if err != nil {
			continue
		}
		t := time.Now()
		for i := 0; i < n; i++ {
			if _, err := decode(); err != nil {
				return err
			}
		}
		fmt.Printf("%s\t%d\t%d\n", name, size, time.Since(t).Nanoseconds()/int64(n))
	}
	return nil
}

// benchRaw times, for one JPEG strip file, the raw jpeg.Decode of each strip (FormatGray, the
// JPEGTables, as tiff-go does) against the whole tiff decode, best of n each, and prints ms/MB.
// examples/bench.rs --raw does the same for Rust. It tells whether a gap is inside the JPEG
// decoder or in the TIFF layer around it.
func benchRaw(path string, n int) error {
	b, err := os.ReadFile(path)
	if err != nil {
		return err
	}
	chunks, tables, err := jpegChunks(b)
	if err != nil {
		return err
	}
	o := jpeg.DecodeOptions{Format: jpeg.FormatGray, Tables: tables}
	raw := func() error {
		for _, c := range chunks {
			if _, err := jpeg.Decode(context.Background(), c, &o); err != nil {
				return err
			}
		}
		return nil
	}
	size := 0
	full := func() error {
		f, err := tiff.Open(bytes.NewReader(b), int64(len(b)), tiff.Limits{})
		if err != nil {
			return err
		}
		size = 0
		p, err := f.Page(0)
		if err != nil {
			return err
		}
		return p.Rows(context.Background(), func(_ int, rows []byte) error { size += len(rows); return nil })
	}
	best := func(f func() error) (time.Duration, error) {
		m := time.Duration(1 << 62)
		for i := 0; i < n; i++ {
			t := time.Now()
			if err := f(); err != nil {
				return 0, err
			}
			m = min(m, time.Since(t))
		}
		return m, nil
	}
	tr, err := best(raw)
	if err != nil {
		return err
	}
	tf, err := best(full)
	if err != nil {
		return err
	}
	fmt.Printf("raw go: %.3f ms/MB  full go: %.3f ms/MB  (%d strips, %d bytes, best of %d)\n",
		float64(tr.Nanoseconds())/float64(size), float64(tf.Nanoseconds())/float64(size), len(chunks), size, n)
	return nil
}

// jpegChunks returns the strips and the JPEGTables (nil when absent) of a classic TIFF (either byte order).
func jpegChunks(b []byte) ([][]byte, []byte, error) {
	var bo binary.ByteOrder = binary.LittleEndian
	if len(b) < 8 || (string(b[:2]) != "II" && string(b[:2]) != "MM") {
		return nil, nil, fmt.Errorf("not a classic TIFF")
	}
	if b[0] == 'M' {
		bo = binary.BigEndian
	}
	ifd := int(bo.Uint32(b[4:]))
	var offs, cnts, tabOff, tabLen, n int
	var typ uint16
	for i, c := 0, int(bo.Uint16(b[ifd:])); i < c; i++ {
		e := b[ifd+2+12*i:]
		switch bo.Uint16(e) {
		case 273:
			offs, n, typ = int(bo.Uint32(e[8:])), int(bo.Uint32(e[4:])), bo.Uint16(e[2:])
		case 279:
			cnts = int(bo.Uint32(e[8:]))
		case 347:
			tabLen, tabOff = int(bo.Uint32(e[4:])), int(bo.Uint32(e[8:]))
		}
	}
	if typ != 4 || offs == 0 || cnts == 0 {
		return nil, nil, fmt.Errorf("no LONG strips")
	}
	chunks := make([][]byte, n)
	for i := range chunks {
		o, c := int(bo.Uint32(b[offs+4*i:])), int(bo.Uint32(b[cnts+4*i:]))
		chunks[i] = b[o : o+c]
	}
	if tabOff == 0 { // the bench files carry whole streams, no JPEGTables
		return chunks, nil, nil
	}
	return chunks, b[tabOff : tabOff+tabLen], nil
}
