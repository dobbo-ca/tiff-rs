package main

import (
	"encoding/binary"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"

	"tiff-parity-oracle/fixture"
)

// rng is xorshift64: the inputs do not depend on the Go version.
type rng uint64

func (r *rng) next() uint64 {
	x := uint64(*r)
	x ^= x << 13
	x ^= x >> 7
	x ^= x << 17
	*r = rng(x)
	return x
}

func (r *rng) intn(n int) int { return int(r.next() % uint64(n)) }

func seedOf(name string) rng { return rng(fnv64([]byte(name)) | 1) }

type gen struct {
	dir   string
	n     int
	err   error
	bases []string
	data  map[string][]byte
	used  map[string]int
}

func (g *gen) emit(name string, b []byte) {
	if g.err != nil {
		return
	}
	g.n++
	// A name used before gets a numeric suffix, so no input overwrites another.
	if g.used == nil {
		g.used = map[string]int{}
	}
	g.used[name]++
	if k := g.used[name]; k > 1 {
		name = fmt.Sprintf("%s-dup%d", name, k)
	}
	g.err = os.WriteFile(filepath.Join(g.dir, name), b, 0o644)
}

// base writes an input and makes it a source of mutations.
func (g *gen) base(name string, b []byte) {
	g.emit(name, b)
	g.bases = append(g.bases, name)
	g.data[name] = b
}

func fx(le, big bool, pp ...fixture.Page) []byte {
	return fixture.File{LittleEndian: le, Big: big, Pages: pp}.Bytes()
}

func photometric(spp int) int { return map[int]int{1: 1, 3: 2, 4: 5}[spp] }

// photometricOf is photometric for a page of compression comp. Photometric 6: the stream is
// YCbCr; tiff-rs reads JPEG under Photometric RGB as RGB as stored (docs/go-differences.md).
func photometricOf(spp, comp int) int {
	if comp == 7 && spp == 3 {
		return 6
	}
	return photometric(spp)
}

func b2i(b bool) int {
	if b {
		return 1
	}
	return 0
}

func endian(le bool) string {
	if le {
		return "le"
	}
	return "be"
}

// generate writes every input: the ported Go tests' files and generators, the Go testdata, and
// mutations of a subset.
func generate(testdata, dir string) error {
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	g := &gen{dir: dir, data: map[string][]byte{}}
	g.roundTrip()
	g.layouts()
	g.limits()
	g.refusals()
	g.entries()
	if err := g.testdata(testdata); err != nil {
		return err
	}
	if g.err != nil {
		return g.err
	}
	plain := g.n
	sort.Strings(g.bases)
	for _, n := range g.bases {
		g.mutate(n, g.data[n])
	}
	fmt.Fprintf(os.Stderr, "%d inputs, %d bases, %d mutations\n", g.n, len(g.bases), g.n-plain)
	return g.err
}

// roundTrip is TestRoundTrip: every compression, sample format, predictor, layout and byte order.
func (g *gen) roundTrip() {
	formats := []struct{ spp, bps int }{{1, 1}, {1, 4}, {1, 8}, {3, 8}, {4, 8}, {1, 16}, {3, 16}}
	for _, comp := range []int{1, 5, 8, 32946, 32773} {
		for fi, f := range formats {
			for _, pred := range []int{0, 2} {
				if pred == 2 && f.bps < 8 {
					continue
				}
				for _, lay := range []string{"strip1", "strip7", "whole", "tile"} {
					for _, le := range []bool{false, true} {
						for _, big := range []bool{false, true} {
							if big && (comp != 1 || lay != "whole" || f.spp != 3) && (!le || f.bps != 16) {
								continue
							}
							p := fixture.Page{W: 37, H: 50, SPP: f.spp, BPS: f.bps, Photometric: photometric(f.spp), Compression: comp, Predictor: pred}
							switch lay {
							case "strip1":
								p.RowsPerStrip = 1
							case "strip7":
								p.RowsPerStrip = 7
							case "tile":
								p.TileW, p.TileL = 16, 16
							}
							name := fmt.Sprintf("rt-c%d-%dx%d-p%d-%s-%s-b%d", comp, f.spp, f.bps, pred, lay, endian(le), b2i(big))
							b := fx(le, big, p)
							// a few hundred base files for the mutations
							if (lay == "strip7" || lay == "tile") && (fi == 3 || fi == 5 || fi == 0) && pred == (fi%2)*2 && le == (comp%2 == 1) && !big {
								g.base(name, b)
							} else if big && le && f.bps == 16 && lay == "tile" && comp != 1 {
								g.base(name, b)
							} else {
								g.emit(name, b)
							}
						}
					}
				}
			}
		}
	}
}

