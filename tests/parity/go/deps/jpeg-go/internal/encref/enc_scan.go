package encref

import (
	"bytes"
	"context"
)

// EncPlane is one component's samples, pre-block-padding. W,H are the true
// (unpadded) sample dimensions; reads past them replicate the edge.
type EncPlane struct {
	Pix    []byte
	Stride int
	W, H   int
}

func (p *EncPlane) at(x, y int) int32 {
	if x >= p.W {
		x = p.W - 1
	}
	if y >= p.H {
		y = p.H - 1
	}
	return int32(p.Pix[y*p.Stride+x])
}

// EncComponent is one scan component: its samples, MCU sampling factors and
// entropy tables.
type EncComponent struct {
	Plane        EncPlane
	HSamp, VSamp int
	Quant        *[64]int32
	DC, AC       *[256]HuffCode
}

// GrowBytes returns buf resized to length n, reusing its capacity when
// possible so callers can avoid a fresh allocation per call.
func GrowBytes(buf []byte, n int) []byte {
	if cap(buf) >= n {
		return buf[:n]
	}
	return make([]byte, n)
}

// blockPix returns block (bx, by)'s rows in place, or false for an
// edge block. It stays inlinable.
func blockPix(p *EncPlane, bx, by int) ([]byte, bool) {
	x, y := bx*8, by*8
	if x+8 <= p.W && y+8 <= p.H {
		return p.Pix[y*p.Stride+x:], true
	}
	return nil, false
}

// edgeBlock copies block (bx, by) into tmp so reads past W, H
// replicate the edge.
func edgeBlock(p *EncPlane, bx, by int, tmp *[64]byte) ([]byte, int) {
	x, y := bx*8, by*8
	for r := 0; r < 8; r++ {
		for c := 0; c < 8; c++ {
			tmp[r*8+c] = byte(p.at(x+c, y+r))
		}
	}
	return tmp[:], 8
}

// maxBlockBytes bounds one coded block, stuffing included
// (jchuff.c BUFSIZE, DCTSIZE2*8).
const maxBlockBytes = 64 * 8

func mcuBlocks(comps []EncComponent) int {
	n := 0
	for _, c := range comps {
		n += c.HSamp * c.VSamp
	}
	return n
}

// WriteScan entropy-codes comps into buf in MCU order (jccoefct.c). Blocks
// past a component's width/height in blocks are dummy: AC=0, DC copies the
// previous emitted block's DC (jccoefct.c:170-204). ctx is polled once per
// MCU row.
func WriteScan(ctx context.Context, buf *bytes.Buffer, w, h int, comps []EncComponent) error {
	hMax, vMax := 1, 1
	for _, c := range comps {
		if c.HSamp > hMax {
			hMax = c.HSamp
		}
		if c.VSamp > vMax {
			vMax = c.VSamp
		}
	}
	mcuX, mcuY := ceilDiv(w, 8*hMax), ceilDiv(h, 8*vMax)
	rowBound := mcuX * mcuBlocks(comps) * maxBlockBytes

	bw := bitWriter{buf: buf, free: 64}
	lastDC := make([]int32, len(comps))
	var blk [64]int16
	var edge [64]byte
	var divs [4]fdctDivisors
	var wBlocks, hBlocks [4]int
	for ci := range comps {
		divs[ci].set(comps[ci].Quant)
		wBlocks[ci], hBlocks[ci] = ceilDiv(comps[ci].Plane.W, 8), ceilDiv(comps[ci].Plane.H, 8)
	}

	for my := 0; my < mcuY; my++ {
		if err := ctx.Err(); err != nil {
			return err
		}
		bw.reserve(rowBound)
		for mx := 0; mx < mcuX; mx++ {
			for ci := range comps {
				c := &comps[ci]
				widthInBlocks, heightInBlocks := wBlocks[ci], hBlocks[ci]
				for dy := 0; dy < c.VSamp; dy++ {
					for dx := 0; dx < c.HSamp; dx++ {
						by, bx := my*c.VSamp+dy, mx*c.HSamp+dx
						if by >= heightInBlocks || bx >= widthInBlocks {
							dc := blk[0] // previous block's DC
							blk = [64]int16{}
							blk[0] = dc
						} else {
							pix, ok := blockPix(&c.Plane, bx, by)
							stride := c.Plane.Stride
							if !ok {
								pix, stride = edgeBlock(&c.Plane, bx, by, &edge)
							}
							fdctQuantRowsGo(pix, stride, c.Quant, &blk)
						}
						writeBlockGo(&bw, &blk, &lastDC[ci], c.DC, c.AC)
					}
				}
			}
		}
	}
	bw.flush()
	return nil
}

// WriteUniformScan entropy-codes a scan whose every block is the same
// DC-only coefficient per component (T14 spec): a uniform image's real and
// dummy blocks all reduce to that component's flat-block DC, so no plane
// read, FDCT or quantisation is needed per block. dc is indexed like comps.
func WriteUniformScan(ctx context.Context, buf *bytes.Buffer, w, h int, comps []EncComponent, dc []int32) error {
	hMax, vMax := 1, 1
	for _, c := range comps {
		if c.HSamp > hMax {
			hMax = c.HSamp
		}
		if c.VSamp > vMax {
			vMax = c.VSamp
		}
	}
	mcuX, mcuY := ceilDiv(w, 8*hMax), ceilDiv(h, 8*vMax)
	rowBound := mcuX * mcuBlocks(comps) * maxBlockBytes

	bw := bitWriter{buf: buf, free: 64}
	lastDC := make([]int32, len(comps))
	var blk [64]int16

	for my := 0; my < mcuY; my++ {
		if err := ctx.Err(); err != nil {
			return err
		}
		bw.reserve(rowBound)
		for mx := 0; mx < mcuX; mx++ {
			for ci := range comps {
				c := &comps[ci]
				blk = [64]int16{}
				blk[0] = int16(dc[ci])
				for n := 0; n < c.VSamp*c.HSamp; n++ {
					writeBlockGo(&bw, &blk, &lastDC[ci], c.DC, c.AC)
				}
			}
		}
	}
	bw.flush()
	return nil
}
