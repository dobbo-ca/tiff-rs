package codec

import (
	"bytes"
	"context"
	"encoding/binary"
)

// EncPlane is one component's samples, pre-block-padding. W,H are the true
// (unpadded) sample dimensions; reads past them replicate the edge.
type EncPlane struct {
	Pix    []byte
	Stride int
	W, H   int
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
	n := min(8, p.W-x)
	for r := 0; r < 8; r++ {
		row := p.Pix[min(y+r, p.H-1)*p.Stride+x:][:n]
		d := tmp[r*8 : r*8+8]
		copy(d, row)
		for c := n; c < 8; c++ {
			d[c] = row[n-1]
		}
	}
	return tmp[:], 8
}

// flatBlock: all 64 samples equal.
func flatBlock(pix []byte, stride int) (byte, bool) {
	_ = pix[7*stride+7]
	v := binary.LittleEndian.Uint64(pix)
	if v != uint64(pix[0])*0x0101010101010101 {
		return 0, false
	}
	for r := 1; r < 8; r++ {
		if binary.LittleEndian.Uint64(pix[r*stride:]) != v {
			return 0, false
		}
	}
	return pix[0], true
}

// flatDC: FDCT DC is 64(v-128).
func flatDC(v byte, q int32) int16 {
	return int16(quantizeIslow((int32(v)-128)*64, q))
}

// n0 holds the last block's DC.
const n0 = maxMCUBlocks

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

// Scan codes MCU rows in order.
type Scan struct {
	bw               bitWriter
	comps            []EncComponent
	lastDC           [4]int32
	divs             [4]fdctDivisors
	wBlocks, hBlocks [4]int
	mcuX, mcuY       int
	rowBound         int
	blk              [maxMCUBlocks + 1][64]int16
	dcOnly           [maxMCUBlocks]bool
	edge             [64]byte
	h2v2             bool                // Y 2x2, Cb and Cr 1x1
	pdivs            [2]fdctPairDivisors // Y|Y, Cb|Cr
}

// Start begins comps' scan into buf.
func (s *Scan) Start(buf *bytes.Buffer, w, h int, comps []EncComponent) {
	hMax, vMax := 1, 1
	for _, c := range comps {
		hMax, vMax = max(hMax, c.HSamp), max(vMax, c.VSamp)
	}
	s.comps = comps
	s.h2v2 = len(comps) == 3 && comps[0].HSamp == 2 && comps[0].VSamp == 2 &&
		comps[1].HSamp == 1 && comps[1].VSamp == 1 && comps[2].HSamp == 1 && comps[2].VSamp == 1
	s.mcuX, s.mcuY = ceilDiv(w, 8*hMax), ceilDiv(h, 8*vMax)
	s.rowBound = s.mcuX * mcuBlocks(comps) * maxBlockBytes
	s.bw = bitWriter{buf: buf, free: 64}
	s.lastDC = [4]int32{}
	s.blk[n0][0] = 0
	for ci := range comps {
		s.divs[ci].set(comps[ci].Quant)
		s.wBlocks[ci], s.hBlocks[ci] = ceilDiv(comps[ci].Plane.W, 8), ceilDiv(comps[ci].Plane.H, 8)
	}
	if s.h2v2 && fdctPairSIMD {
		s.setPairDivs()
	}
}

// setPairDivs builds the pair kernel's tables from divs.
func (s *Scan) setPairDivs() {
	s.pdivs[0].set(&s.divs[0], &s.divs[0])
	s.pdivs[1].set(&s.divs[1], &s.divs[2])
}

// MCURows is the MCU row count.
func (s *Scan) MCURows() int { return s.mcuY }