func smooth(w, h, spp int) []byte {
	pix := make([]byte, w*spp*h)
	for i := range pix {
		pix[i] = byte(i%(w*spp)/spp*2 + i/(w*spp))
	}
	return pix
}

// layouts: TestMultiPage, TestSingleStripStreams, TestJPEG, TestInfoPalette, TestCCITT.
func (g *gen) layouts() {
	var pages []fixture.Page
	for i := 0; i < 3; i++ {
		p := fixture.Page{W: 10 + i, H: 5 + 2*i, SPP: 1, BPS: 8, Photometric: 1, Compression: 5 * (i % 2)}
		p.Pix = fixture.Pattern(p.W, p.H, 1, 8)
		pages = append(pages, p)
	}
	g.base("multipage", fx(false, false, pages...))
	g.base("multipage-big-le", fx(true, true, pages...))
	for _, comp := range []int{1, 5, 8, 32773, 7} {
		// Photometric 6 for JPEG: the stream is YCbCr; tiff-rs reads JPEG under Photometric RGB as
		// RGB as stored (docs/go-differences.md).
		p := fixture.Page{W: 600, H: 300, SPP: 3, BPS: 8, Photometric: photometricOf(3, comp), Compression: comp}
		g.emit(fmt.Sprintf("strip-c%d", comp), fx(false, false, p))
	}
	for _, lay := range []string{"strips", "tiles"} {
		for _, spp := range []int{1, 3} {
			// Photometric 6: the stream is YCbCr; tiff-rs reads JPEG under Photometric RGB as RGB as
			// stored (docs/go-differences.md).
			p := fixture.Page{W: 70, H: 50, SPP: spp, BPS: 8, Photometric: photometricOf(spp, 7), Compression: 7, RowsPerStrip: 20}
			if lay == "tiles" {
				p.RowsPerStrip, p.TileW, p.TileL = 0, 32, 32
			}
			p.Pix = smooth(70, 50, spp)
			g.base(fmt.Sprintf("jpeg-%s-%d", lay, spp), fx(false, false, p))
		}
	}
	cm := make([]uint16, 3*16)
	for i := range cm {
		cm[i] = uint16(i * 1000)
	}
	g.base("palette", fx(true, false, fixture.Page{W: 8, H: 8, SPP: 1, BPS: 4, Photometric: 3, ColorMap: cm}))
	bits := "1" + "001" + "00110101" + "000101" + "11" + "000000000001000000000001"
	for len(bits)%8 != 0 {
		bits += "0"
	}
	var data []byte
	for i := 0; i < len(bits); i += 8 {
		var v byte
		for _, c := range bits[i : i+8] {
			v = v<<1 | byte(c-'0')
		}
		data = append(data, v)
	}
	for _, photo := range []int{0, 1} {
		g.base(fmt.Sprintf("ccitt-g4-p%d", photo), fx(false, false, fixture.Page{W: 8, H: 3, SPP: 1, BPS: 1, Photometric: photo, Compression: 4, Chunks: [][]byte{data}}))
	}
	g.emit("ccitt-g3", fx(false, false, fixture.Page{W: 8, H: 3, SPP: 1, BPS: 1, Photometric: 0, Compression: 3, Chunks: [][]byte{data}}))
	g.emit("ccitt-g3-2d", fx(false, false, fixture.Page{W: 8, H: 3, SPP: 1, BPS: 1, Photometric: 0, Compression: 3, T4: 1, Chunks: [][]byte{data}}))
	g.emit("ccitt-g3-fill", fx(false, false, fixture.Page{W: 8, H: 3, SPP: 1, BPS: 1, Photometric: 0, Compression: 3, T4: 4, Chunks: [][]byte{data}}))
	g.emit("ccitt-rgb", fx(false, false, fixture.Page{W: 8, H: 3, SPP: 3, BPS: 8, Photometric: 2, Compression: 4, Chunks: [][]byte{data}}))
	// the FuzzTIFF seeds
	for _, comp := range []int{1, 5, 7, 8, 32773} {
		for _, tiles := range []bool{false, true} {
			// Photometric 6 for JPEG: the stream is YCbCr; tiff-rs reads JPEG under Photometric RGB
			// as RGB as stored (docs/go-differences.md).
			p := fixture.Page{W: 33, H: 21, SPP: 3, BPS: 8, Photometric: photometricOf(3, comp), Compression: comp, RowsPerStrip: 5}
			name := fmt.Sprintf("fz-c%d-strips", comp)
			if tiles {
				p.RowsPerStrip, p.TileW, p.TileL = 0, 16, 16
				name = fmt.Sprintf("fz-c%d-tiles", comp)
			}
			g.emit(name, fx(false, false, p, p))
		}
	}
}

