package codec

import (
	"context"
	"errors"
)

// Coefs is what a frame's scans leave besides the blocks themselves: the
// progressive state IDCTRows needs to match a whole-frame decode.
type Coefs struct {
	ps      progState
	scanned [3]bool
	prog    bool
	smooth  bool
}

// DecodeCoefs entropy-decodes every scan of f into planes' Blocks and does
// no IDCT. Every plane must have Coef set. Truncation is reported as
// DecodeScan reports it; a fatal marker after a single-scan frame's last
// row is not an error, as Decode keeps that image.
func DecodeCoefs(ctx context.Context, f *Frame, src []byte, planes []Plane) (Coefs, bool, error) {
	if !f.Progressive {
		truncated, _, _, err := DecodeScanBand(ctx, f, src, planes, nil)
		if errors.Is(err, ErrAfterScan) {
			err = nil // every row is decoded, as Decode keeps its image
		}
		return Coefs{}, truncated, err
	}
	truncated, _, _, ps, scanned, err := decodeProgScans(ctx, f, src, planes)
	if err != nil {
		return Coefs{}, truncated, err
	}
	return Coefs{ps: ps, scanned: scanned, prog: true, smooth: ps.smoothingOK(f, planes)}, truncated, nil
}

// IDCTRows writes component ci's block rows [by0, by1) of p (a plane
// DecodeCoefs filled) into pix, whose row 0 is block row by0, exactly as a
// whole-frame decode would. pix is first set to 128, the value of the
// blocks a decode never transforms.
func (c *Coefs) IDCTRows(f *Frame, p *Plane, ci, by0, by1 int, pix []byte) {
	bs := p.blockSize()
	pix = pix[:(by1-by0)*bs*p.Stride()]
	for i := range pix {
		pix[i] = 128
	}
	width, high := p.BlocksWide, p.BlocksHigh
	var sm *smoother
	if c.prog {
		if !c.scanned[ci] {
			return
		}
		width, high = p.RealWide, p.RealHigh
		if c.smooth {
			_, vmax := MaxSampling(f)
			s := newSmoother(f, &c.ps, ci, p, vmax)
			sm = &s
		}
	}
	for by := by0; by < min(by1, high); by++ {
		idctRow(p, sm, by, width, pix, by0)
	}
}
