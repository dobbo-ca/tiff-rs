package jpeg

import (
	"context"
	"errors"
	"image"
	"io"

	"github.com/dobbo-ca/jpeg-go/internal/codec"
	"github.com/dobbo-ca/jpeg-go/internal/ledger"
)

// rowReaderMaxNeed caps one RowReader's reservation
// (docs/design.md section 6). Limits.MaxBytes can only lower it; maxNeed,
// the cap of Decode and region decode, is unchanged.
const rowReaderMaxNeed = 1100 << 20

var errRowReaderClosed = errors.New("jpeg: RowReader is closed")

// RowReader hands out a frame's RGBA rows one MCU row at a time. It
// entropy-decodes every scan once, keeping only the int16 coefficients
// (128 bytes per block, padded to whole MCUs), and transforms, upsamples
// and colour-converts a band when Next asks for it, so its peak is the
// coefficients plus one band. Its bytes are reserved from the process
// limiter until Close.
type RowReader struct {
	d         *Decoder // fresh, never pooled, so Close lets the GC take it all
	cp        [3]codec.Plane
	coefs     codec.Coefs
	cfg       Config
	out       image.RGBA
	next      int
	mcuRows   int
	win       int // MCU rows per band: up to 3 when chroma needs vertical context
	truncated bool
}

// rowReaderNeed is the bytes a RowReader holds for a w x h frame: the
// blocks of every component, one band of planes and RGBA, and the chroma
// rows (docs/design.md section 6). ext is the page rounding only
// Limits.MaxBytes counts, as decodeNeed's. hy, vy are the first
// component's sampling; the other two are 1x1.
func rowReaderNeed(w, h, nc, hy, vy int, rgb bool) (need, ext int64) {
	hmax, vmax := hy, vy
	mcus, mcuRows := ceilDiv(w, hmax*8), ceilDiv(h, vmax*8)
	win := rowReaderWin(nc, vmax, mcuRows)
	need = 1 << 16
	for ci := 0; ci < nc; ci++ {
		ch, cv := 1, 1
		if ci == 0 {
			ch, cv = hy, vy
		}
		bw := int64(mcus * ch)
		need += bw * int64(mcuRows*cv) * 128 // coefficients
		need += bw * 8 * int64(win*cv) * 8   // one band of samples
		ext += 2 * 8192
	}
	need += 4 * int64(w) * int64(min(win*vmax*8, h)) // RGBA band
	ext += 8192
	if nc == 3 && !rgb {
		need += 2 * int64(w) // upsampled chroma rows
		ext += 2 * 8192
	}
	return need, ext
}

// rowReaderWin is the MCU rows one band spans: with 3 components and
// vmax 2 the chroma is upsampled vertically, and fancy upsampling reads
// one MCU row on each side (planBand's context rows).
func rowReaderWin(nc, vmax, mcuRows int) int {
	if nc == 3 && vmax == 2 {
		return min(3, mcuRows)
	}
	return 1
}

// RowReaderNeed is the bytes NewRowReader reserves for src's frame, read
// from its headers alone (tables as for NewRowReader). The frame must be
// one NewRowReader accepts, else it returns the parse error.
func RowReaderNeed(src, tables []byte) (int64, error) {
	var f codec.Frame
	if err := f.Parse(src, tables); err != nil {
		return 0, err
	}
	need, _ := rowReaderNeed(f.Width, f.Height, f.NComp, int(f.Comp[0].H), int(f.Comp[0].V), f.RGB)
	return need, nil
}

// MaxRowReaderNeed is the most RowReaderNeed can be for any frame of at
// most maxPixels (and at most 65535 on a side), over every width at the
// tallest height that fits, every sampling jpeg-go accepts, and 1 or 3
// components.
func MaxRowReaderNeed(maxPixels int64) int64 {
	var worst int64
	try := func(w, h, nc, hy, vy int) {
		if need, _ := rowReaderNeed(w, h, nc, hy, vy, false); need > worst {
			worst = need
		}
	}
	for w := 1; w <= 65535; w++ {
		h := int(min(65535, maxPixels/int64(w)))
		if h < 1 {
			continue
		}
		for hy := 1; hy <= 4; hy++ {
			for vy := 1; vy <= 4; vy++ {
				try(w, h, 1, hy, vy)
			}
		}
		for _, s := range [...][2]int{{1, 1}, {2, 1}, {2, 2}, {1, 2}, {4, 1}, {4, 2}} {
			try(w, h, 3, s[0], s[1])
		}
	}
	return worst
}

