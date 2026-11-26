package jpeg

import (
	"context"
	"fmt"

	"github.com/dobbo-ca/jpeg-go/internal/codec"
)

// Coefficients is a frame's quantized DCT coefficients, libjpeg-turbo's
// jpeg_read_coefficients view. It lets a caller run its own IDCT and
// colour step.
type Coefficients struct {
	Config
	Comp []ComponentCoefficients // frame component order
}

// ComponentCoefficients holds one component's blocks, row-major and
// padded to whole MCUs: block (bx, by) is Blocks[by*BlocksWide+bx].
// Blocks are in natural order and not dequantized; Quant is the
// natural-order table they were coded with, or zero for a component
// no scan reached (turbo's NULL quant_table).
type ComponentCoefficients struct {
	H, V                   int // sampling factors
	BlocksWide, BlocksHigh int
	Blocks                 [][64]int16
	Quant                  [64]uint16
}

// DecodeCoefficients entropy-decodes src without IDCT, upsampling or
// colour conversion; tables, when non-nil, supplies DQT/DHT. The result
// aliases d's buffers until d's next call. Truncated entropy data gives
// the blocks libjpeg-turbo would, with an error wrapping ErrTruncated.
func (d *Decoder) DecodeCoefficients(ctx context.Context, src, tables []byte) (*Coefficients, error) {
	return d.DecodeCoefficientsLimits(ctx, src, tables, Limits{}, false)
}

// DecodeCoefficientsLimits is DecodeCoefficients bounded by l, checked from
// the SOF before any allocation (ErrLimit). reserved means the caller
// already reserved the bytes: the process limiter is skipped, as
// DecodeOptions.Reserved does; l and the 1 GiB cap still apply.
func (d *Decoder) DecodeCoefficientsLimits(ctx context.Context, src, tables []byte, l Limits, reserved bool) (*Coefficients, error) {
	if err := d.ensureTables(tables); err != nil {
		return nil, err
	}
	d.frame = d.tablesFrame
	if err := d.frame.ParseFrame(src); err != nil {
		return nil, err
	}
	f := &d.frame
	f.Ledger = l.Ledger()
	if err := l.checkPixels(f.Width, f.Height); err != nil {
		return nil, err
	}
	hmax, vmax := codec.MaxSampling(f)
	need := int64(1 << 16) // tables and frame state, as decodeNeed
	for ci := 0; ci < f.NComp; ci++ {
		w := ceilDiv(f.Width, hmax*8) * int(f.Comp[ci].H)
		h := ceilDiv(f.Height, vmax*8) * int(f.Comp[ci].V)
		need += int64(w) * int64(h) * 128
	}
	if err := l.checkBytes(need); err != nil {
		return nil, err
	}
	d.lastNeed = need
	if reserved {
		if need > maxNeed {
			return nil, fmt.Errorf("jpeg: decode needs %d bytes, over the 1 GiB cap; use Region", need)
		}
	} else if err := d.reserve(ctx, need); err != nil {
		return nil, err
	}
	defer d.releaseReservation()

	for ci := 0; ci < f.NComp; ci++ {
		d.planes[ci] = codec.Plane{Coef: true, Blocks: d.planes[ci].Blocks, Pix: d.planes[ci].Pix}
	}
	truncated, err := codec.DecodeScan(ctx, f, src, d.planes[:f.NComp])
	for ci := 0; ci < f.NComp; ci++ {
		d.planes[ci].Coef = false // Decode's planes must not keep coefficient mode
	}
	if err != nil {
		return nil, err
	}

	c := &d.coefs
	c.Config = configOf(f)
	c.Comp = c.Comp[:0]
	for ci := 0; ci < f.NComp; ci++ {
		p := &d.planes[ci]
		c.Comp = append(c.Comp, ComponentCoefficients{
			H: int(f.Comp[ci].H), V: int(f.Comp[ci].V),
			BlocksWide: p.BlocksWide, BlocksHigh: p.BlocksHigh,
			Blocks: p.Blocks, Quant: *p.Quant(),
		})
	}
	if truncated {
		return c, fmt.Errorf("%w: entropy data ran out, had a bad Huffman code, or restart markers out of sequence", ErrTruncated)
	}
	return c, nil
}