type tag struct {
	id, typ uint16
	v       uint32
}

// raw is a little-endian classic TIFF: header, a data blob, then one IFD (hostile_test.go).
func raw(data []byte, tags []tag) []byte {
	b := append([]byte("II*\x00\x00\x00\x00\x00"), data...)
	binary.LittleEndian.PutUint32(b[4:], uint32(len(b)))
	b = binary.LittleEndian.AppendUint16(b, uint16(len(tags)))
	for _, t := range tags {
		b = binary.LittleEndian.AppendUint16(b, t.id)
		b = binary.LittleEndian.AppendUint16(b, t.typ)
		b = binary.LittleEndian.AppendUint32(b, 1)
		b = binary.LittleEndian.AppendUint32(b, t.v)
	}
	return binary.LittleEndian.AppendUint32(b, 0)
}

// limits: TestLimits, TestHostileHeaders.
func (g *gen) limits() {
	page := func(w, h int) fixture.Page { return fixture.Page{W: w, H: h, SPP: 1, BPS: 8, Photometric: 1} }
	many := func(n int) []byte {
		pp := make([]fixture.Page, n)
		for i := range pp {
			pp[i] = page(1, 1)
		}
		return fx(false, false, pp...)
	}
	g.emit("lim-ifds-10000", many(10000))
	g.emit("lim-ifds-10001", many(10001))
	b := fx(false, false, page(1, 1), page(1, 1))
	copy(b[len(b)-4:], b[4:8])
	g.emit("lim-ifd-loop", b)
	g.emit("lim-pixels", fx(false, false, fixture.Page{W: 2100, H: 2100, SPP: 1, BPS: 8, Photometric: 1, Chunks: [][]byte{{0}}, Pix: []byte{0}}))
	g.emit("lim-huge", fx(false, false, fixture.Page{W: 1 << 31, H: 1 << 31, SPP: 1, BPS: 8, Photometric: 1, Chunks: [][]byte{{0}}, Pix: []byte{0}}))
	g.base("lim-row-wider", fx(false, false, page(2100, 4)))
	t := page(2112, 64)
	t.TileW, t.TileL = 16, 16
	g.emit("lim-tile-row-wider", fx(false, false, t))
	for name, tags := range map[string][]tag{
		"rows-per-strip-0":       {{256, 4, 4}, {257, 4, 4}, {262, 3, 1}, {273, 4, 8}, {278, 4, 0}, {279, 4, 16}},
		"tile-wider-than-a-band": {{256, 4, 1}, {257, 4, 1}, {262, 3, 2}, {277, 3, 3}, {322, 4, 1 << 28}, {323, 4, 1}, {324, 4, 8}, {325, 4, 16}},
	} {
		g.base("hostile-"+name, raw(make([]byte, 16), tags))
	}
	g.base("fillorder2", raw([]byte{7, 0, 0, 0}, []tag{{256, 4, 1}, {257, 4, 1}, {258, 3, 8}, {262, 3, 1}, {266, 3, 2}, {273, 4, 8}, {278, 4, 1}, {279, 4, 1}}))
}