// NewRowReader parses src (tables, when non-nil, supplies DQT/DHT),
// reserves the frame's RowReaderNeed from the process limiter, waiting
// under ctx like Decode, and entropy-decodes every scan. Any frame decodes
// (progressive, multi-scan or single-scan), always to RGBA at Scale 1. l
// is checked from the SOF before any allocation: MaxPixels, and MaxBytes
// against the need, which is also capped at 1100 MiB whatever MaxBytes
// says. The caller must Close the reader, which gives the reservation
// back; Next closes it itself at the end or on error. Truncated data
// decodes like Decode's: see Truncated.
func NewRowReader(ctx context.Context, src, tables []byte, l Limits) (*RowReader, error) {
	r := &RowReader{d: new(Decoder)}
	d := r.d
	if err := d.ensureTables(tables); err != nil {
		return nil, err
	}
	d.frame = d.tablesFrame
	if err := d.frame.ParseFrame(src); err != nil {
		return nil, err
	}
	f := &d.frame
	if err := l.checkPixels(f.Width, f.Height); err != nil {
		return nil, err
	}
	if f.NComp == 3 && f.RGB && (f.Comp[1].H != f.Comp[0].H || f.Comp[1].V != f.Comp[0].V ||
		f.Comp[2].H != f.Comp[0].H || f.Comp[2].V != f.Comp[0].V) {
		return nil, ErrUnsupported // non-uniform RGB sampling, as Decode
	}
	r.cfg = configOf(f)
	need, ext := rowReaderNeed(f.Width, f.Height, f.NComp, int(f.Comp[0].H), int(f.Comp[0].V), f.RGB)
	if need > rowReaderMaxNeed {
		return nil, &LimitError{Name: "MaxBytes", Got: need, Max: rowReaderMaxNeed}
	}
	if err := l.checkBytes(need + ext); err != nil {
		return nil, err
	}
	if err := d.reserveWait(ctx, need); err != nil {
		return nil, err
	}
	if err := r.start(ctx, src, need+ext); err != nil {
		_ = r.Close()
		return nil, err
	}
	return r, nil
}

// start decodes the scans and sizes the band, charging a ledger of max
// bytes before each allocation.
func (r *RowReader) start(ctx context.Context, src []byte, max int64) error {
	d, f := r.d, &r.d.frame
	f.Ledger = ledger.New(max)
	for ci := 0; ci < f.NComp; ci++ {
		r.cp[ci].Coef = true
	}
	var err error
	r.coefs, r.truncated, err = codec.DecodeCoefs(ctx, f, src, r.cp[:f.NComp])
	if err != nil {
		return err
	}
	_, vmax := codec.MaxSampling(f)
	r.mcuRows = ceilDiv(f.Height, vmax*8)
	r.win = rowReaderWin(f.NComp, vmax, r.mcuRows)
	for ci := 0; ci < f.NComp; ci++ {
		bw := r.cp[ci].BlocksWide
		n := bw * 8 * r.win * int(f.Comp[ci].V) * 8
		if err := f.Ledger.Charge(int64(n)); err != nil {
			return err
		}
		d.planes[ci] = codec.Plane{BlockSize: 8, BlocksWide: bw, Pix: make([]byte, n)}
	}
	if err := d.ensureRGBA(image.Rect(0, 0, f.Width, min(r.win*vmax*8, f.Height))); err != nil {
		return err
	}
	if f.NComp == 3 && !f.RGB {
		if err := d.ensureChromaRows(f.Width); err != nil {
			return err
		}
	}
	f.Ledger = nil // the band buffers are made; Next allocates nothing
	return nil
}

// Config is the frame's shape.
func (r *RowReader) Config() Config { return r.cfg }

// Truncated reports that entropy data ran out, had a bad Huffman code or
// restart markers out of sequence: the rows are Decode's, which would
// return them with an error wrapping ErrTruncated.
func (r *RowReader) Truncated() bool { return r.truncated }

// Next returns the next MCU row's pixels, up to 8*vmax rows at full
// width, at their position in the frame (Bounds().Min.Y). The image is
// valid until the next call. It returns io.EOF after the last row, and
// closes the reader.
func (r *RowReader) Next() (*image.RGBA, error) {
	if r.d == nil {
		return nil, errRowReaderClosed
	}
	if r.next >= r.mcuRows {
		_ = r.Close()
		return nil, io.EOF
	}
	d, f := r.d, &r.d.frame
	hmax, vmax := codec.MaxSampling(f)
	m := r.next
	lo, hi := m, m
	if r.win > 1 {
		lo, hi = max(m-1, 0), min(m+1, r.mcuRows-1)
	}
	for ci := 0; ci < f.NComp; ci++ {
		v := int(f.Comp[ci].V)
		d.planes[ci].BlocksHigh = (hi - lo + 1) * v
		r.coefs.IDCTRows(f, &r.cp[ci], ci, lo*v, (hi+1)*v, d.planes[ci].Pix)
	}
	outStep := vmax * 8
	w, h := f.Width, f.Height
	img, err := d.rgba(f, f.NComp == 1, nil, image.Rect(0, lo*outStep, w, min((hi+1)*outStep, h)), nil, hmax, vmax, 8, 0, false)
	if err != nil {
		_ = r.Close()
		return nil, err
	}
	y0 := m * outStep
	rows := image.Rect(0, y0, w, min(y0+outStep, h))
	off := img.PixOffset(0, y0)
	r.out = image.RGBA{Pix: img.Pix[off : off+rows.Dy()*img.Stride], Stride: img.Stride, Rect: rows}
	r.next++
	return &r.out, nil
}

// Close gives the reservation back and drops the coefficients. It is
// idempotent and always returns nil.
func (r *RowReader) Close() error {
	if r.d != nil {
		r.d.releaseReservation()
		r.d, r.cp, r.out = nil, [3]codec.Plane{}, image.RGBA{}
	}
	return nil
}