// Row codes MCU row my (jccoefct.c).
func (s *Scan) Row(my int, planes []EncPlane) {
	s.bw.reserve(s.rowBound)
	inner := s.interiorMCUs(planes)
	for mx := 0; mx < s.mcuX; mx++ {
		// FDCT whole MCU, then code it.
		n, dc, comps := 0, s.blk[n0][0], s.comps
		if mx < inner {
			dc, comps = s.fdctH2V2(mx, planes), nil
		}
		for ci := range comps {
			c, p := &s.comps[ci], &planes[ci]
			for dy := 0; dy < c.VSamp; dy++ {
				for dx := 0; dx < c.HSamp; dx++ {
					by, bx := my*c.VSamp+dy, mx*c.HSamp+dx
					s.dcOnly[n] = true
					if by >= s.hBlocks[ci] || bx >= s.wBlocks[ci] { // dummy: previous DC
						s.blk[n][0] = dc
						n++
						continue
					}
					pix, ok := blockPix(p, bx, dy)
					stride := p.Stride
					if !ok {
						pix, stride = edgeBlock(p, bx, dy, &s.edge)
					}
					if v, ok := flatBlock(pix, stride); ok {
						s.blk[n][0] = flatDC(v, c.Quant[0])
					} else {
						fdctQuantRows(pix, stride, c.Quant, &s.divs[ci], &s.blk[n])
						s.dcOnly[n] = false
					}
					dc = s.blk[n][0]
					n++
				}
			}
		}
		n = 0
		for ci := range s.comps {
			c := &s.comps[ci]
			for k := 0; k < c.VSamp*c.HSamp; k++ {
				if s.dcOnly[n] {
					writeDCOnly(&s.bw, int32(s.blk[n][0]), &s.lastDC[ci], c.DC, c.AC)
				} else {
					writeBlock(&s.bw, &s.blk[n], &s.lastDC[ci], c.DC, c.AC)
				}
				n++
			}
		}
		s.blk[n0][0] = dc
	}
}

// interiorMCUs: MCUs below it have no edge or dummy block (4:2:0).
func (s *Scan) interiorMCUs(p []EncPlane) int {
	if !s.h2v2 || p[0].H < 16 || p[1].H < 8 || p[2].H < 8 {
		return 0
	}
	return min(p[0].W/16, p[1].W/8, p[2].W/8)
}

// fdctH2V2 is Row's FDCT loop for an interior MCU; returns its last DC.
func (s *Scan) fdctH2V2(mx int, p []EncPlane) int16 {
	ys, o := p[0].Stride, 16*mx
	pix := [6][]byte{p[0].Pix[o:], p[0].Pix[o+8:], p[0].Pix[8*ys+o:], p[0].Pix[8*ys+o+8:], p[1].Pix[8*mx:], p[2].Pix[8*mx:]}
	stride := [6]int{ys, ys, ys, ys, p[1].Stride, p[2].Stride}
	var v [6]byte
	var flat [6]bool
	for n := range 6 {
		v[n], flat[n] = flatBlock(pix[n], stride[n])
	}
	for n := 0; n < 6; n++ {
		ci := max(0, n-3)
		c := &s.comps[ci]
		switch {
		case flat[n]:
			s.blk[n][0] = flatDC(v[n], c.Quant[0])
			s.dcOnly[n] = true
		case fdctPairSIMD && n%2 == 0 && !flat[n+1]: // Y0|Y1, Y2|Y3, Cb|Cr
			fdctQuantPair(pix[n], pix[n+1], stride[n], stride[n+1], &s.pdivs[n/4], &s.blk[n], &s.blk[n+1])
			s.dcOnly[n], s.dcOnly[n+1] = false, false
			n++
		default:
			fdctQuantRows(pix[n], stride[n], c.Quant, &s.divs[ci], &s.blk[n])
			s.dcOnly[n] = false
		}
	}
	return s.blk[5][0]
}

// Finish flushes the scan's last bits.
func (s *Scan) Finish() { s.bw.flush() }

// from is p from row y.
func (p EncPlane) from(y int) EncPlane {
	return EncPlane{Pix: p.Pix[y*p.Stride:], Stride: p.Stride, W: p.W, H: p.H - y}
}

// WriteScan codes whole planes; polls ctx.
func WriteScan(ctx context.Context, buf *bytes.Buffer, w, h int, comps []EncComponent) error {
	var s Scan
	s.Start(buf, w, h, comps)
	var planes [4]EncPlane
	for my := 0; my < s.mcuY; my++ {
		if err := ctx.Err(); err != nil {
			return err
		}
		for ci := range comps {
			planes[ci] = comps[ci].Plane.from(my * 8 * comps[ci].VSamp)
		}
		s.Row(my, planes[:len(comps)])
	}
	s.Finish()
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

	for my := 0; my < mcuY; my++ {
		if err := ctx.Err(); err != nil {
			return err
		}
		bw.reserve(rowBound)
		for mx := 0; mx < mcuX; mx++ {
			for ci := range comps {
				c := &comps[ci]
				for n := 0; n < c.VSamp*c.HSamp; n++ {
					writeDCOnly(&bw, dc[ci], &lastDC[ci], c.DC, c.AC)
				}
			}
		}
	}
	bw.flush()
	return nil
}