// refusals: TestRefusals.
func (g *gen) refusals() {
	base := fixture.Page{W: 8, H: 8, SPP: 1, BPS: 8, Photometric: 1}
	for name, mut := range map[string]func(*fixture.Page){
		"lab":          func(p *fixture.Page) { p.Photometric = 8 },
		"old-jpeg":     func(p *fixture.Page) { p.Compression = 6 },
		"float-pred":   func(p *fixture.Page) { p.Predictor = 3 },
		"pred-at-4bit": func(p *fixture.Page) { p.Predictor, p.BPS = 2, 4 },
	} {
		p := base
		mut(&p)
		p.Chunks = [][]byte{make([]byte, 64)}
		g.emit("refuse-"+name, fx(false, false, p))
	}
	for _, comp := range []int{1, 5, 8, 32773} {
		p := base
		p.Compression = comp
		p.Chunks = [][]byte{{1, 2}}
		g.emit(fmt.Sprintf("refuse-truncated-c%d", comp), fx(false, false, p))
	}
	g.emit("refuse-not-a-tiff", []byte("GIF89a.."))
}

// oob is oobTIFF of skip_test.go: a 1x1 8-bit grey TIFF, classic or BigTIFF, plus one extra tag
// whose count and value field are written as given.
func oob(big bool, tag, typ uint16, count, val uint64, more ...[4]uint64) []byte {
	le := binary.LittleEndian
	var b []byte
	if big {
		b = append([]byte("II+\x00\x08\x00\x00\x00"), make([]byte, 8)...)
	} else {
		b = []byte("II*\x00\x00\x00\x00\x00")
	}
	data := uint64(len(b))
	b = append(b, 7)
	if big {
		le.PutUint64(b[8:], uint64(len(b)))
	} else {
		le.PutUint32(b[4:], uint32(len(b)))
	}
	tags := [][4]uint64{{256, 4, 1, 1}, {257, 4, 1, 1}, {258, 3, 1, 8}, {262, 3, 1, 1},
		{273, 4, 1, data}, {278, 4, 1, 1}, {279, 4, 1, 1}, {uint64(tag), uint64(typ), count, val}}
	tags = append(tags, more...)
	if big {
		b = le.AppendUint64(b, uint64(len(tags)))
	} else {
		b = le.AppendUint16(b, uint16(len(tags)))
	}
	for _, t := range tags {
		b = le.AppendUint16(b, uint16(t[0]))
		b = le.AppendUint16(b, uint16(t[1]))
		if big {
			b = le.AppendUint64(le.AppendUint64(b, t[2]), t[3])
		} else {
			b = le.AppendUint32(le.AppendUint32(b, uint32(t[2])), uint32(t[3]))
		}
	}
	if big {
		return le.AppendUint64(b, 0)
	}
	return le.AppendUint32(b, 0)
}

// entries: the oobTIFF tests of entries_test.go and skip_test.go.
func (g *gen) entries() {
	types := []uint16{1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 16, 17, 18}
	for _, typ := range types {
		for _, big := range []bool{false, true} {
			vw := uint64(4)
			if big {
				vw = 8
			}
			sz := map[uint16]uint64{1: 1, 2: 1, 6: 1, 7: 1, 3: 2, 8: 2, 4: 4, 9: 4, 11: 4, 13: 4}[typ]
			if sz == 0 {
				sz = 8
			}
			for kind, cv := range map[string][2]uint64{"count": {1 << 30, 0}, "offset": {vw/sz + 1, 1 << 20}} {
				g.emit(fmt.Sprintf("oob-skip-t%d-b%d-%s", typ, b2i(big), kind), oob(big, 700, typ, cv[0], cv[1]))
			}
			cs := []uint64{1<<32 - 1, 1<<32 - 2}
			offs := []uint64{1<<32 - 1, 1<<32 - 8, 1 << 20}
			if big {
				cs = append(cs, 1<<63, 1<<63-1, 1<<64-1, 1<<64-2)
				offs = append(offs, 1<<63-8, 1<<63, 1<<64-8, 1<<64-1)
			}
			for _, c := range cs {
				for _, o := range offs {
					for _, tg := range []uint16{274, 700} {
						g.emit(fmt.Sprintf("oob-ovf-t%d-b%d-c%x-o%x-g%d", typ, b2i(big), c, o, tg), oob(big, tg, typ, c, o))
					}
				}
			}
		}
	}
	bad := [][4]uint64{{274, 12, 1, 0}, {274, 8, 1, 0}, {274, 2, 1, 0}, {274, 3, 2, 0}, {274, 3, 0, 0}}
	for _, big := range []bool{false, true} {
		bi := b2i(big)
		g.base(fmt.Sprintf("oob-orient-b%d", bi), oob(big, 274, 3, 1, 6, [4]uint64{274, 3, 1, 8}, [4]uint64{274, 12, 1, 0}, [4]uint64{274, 3, 1 << 30, 0}))
		g.base(fmt.Sprintf("oob-xmp-b%d", bi), oob(big, 700, 1, 4, 0x34333231, [4]uint64{700, 1, 1 << 30, 0}, [4]uint64{700, 3, 1, 1}, [4]uint64{700, 1, 9, 0}))
		for i, x := range bad {
			g.emit(fmt.Sprintf("oob-order-b%d-%d-last", bi, i), oob(big, 274, 3, 1, 6, x))
			g.emit(fmt.Sprintf("oob-order-b%d-%d-first", bi, i), oob(big, uint16(x[0]), uint16(x[1]), x[2], x[3], [4]uint64{274, 3, 1, 6}))
		}
		g.emit(fmt.Sprintf("oob-lone-sshort-b%d", bi), oob(big, 274, 8, 1, 6))
		for _, cv := range [][2]uint64{{1 << 30, 0}, {100, 1 << 20}} {
			for _, tg := range []uint16{338, 339} {
				g.emit(fmt.Sprintf("oob-needed-b%d-g%d-%x", bi, tg, cv[1]), oob(big, tg, 3, cv[0], cv[1]))
			}
		}
		for _, tg := range []uint16{320, 347, 292, 293, 324, 325, 266} {
			for _, cv := range [][2]uint64{{1 << 30, 0}, {3, 1 << 20}} {
				g.emit(fmt.Sprintf("oob-unread-b%d-g%d-%x", bi, tg, cv[1]), oob(big, tg, 4, cv[0], cv[1]))
			}
		}
		g.emit(fmt.Sprintf("oob-skipped-orient-b%d", bi), oob(big, 274, 3, 1<<30, 0))
		g.emit(fmt.Sprintf("oob-jpegtables-b%d", bi), oob(big, 347, 7, 1<<30, 0, [4]uint64{259, 3, 1, 7}))
		g.base(fmt.Sprintf("oob-xmp-short-b%d", bi), oob(big, 700, 7, 2, 0x4241))
		g.resolution(big)
	}
	g.emit("oob-dup-orient-last-wins", oob(false, 274, 3, 1, 6, [4]uint64{274, 3, 1 << 30, 0}))
	g.emit("oob-skipped-once", oob(false, 700, 3, 1<<30, 0, [4]uint64{700, 3, 1 << 30, 0}))
	g.emit("oob-tiled-ignores-rps", oob(false, 278, 4, 1<<30, 0, [4]uint64{322, 4, 1, 1}, [4]uint64{323, 4, 1, 1}, [4]uint64{324, 4, 1, 8}, [4]uint64{325, 4, 1, 1}))
	g.emit("oob-bad-then-valid", oob(false, 274, 3, 1<<30, 0, [4]uint64{274, 3, 1, 6}))
}

// resolution is TestResolutionEntries.
func (g *gen) resolution(big bool) {
	le := binary.LittleEndian
	rat := func(n, d uint32) []byte { return le.AppendUint32(le.AppendUint32(nil, n), d) }
	build := func(x, y []byte, more ...[4]uint64) []byte {
		val := func(r []byte, off uint64) uint64 {
			if big {
				return le.Uint64(r)
			}
			return off
		}
		b := oob(big, 282, 5, 1, 0, append([][4]uint64{{283, 5, 1, 0}, {296, 3, 1, 3}}, more...)...)
		b = oob(big, 282, 5, 1, val(x, uint64(len(b))), append([][4]uint64{{283, 5, 1, val(y, uint64(len(b))+8)}, {296, 3, 1, 3}}, more...)...)
		if !big {
			b = append(append(b, x...), y...)
		}
		return b
	}
	bi := b2i(big)
	g.base(fmt.Sprintf("res-ok-b%d", bi), build(rat(300, 1), rat(7, 2)))
	g.emit(fmt.Sprintf("res-zero-den-b%d", bi), build(rat(300, 0), rat(7, 2)))
	g.emit(fmt.Sprintf("res-oob-b%d", bi), oob(big, 282, 5, 1<<62, 1<<62, [4]uint64{296, 3, 1 << 30, 0}))
}

func (g *gen) testdata(dir string) error {
	var all []string
	for _, sub := range []string{"golden", "synthetic"} {
		m, err := filepath.Glob(filepath.Join(dir, sub, "*"))
		if err != nil {
			return err
		}
		all = append(all, m...)
	}
	sort.Strings(all)
	tifs := 0
	for _, p := range all {
		b, err := os.ReadFile(p)
		if err != nil {
			return err
		}
		n := filepath.Base(p)
		switch filepath.Ext(n) {
		case ".tif":
			tifs++
			// the same TIFFs in every mode, mutations for about half
			if tifs%2 == 0 || strings.HasPrefix(n, "bw") {
				g.base("td-"+n, b)
			} else {
				g.emit("td-"+n, b)
			}
		case ".jpg":
			j := strings.TrimSuffix(n, ".jpg")
			w, h, spp, err := sof(b)
			if err != nil {
				return fmt.Errorf("%s: %w", n, err)
			}
			// Photometric 6 for 3 samples: the stream is YCbCr; tiff-rs reads JPEG under Photometric
			// RGB as RGB as stored (docs/go-differences.md).
			p := fixture.Page{W: w, H: h, SPP: spp, BPS: 8, Photometric: photometricOf(spp, 7), Compression: 7, RowsPerStrip: h, Chunks: [][]byte{b}}
			g.base("td-jpg-"+j, fx(false, false, p))
			if strings.HasPrefix(j, "jpeg_") {
				// TestJPEGAnyFrameBands. Photometric 6: the stream is YCbCr; tiff-rs reads JPEG
				// under Photometric RGB as RGB as stored (docs/go-differences.md).
				p = fixture.Page{W: 16, H: 64, SPP: 3, BPS: 8, Photometric: 6, Compression: 7, RowsPerStrip: 64, Chunks: [][]byte{b}}
				g.emit("td-jpgany-"+j, fx(false, false, p))
			}
			if j == "cmyk_jpeg_plain" {
				// TestJPEGCMYKCorrupt
				for i, c := range [][]byte{b[:len(b)/2], b[:len(b)-200], append(b[:len(b)/2:len(b)/2], 0xff, 0xd9)} {
					p := fixture.Page{W: 70, H: 16, SPP: 4, BPS: 8, Photometric: 5, Compression: 7, RowsPerStrip: 16, Chunks: [][]byte{c}}
					g.emit(fmt.Sprintf("td-cmyk-corrupt-%d", i), fx(false, false, p))
				}
			}
		}
	}
	if tifs < 30 {
		return fmt.Errorf("only %d tifs in %s", tifs, dir)
	}
	return g.fuzzSeeds(filepath.Join(dir, "fuzz", "FuzzTIFF"))
}

// fuzzSeeds converts the Go fuzz corpus entries ("go test fuzz v1", one []byte("...")).
func (g *gen) fuzzSeeds(dir string) error {
	m, err := filepath.Glob(filepath.Join(dir, "*"))
	if err != nil {
		return err
	}
	for _, p := range m {
		t, err := os.ReadFile(p)
		if err != nil {
			return err
		}
		lines := strings.Split(strings.TrimSpace(string(t)), "\n")
		if len(lines) != 2 || !strings.HasPrefix(lines[1], "[]byte(") || !strings.HasSuffix(lines[1], ")") {
			return fmt.Errorf("%s: not a []byte corpus entry", p)
		}
		s, err := strconv.Unquote(strings.TrimSuffix(strings.TrimPrefix(lines[1], "[]byte("), ")"))
		if err != nil {
			return fmt.Errorf("%s: %w", p, err)
		}
		g.base("seed-"+filepath.Base(p), []byte(s))
	}
	return nil
}

// sof reads the frame size and component count from a JPEG's first SOFn marker.
func sof(b []byte) (w, h, spp int, err error) {
	for i := 2; i+9 < len(b); {
		if b[i] != 0xff {
			i++
			continue
		}
		m := b[i+1]
		if m >= 0xc0 && m <= 0xcf && m != 0xc4 && m != 0xc8 && m != 0xcc {
			return int(b[i+7])<<8 | int(b[i+8]), int(b[i+5])<<8 | int(b[i+6]), int(b[i+9]), nil
		}
		if m == 0xff || m == 0 || m == 0x01 || m >= 0xd0 && m <= 0xd8 {
			i++
			continue
		}
		i += 2 + (int(b[i+2])<<8 | int(b[i+3]))
	}
	return 0, 0, 0, errors.New("no SOF marker")
}
