package codec

import (
	"bytes"
	"context"
	"encoding/binary"
	"errors"
)

// Block is one 8x8 block of coefficients, in natural (raster) order,
// undequantized (turbo jdhuff.c writes zigzag input to natural output
// positions, jdhuff.c:618).
type Block = [64]int16

const blockBytes = 128 // one Block

// Plane is one frame component's output, padded to a whole number of
// MCUs (turbo jdcoefct.c:903-907). DecodeScan IDCTs each block into Pix as
// it is decoded (docs/design.md section 4 step 7), stride BlocksWide*8,
// unless the caller sets Skip (entropy decode only) or Coef (keep the raw
// coefficients in Blocks instead, jpeg_read_coefficients' view).
// RealWide/RealHigh give the unpadded block count.
type Plane struct {
	Skip, Coef bool

	// ReconIDCT replaces idct8Go for this plane's blocks (jpeg.DecodeOptions.
	// Recon.IDCT, T06). coef is dequantized, natural order.
	ReconIDCT func(dst []byte, stride int, coef *[64]int32)

	// BlockSize is this plane's IDCT output size (8, 4, 2 or 1), the
	// per-component DCT scaling of jpeg.DecodeOptions.Scale (T07,
	// jdmaster.c:290-305). Zero means 8 (Scale 1).
	BlockSize int

	Pix        []byte
	Blocks     []Block
	BlocksWide int
	BlocksHigh int
	RealWide   int
	RealHigh   int

	quant   [64]uint16
	latched bool // quant copied at first scan, turbo latch_quant_tables
}

func (p *Plane) at(bx, by int) *Block { return &p.Blocks[by*p.BlocksWide+bx] }

// Quant is the natural-order table latched at this plane's first scan.
func (p *Plane) Quant() *[64]uint16 { return &p.quant }

// blockSize is p.BlockSize, defaulting to 8 (Scale 1) when unset.
func (p *Plane) blockSize() int {
	if p.BlockSize == 0 {
		return 8
	}
	return p.BlockSize
}

// Stride is Pix's row stride: BlocksWide times the plane's IDCT block size.
func (p *Plane) Stride() int { return p.BlocksWide * p.blockSize() }

// idctBS runs the islow IDCT matching a component's scaled block size
// (T07: Scale 2/4/8 selects the reduced IDCTs from dct.go). A direct call
// per case, not a func variable, keeps blk off the heap: escape analysis
// can't see through an indirect call. mayAC false means the Huffman
// decoder stored no AC in coef, so the dcOnly scan is skipped.
func idctBS(bs int, coef *Block, mayAC bool, quant *[64]uint16, dst []byte, stride int) {
	if bs > 1 && (!mayAC || dcOnly(coef)) { // idct1Go is already DC only
		fillConst(bs, dst, stride, idctDCConst(coef[0], quant[0]))
		return
	}
	switch bs {
	case 4:
		idct4(coef, quant, dst, stride)
	case 2:
		idct2Go(coef, quant, dst, stride)
	case 1:
		idct1Go(coef, quant, dst, stride)
	default:
		idct8(coef, quant, dst, stride)
	}
}

// MaxSampling returns f's max horizontal/vertical sampling factors
// (docs/design.md section 4).
func MaxSampling(f *Frame) (int, int) {
	hmax, vmax := 1, 1
	for i := 0; i < f.NComp; i++ {
		if int(f.Comp[i].H) > hmax {
			hmax = int(f.Comp[i].H)
		}
		if int(f.Comp[i].V) > vmax {
			vmax = int(f.Comp[i].V)
		}
	}
	return hmax, vmax
}

// ComponentBlockSize is a component's IDCT output size given the frame's
// min_DCT_scaled_size (jdmaster.c's per-component doubling loop, T07):
// a component sampled below hmax/vmax gets a larger IDCT so its plane
// stays proportional to the full-size output, up to 8x8.
func ComponentBlockSize(h, v, hmax, vmax, minSize int) int {
	ssize := minSize
	for ssize < 8 && h*ssize*2 <= hmax*minSize && v*ssize*2 <= vmax*minSize {
		ssize *= 2
	}
	return ssize
}

// DownsampledDims is a component's real pixel width/height at block size
// bs (jdmaster.c's downsampled_width/height under IDCT scaling, T07).
func DownsampledDims(f *Frame, ci, hmax, vmax, bs int) (int, int) {
	c := f.Comp[ci]
	w := ceilDiv(f.Width*int(c.H)*bs, hmax*8)
	h := ceilDiv(f.Height*int(c.V)*bs, vmax*8)
	return w, h
}

// Preallocated so failures allocate nothing.
var (
	errNoQuantTable = errors.New("jpeg: quantization table was not defined")
	errNoHuffTable  = errors.New("jpeg: Huffman table was not defined")
	errEOIExpected  = errors.New("jpeg: another scan after one covering every component")
)

func ceilDiv(a, b int) int { return (a + b - 1) / b }

func clampInt(v, lo, hi int) int {
	if v < lo {
		return lo
	}
	if v > hi {
		return hi
	}
	return v
}

type scanComp struct {
	ci         int
	mcuW, mcuH int
}

// Band restricts decoding to MCU rows [First, Last] of the frame's
// max-sampling grid, growing planes to only that band instead of the whole
// frame (region decode, T08). When Cols is set, MCUs outside columns
// [ColFirst, ColLast] are not transformed. Without a DRI they are still
// entropy-decoded, for DC prediction; with a DRI, restart intervals
// holding no needed MCU are skipped (skipTo). Restart is
// DecodeOptions.Restart: the absolute src offset of each restart
// interval's first entropy byte (NDPI tag 65426); nil, or an offset not
// just past its RSTn, means scan the entropy stream for RSTn instead.
type Band struct {
	First, Last       int
	Cols              bool
	ColFirst, ColLast int
	Restart           []int
	CheckFrame        bool // entropy-decode every MCU, for whole-frame errors
}

// DecodeScan entropy-decodes f's scan(s), starting at f.ScanOffset (the
// first entropy byte f.Parse found), following turbo's baseline scan loop
// (jdhuff.c, jdinput.c per_scan_setup, jdcoefct.c consume_data). planes must
// have len(planes) == f.NComp; each Plane's Pix or Blocks buffer is grown
// (never shrunk, so a Decoder reusing planes makes no garbage) to this
// frame's component size. A frame may hold several non-interleaved
// sequential scans, one per component (docs/design.md section 1);
// DecodeScan follows the markers between them and returns whether data ran
// out: a scan's entropy data or the stream itself (turbo's fake EOI). A
// frame component that no scan covered decodes as zero blocks, which turbo
// does not warn about. It errors
// like turbo when a scan's quant or Huffman table is undefined, and on
// ctx cancellation, polled once per MCU row (docs/design.md section 4).
func DecodeScan(ctx context.Context, f *Frame, src []byte, planes []Plane) (bool, error) {
	if f.Progressive {
		truncated, _, _, err := decodeProgressive(ctx, f, src, planes)
		return truncated, err
	}
	truncated, _, _, err := decodeScan(ctx, f, src, planes, nil, 0)
	if a, ok := err.(afterScan); ok {
		err = a.err // jpeg_read_coefficients reads to EOI first
	}
	return truncated || f.Overran, err
}

// DecodeScanBand is DecodeScan restricted to band (T08); band == nil
// decodes every row, identical to DecodeScan. For the last scan, decoded
// counts the entropy bytes the Huffman bit reader read and scanned the
// bytes searched for markers, so a caller can check what a region reads.
func DecodeScanBand(ctx context.Context, f *Frame, src []byte, planes []Plane, band *Band) (truncated bool, decoded, scanned int, err error) {
	if f.Progressive {
		return decodeProgressive(ctx, f, src, planes)
	}
	truncated, decoded, scanned, err = decodeScan(ctx, f, src, planes, band, 0)
	return truncated || f.Overran, decoded, scanned, err
}

// DecodeScanN is DecodeScanBand with no band that may entropy-decode a
// baseline scan's restart intervals on up to workers goroutines
// in parallel; truncated and err are the same.
func DecodeScanN(ctx context.Context, f *Frame, src []byte, planes []Plane, workers int) (bool, error) {
	if f.Progressive {
		truncated, _, _, err := decodeProgressive(ctx, f, src, planes)
		return truncated, err
	}
	truncated, _, _, err := decodeScan(ctx, f, src, planes, nil, workers)
	return truncated || f.Overran, err
}

func decodeScan(ctx context.Context, f *Frame, src []byte, planes []Plane, band *Band, workers int) (bool, int, int, error) {
	hmax, vmax := MaxSampling(f)
	mcuRows := ceilDiv(f.Height, vmax*8)
	first, last := 0, mcuRows-1
	// Only a single scan covering every frame component (interleaved, or the
	// lone scan of a 1-component frame) shares one MCU-row grid; any other
	// multi-scan frame, including one whose first scan interleaves only
	// some components (e.g. "0 1: ...; 2: ...;"), is skipped (full, correct
	// decode) rather than banded on the first scan's grid alone (T08 fix:
	// f.ScanN == 1 alone missed a first scan that interleaves 2 of 3).
	banded := band != nil && (f.NComp == 1 || f.ScanN == f.NComp)
	multi := f.ScanN < f.NComp // has_multiple_scans: blocks persist
	if banded {
		first, last = band.First, band.Last
	}

	for ci := range planes {
		c := f.Comp[ci]
		w := ceilDiv(f.Width, hmax*8) * int(c.H)
		h := (last - first + 1) * int(c.V)
		p := &planes[ci]
		bs := p.blockSize()
		if p.Coef || multi && !p.Skip {
			if err := f.Ledger.Charge(int64(w) * int64(h) * blockBytes); err != nil {
				return false, 0, 0, err
			}
			if cap(p.Blocks) < w*h {
				p.Blocks = make([]Block, w*h)
			} else {
				p.Blocks = p.Blocks[:w*h]
				for i := range p.Blocks {
					p.Blocks[i] = Block{} // a skipped scan must leave this Plane at zero
				}
			}
		}
		if !p.Coef && !p.Skip {
			n := w * h * bs * bs
			if err := f.Ledger.Charge(int64(n)); err != nil {
				return false, 0, 0, err
			}
			if cap(p.Pix) < n {
				p.Pix = make([]byte, n)
			}
			p.Pix = p.Pix[:n]
		}
		p.BlocksWide, p.BlocksHigh = w, h
		p.RealWide = ceilDiv(f.Width*int(c.H), hmax*8)
		fullRealHigh := ceilDiv(f.Height*int(c.V), vmax*8)
		p.RealHigh = clampInt(fullRealHigh-first*int(c.V), 0, h)
		p.latched = false
		if !p.Skip && !p.Coef {
			// Real pixel extent scales with bs (jdmaster.c downsampled_width/
			// height), not RealWide*bs: rounding differs from the 8x8 block
			// count when the image size isn't a multiple of bs*hmax (T07).
			rw, rh := DownsampledDims(f, ci, hmax, vmax, bs)
			rh = clampInt(rh-first*int(c.V)*bs, 0, h*bs)
			fillPadding(p, rw, rh)
		}
	}
	f.installStdHuffTables()

	var starts []int
	if workers > 1 && band == nil && bandable(f, planes) {
		if err := f.Ledger.Charge(8 * int64(ceilDiv(scanMCUs(f, hmax, vmax), f.RestartInterval))); err != nil {
			return false, 0, 0, err
		}
		starts = restartStarts(src, f.ScanOffset, ceilDiv(scanMCUs(f, hmax, vmax), f.RestartInterval))
	}

	var scanned [3]bool
	var r bitReader
	offset := f.ScanOffset
	truncated := false
	for {
		// turbo's JWRN_NOT_SEQUENTIAL, then decodes as usual
		if f.ScanSs != 0 || f.ScanSe != 63 || f.ScanAh != 0 || f.ScanAl != 0 {
			truncated = true
		}
		var stopped bool
		var err error
		ok := false
		if starts != nil {
			r, ok, err = decodeBands(ctx, f, src, hmax, vmax, planes, starts, workers)
			starts = nil
		}
		if !ok && err == nil {
			r = bitReader{data: src, pos: offset}
			stopped, err = decodeOneScan(ctx, f, src, hmax, vmax, planes, &r, banded, multi, first, last, band, span{})
		}
		if err != nil {
			return true, 0, 0, err
		}
		for k := 0; k < f.ScanN; k++ {
			scanned[f.ScanComp[k]] = true
		}
		if stopped {
			// Region done early: report data lost so far.
			truncated = truncated || r.eof || r.lost
			break
		}
		if !r.marker {
			// Last MCU fit in buffered bits (jdmarker.c next_marker).
			c, ok := nextMarker(&r)
			r.marker, r.markerByte = ok, c
		}
		truncated = truncated || r.insufficient || r.eof || r.lost

		// A further SOS means more non-interleaved scans follow; anything
		// else (EOI, or no marker found) ends the frame.
		if !r.marker || r.markerByte == markerEOI {
			break
		}
		sos, next, err := f.consumeFromMarker(src, r.markerByte, r.pos, true)
		if err != nil {
			// Not just end of frame: turbo's next_marker ERREXITs fatally
			// on a marker it doesn't recognize (jdmarker.c:742).
			if !multi {
				err = afterScan{err}
			}
			return truncated, r.decoded, r.scanned, err
		}
		if !sos {
			break
		}
		if !multi {
			return truncated, r.decoded, r.scanned, afterScan{errEOIExpected}
		}
		offset = next
	}
	for ci := 0; ci < f.NComp; ci++ {
		if scanned[ci] {
			continue
		}
		if p := &planes[ci]; !p.Skip && !p.Coef {
			for i := range p.Pix {
				p.Pix[i] = 128 // IDCT of an all-zero block
			}
		}
	}
	return truncated, r.decoded, r.scanned, nil
}

// fillPadding sets the pixels past the real (unpadded) rw x rh extent to
// 128, the IDCT of the zero blocks turbo keeps there; a non-interleaved
// scan never visits them.
func fillPadding(p *Plane, rw, rh int) {
	bs := p.blockSize()
	stride := p.Stride()
	for y := 0; y < p.BlocksHigh*bs; y++ {
		from := rw
		if y >= rh {
			from = 0
		}
		row := p.Pix[y*stride+from : (y+1)*stride]
		for i := range row {
			row[i] = 128
		}
	}
}

// decodeOneScan decodes the scan currently described by f.ScanN/ScanComp
// (etc.) from r (positioned at its first entropy byte by the caller),
// writing into planes indexed by frame component. When banded, first/last
// are the MCU-row band in this scan's own grid (interleaved rows, or
// blocks for a lone non-interleaved component): MCUs outside the band are
// entropy decoded but not transformed (DC prediction must stay correct),
// or skipped a restart interval at a time with a DRI, and decoding stops
// after the band's last needed MCU (T08). stopped reports that it ended
// before the scan's last MCU. banded is false for every non-interleaved
// scan of a multi-scan frame (T08's DecodeScan gate), since such a scan's
// own grid does not share a single MCU-row unit with the others. A
// non-nil error means ctx was cancelled mid-scan or a table was missing;
// the caller inspects r for markers and truncation.
func decodeOneScan(ctx context.Context, f *Frame, src []byte, hmax, vmax int, planes []Plane, r *bitReader, banded, multi bool, first, last int, band *Band, sp span) (stopped bool, err error) {
	interleaved := f.ScanN > 1
	var comps [4]scanComp
	for k := 0; k < f.ScanN; k++ {
		ci := f.ScanComp[k]
		if interleaved {
			comps[k] = scanComp{ci, int(f.Comp[ci].H), int(f.Comp[ci].V)}
		} else {
			comps[k] = scanComp{ci, 1, 1}
		}
	}

	var mcusPerRow, mcuRows int
	if interleaved {
		mcusPerRow = ceilDiv(f.Width, hmax*8)
		mcuRows = ceilDiv(f.Height, vmax*8)
	} else {
		c := f.Comp[comps[0].ci]
		mcusPerRow = ceilDiv(f.Width*int(c.H), hmax*8)
		mcuRows = ceilDiv(f.Height*int(c.V), vmax*8)
	}

	// turbo start_input_pass: latch quant, then Huffman tables.
	if err := latchQuant(f, planes); err != nil {
		return false, err
	}
	var dc, ac [4]derivedTable
	for k := 0; k < f.ScanN; k++ {
		if !f.HasDC[f.ScanDC[k]] || !f.HasAC[f.ScanAC[k]] {
			return false, errNoHuffTable
		}
		// Same selector, same table.
		j := 0
		for j < k && f.ScanDC[j] != f.ScanDC[k] {
			j++
		}
		if j < k {
			dc[k] = dc[j]
		} else if err := dc[k].build(&f.DC[f.ScanDC[k]], true); err != nil {
			return false, err
		}
		j = 0
		for j < k && f.ScanAC[j] != f.ScanAC[k] {
			j++
		}
		if j < k {
			ac[k] = ac[j]
		} else if err := ac[k].build(&f.AC[f.ScanAC[k]], false); err != nil {
			return false, err
		}
	}

	// band.First/Last are frame-grid MCU-row indices (each spanning vmax
	// component block-rows), but a non-interleaved scan's own "MCU" is a
	// single component block (mcuH == 1 above): a single-component frame
	// (the only case reaching here banded, T08's ScanN==NComp gate above)
	// still has hmax == vmax == that component's own H/V, so converting by
	// its V factor maps the frame-grid band onto this scan's block rows
	// (T08 fix: grayscale streams sampled above 1x1 otherwise banded on the
	// wrong grid, off by a factor of V).
	colFirst, colLast := 0, -1 // colLast < 0: no column restriction
	if banded && band != nil && band.Cols {
		colFirst, colLast = band.ColFirst, band.ColLast
	}
	if banded && !interleaved {
		v := int(f.Comp[comps[0].ci].V)
		first, last = first*v, (last+1)*v-1
		if colLast >= 0 {
			h := int(f.Comp[comps[0].ci].H)
			colFirst, colLast = colFirst*h, (colLast+1)*h-1
		}
	}

	var lastDC [4]int32
	var scratch [maxMCUBlocks]Block
	var saved [maxMCUBlocks]Block // rollback copy, non-plain MCUs
	var mcu [maxMCUBlocks]*Block
	var mcuK [maxMCUBlocks]int
	var hasAC [maxMCUBlocks]bool // per block: an AC symbol was decoded

	// blockK is comps[k]'s component index per MCU block slot, in the same
	// order the loop below fills mcu/mcuK; the sampling pattern is the same
	// for every MCU, so a banded scan's skipped MCUs (T08's write flag, no
	// coefficient ever needed) can walk decodeMCUFastSkip's blocks by this
	// alone, without the write path's per-MCU plane pointer and scratch
	// zeroing (P5 attempt 1+2).
	var blockK [maxMCUBlocks]int
	var slots [maxMCUBlocks]mcuSlot
	// plain: every block is a scratch block (decompress_onepass), so the
	// MCU pointers are fixed for the scan.
	plain := !multi
	nBlk := 0
	for k := 0; k < f.ScanN; k++ {
		sc := comps[k]
		p := &planes[sc.ci]
		plain = plain && !p.Coef
		bs, stride := p.blockSize(), p.Stride()
		for yy := 0; yy < sc.mcuH; yy++ {
			for xx := 0; xx < sc.mcuW; xx++ {
				blockK[nBlk] = k
				slots[nBlk] = mcuSlot{p, yy*bs*stride + xx*bs, sc.mcuW * bs, sc.mcuH * bs * stride}
				nBlk++
			}
		}
	}
	if plain {
		for i := range nBlk {
			mcu[i], mcuK[i] = &scratch[i], blockK[i]
		}
	}

	ri := f.RestartInterval
	restartsToGo := ri
	nextRestart, interval := 0, 0 // interval counts restarts processed
	mk, mkReal := -1, false       // nextBail from r.pos, cached

	lastRow := mcuRows - 1
	if banded {
		lastRow = min(last, mcuRows-1)
	}
	end := (lastRow + 1) * mcusPerRow
	check := banded && band.CheckFrame
	if check {
		end = mcuRows * mcusPerRow
	} else if colLast >= 0 && colLast < mcusPerRow-1 && lastRow < mcuRows-1 {
		// turbo's crop decodes all of the last row, then the marker after it
		end = lastRow*mcusPerRow + colLast + 1
	}
	var restart []int
	if band != nil {
		restart = band.Restart
	}
	m0 := 0
	if sp.k1 > 0 { // lands as skipTo does
		m0, end = sp.k0*ri, min(sp.k1*ri, end)
		interval, nextRestart = sp.k0, sp.k0&7
	}

	polled, tried := -1, -1
	for m := m0; m < end; m++ {
		my, mx := m/mcusPerRow, m%mcusPerRow
		if my != polled {
			polled = my
			select {
			case <-ctx.Done():
				return false, ctx.Err()
			default:
			}
		}
		write := !banded || my >= first && my <= lastRow && (colLast < 0 || mx >= colFirst && mx <= colLast)
		if !write && ri != 0 && !check {
			next := max(my, first)*mcusPerRow + colFirst
			if my >= first && mx > colFirst {
				next += mcusPerRow // right of the band: next row
			}
			if t := next / ri; t > interval && next != tried && next < end {
				tried = next
				if k, ok := skipTo(src, r, restart, interval, t); ok {
					interval, nextRestart, restartsToGo = k, k&7, ri
					lastDC = [4]int32{}
					mk = -1
					m = k*ri - 1
					continue
				}
			}
		}
		if forceSkip {
			write = false
		}
		if ri != 0 && restartsToGo == 0 {
			processRestart(r, lastDC[:f.ScanN], &nextRestart)
			restartsToGo = ri
			interval++
		}
		n := nBlk
		if write && !plain {
			n = 0
			for k := 0; k < f.ScanN; k++ {
				sc := comps[k]
				p := &planes[sc.ci]
				for yy := 0; yy < sc.mcuH; yy++ {
					for xx := 0; xx < sc.mcuW; xx++ {
						blk := &scratch[n]
						if p.Coef || multi && !p.Skip {
							blk = p.at(mx*sc.mcuW+xx, (my-first)*sc.mcuH+yy)
						}
						if !multi && !p.Coef { // Coef planes were zeroed above
							*blk = Block{} // jdcoefct.c decompress_onepass zeroes each MCU
						}
						mcu[n], mcuK[n] = blk, k
						n++
					}
				}
			}
		}
		// turbo's 512-bytes-per-block margin keeps short tails exact.
		fast := !r.marker && len(src)-r.pos >= 512*n
		undo := false // try fast, roll back on bail
		if ri != 0 {
			fast = false
			if !r.marker && !noFastRST {
				if r.pos > mk {
					mk, mkReal = nextBail(src, r.pos)
				}
				// Never bails here, so equals slow.
				fast = mkReal && mk-r.pos >= 512*n
				// Real marker ahead: eof stays unset.
				// Last MCU almost always bails: go slow.
				undo = mkReal && !fast && restartsToGo != 1
			}
		}
		if write {
			if !r.insufficient { // else turbo leaves the MCU unchanged
				if undo {
					rs, dcs := *r, lastDC
					if !plain {
						for i := range n {
							saved[i] = *mcu[i]
						}
					}
					if !fastMCU(r, &dc, &ac, &lastDC, &mcu, &mcuK, n, &hasAC) {
						*r, lastDC = rs, dcs
						for i := range n {
							if plain {
								*mcu[i] = Block{} // scratch starts each MCU zeroed
							} else {
								*mcu[i] = saved[i]
							}
						}
						decodeMCUSlow(r, &dc, &ac, &lastDC, &mcu, &mcuK, n, &hasAC)
					}
				} else if !fast {
					decodeMCUSlow(r, &dc, &ac, &lastDC, &mcu, &mcuK, n, &hasAC)
				} else if !fastMCU(r, &dc, &ac, &lastDC, &mcu, &mcuK, n, &hasAC) {
					decodeMCUSlow(r, &dc, &ac, &lastDC, &mcu, &mcuK, n, &hasAC)
					for i := range hasAC {
						hasAC[i] = true // the failed attempt's stores stay
					}
				}
			}
		} else if !r.insufficient {
			// No block in this MCU is ever read: skip the plane-pointer
			// resolution and scratch zeroing above, and decode with no
			// coefficient store at all (P5 attempt 1+2).
			if undo {
				rs, dcs := *r, lastDC
				if !fastMCU(r, &dc, &ac, &lastDC, nil, &blockK, n, nil) {
					*r, lastDC = rs, dcs
					decodeMCUSlow(r, &dc, &ac, &lastDC, nil, &blockK, n, nil)
				}
			} else if !fast || !fastMCU(r, &dc, &ac, &lastDC, nil, &blockK, n, nil) {
				decodeMCUSlow(r, &dc, &ac, &lastDC, nil, &blockK, n, nil)
			}
		}
		if probeBits {
			probeState = append(probeState, probe{r.pos, r.bitsLeft, lastDC})
			if write {
				probeACs = append(probeACs, hasAC)
			}
		}
		if write {
			idctMCU(&slots, &mcu, &hasAC, n, mx, my-first, multi)
			if plain {
				// Restore zero blocks for the next MCU; an AC-free
				// block only holds DC.
				for i := range scratch[:n] {
					if hasAC[i] {
						scratch[i] = Block{}
					} else {
						scratch[i][0] = 0
					}
				}
			}
		}
		restartsToGo--
	}
	stopped = end < mcuRows*mcusPerRow
	if stopped && ri != 0 && (interval+1)*ri < mcuRows*mcusPerRow {
		// Turbo reads this RSTn next; a resync there warns.
		s, want := *r, nextRestart
		if readRestartMarker(&s, &want) {
			r.lost = true
		}
		r.scanned = s.scanned
	}
	return stopped, nil
}

// mcuSlot is one MCU block's plane and offsets, fixed per scan.
type mcuSlot struct {
	p                *Plane
	off              int // from the MCU's top-left sample
	colStep, rowStep int // per MCU column and row
}

// idctMCU transforms MCU (mx, my)'s decoded blocks into their planes.
// Stale hasAC flags are safe: onepass blocks start zeroed.
func idctMCU(slots *[maxMCUBlocks]mcuSlot, mcu *[maxMCUBlocks]*Block, hasAC *[maxMCUBlocks]bool, n, mx, my int, multi bool) {
	for i := range slots[:n] {
		s := &slots[i]
		p := s.p
		if p.Skip || p.Coef {
			continue
		}
		dst := p.Pix[my*s.rowStep+mx*s.colStep+s.off:]
		if p.ReconIDCT != nil {
			var deq [64]int32
			for j, c := range mcu[i] {
				deq[j] = int32(dequantize(c, p.quant[j]))
			}
			p.ReconIDCT(dst, p.Stride(), &deq)
			continue
		}
		idctBS(p.blockSize(), mcu[i], hasAC[i] || multi, &p.quant, dst, p.Stride())
	}
}

// skipTo moves r from interval cur to the start of the latest interval
// k in (cur, t] whose decoder state is known without decoding: its RSTn
// was consumed, so no marker is pending and process_restart cleared
// insufficient_data (jdhuff.c:512-541). A pending marker keeps the
// previous interval's flag, which only decoding reveals, so such an
// interval is not a landing point. A Restart offset counts when RST(k-1)
// sits just before it; otherwise turbo's own restart automaton
// (readRestartMarker) is replayed on the raw bytes from r, finding the
// same markers the full decode would, so any resync it meets is one turbo
// warns about (jdmarker.c:1223) and marks r lost.
func skipTo(src []byte, r *bitReader, restart []int, cur, t int) (int, bool) {
	for k := min(t, len(restart)-1); k > cur; k-- {
		off := restart[k]
		if off >= 2 && off <= len(src) && src[off-2] == 0xff && src[off-1] == 0xd0+byte((k-1)&7) {
			*r = bitReader{data: src, pos: off, decoded: r.decoded, scanned: r.scanned, lost: r.lost}
			return k, true
		}
	}
	s := *r
	best, pos := 0, 0
	expected := cur & 7
	resynced := false
	for k := cur + 1; k <= t; k++ {
		if readRestartMarker(&s, &expected) {
			resynced = true
		}
		if !s.marker {
			best, pos = k, s.pos
		}
	}
	r.scanned = s.scanned
	if best == 0 {
		return 0, false
	}
	*r = bitReader{data: src, pos: pos, decoded: r.decoded, scanned: r.scanned, lost: r.lost || resynced}
	return best, true
}

// nextBail is the first FF at or after pos not followed by 00, where the
// fast path would bail, and whether it starts a marker inside src: only
// then do both paths stop there with eof unset, whatever their buffering.
func nextBail(src []byte, pos int) (int, bool) {
	for pos < len(src) {
		i := bytes.IndexByte(src[pos:], 0xff)
		if i < 0 {
			break
		}
		pos += i + 1
		if pos < len(src) && src[pos] == 0 {
			pos++
			continue
		}
		j := pos
		for j < len(src) && src[j] == 0xff {
			j++
		}
		return pos - 1, j < len(src) && src[j] != 0
	}
	return len(src), false
}

// maxMCUBlocks is turbo's D_MAX_BLOCKS_IN_MCU.
const maxMCUBlocks = 10

// Test hooks for decode_bitexact_test.go.
var (
	noCombined bool
	forceSkip  bool // every MCU takes the skip path
	goMCU      bool // fastMCU runs Go, not assembly
	noFastRST  bool // restart intervals always decode slow
	probeBits  bool
	probeState []probe
	probeACs   [][maxMCUBlocks]bool // hasAC per written MCU
)

// probe is the reader state after one MCU.
type probe struct {
	pos, bitsLeft int
	dc            [4]int32
}

// naturalOrderFast is jpeg_natural_order (zigzagToNatural) padded to 128
// entries, its 64..127 range mapped to 63 like the 16 real overflow
// entries (jdhuff.c:618): decodeMCUFast indexes it by k&127 and the block
// by &63 so both index expressions are provably in range, dropping the
// bounds checks a plain "if k < 64" branch left in the hot loop.
var naturalOrderFast = func() (t [128]uint8) {
	for i, v := range zigzagToNatural {
		t[i] = uint8(v)
	}
	for i := 64; i < 128; i++ {
		t[i] = 63
	}
	return t
}()

// refillFast is turbo's FILL_BIT_BUFFER_FAST (jdhuff.c:414-417).
func refillFast(data []byte, dataLen, pos int, lb uint64, bits int, marker bool) (uint64, int, int, bool) {
	var acc uint64
	if pos >= 0 && pos+8 <= dataLen {
		v := binary.BigEndian.Uint64(data[pos : pos+8])
		x := ^v | 0xffff // SWAR 0xff test; bytes 6-7 unused
		if (x-0x0101010101010101)&^x&0x8080808080808080 == 0 {
			return lb | v>>16<<((16-bits)&63), pos + 6, bits + 48, marker
		}
	}
	for j := 0; j < 6; j++ {
		acc <<= 8
		if pos >= 0 && pos < dataLen-1 {
			c0, c1 := data[pos], data[pos+1]
			if c0 != 0xff {
				acc |= uint64(c0)
				pos++
			} else if c1 == 0 {
				acc |= 0xff
				pos += 2
			} else {
				marker = true
			}
		} else {
			marker = true
		}
	}
	return lb | acc<<((16-bits)&63), pos, bits + 48, marker
}

// slowHuffL is slowHuff on a left-aligned buffer.
func slowHuffL(d *derivedTable, data []byte, pos int, lb uint64, bits int, marker, insufficient, lost, eof bool) (byte, int, uint64, int, bool, bool, bool, bool) {
	rd := bitReader{data: data, pos: pos, buf: lb >> uint(64-bits), bitsLeft: bits, marker: marker, insufficient: insufficient, lost: lost, eof: eof}
	s := rd.slowHuff(d, lookAheadBits+1)
	return s, rd.pos, rd.buf << uint(64-rd.bitsLeft), rd.bitsLeft, rd.marker, rd.insufficient, rd.lost, rd.eof
}

// fastMCUGo is fastMCU's Go body.
func fastMCUGo(r *bitReader, dc, ac *[4]derivedTable, lastDC *[4]int32, blks *[maxMCUBlocks]*Block, ks *[maxMCUBlocks]int, n int, hasAC *[maxMCUBlocks]bool) bool {
	if blks == nil {
		return decodeMCUFastSkip(r, dc, ac, lastDC, ks[:n])
	}
	return decodeMCUFast(r, dc, ac, lastDC, blks[:n], ks[:n], hasAC[:n])
}

// decodeMCUFast is turbo's decode_mcu_fast; failed writes stay.
// hasAC[i] reports block i's first AC symbol was not EOB.
func decodeMCUFast(r *bitReader, dc, ac *[4]derivedTable, lastDC *[4]int32, mcu []*Block, mcuK []int, hasAC []bool) bool {
	data := r.data
	dataLen := len(data)
	pos := r.pos
	bits := r.bitsLeft
	lb := r.buf << uint(64-bits) // left-aligned: peeks shift by 56
	marker := r.marker
	insufficient := r.insufficient
	lost := r.lost
	eof := r.eof
	dcs := *lastDC

	mcuK = mcuK[:len(mcu)] // one check per MCU
	hasAC = hasAC[:len(mcu)]
	for i, blk := range mcu {
		k := mcuK[i] & 3 // ScanN <= 4
		d, a := &dc[k], &ac[k]

		if bits <= 16 {
			lb, pos, bits, marker = refillFast(data, dataLen, pos, lb, bits, marker)
		}
		var s int32
		if ce := d.combined[lb>>(64-combBits)]; ce != 0 {
			fill := (!combZero || ce&64 == 0) && bits-int(ce>>8&15) <= 16 // turbo refills before the value
			lb <<= ce & 63
			bits -= int(ce & 63)
			if fill {
				lb, pos, bits, marker = refillFast(data, dataLen, pos, lb, bits, marker)
			}
			s = int32(ce) >> 16
		} else {
			entry := d.lookup[lb>>56]
			if nb := int(entry >> lookAheadBits); nb >= 1 && nb <= lookAheadBits {
				lb <<= nb & 63
				bits -= nb
				s = int32(entry & (1<<lookAheadBits - 1))
			} else {
				var sym byte
				sym, pos, lb, bits, marker, insufficient, lost, eof = slowHuffL(d, data, pos, lb, bits, marker, insufficient, lost, eof)
				s = int32(sym)
			}
			if s != 0 {
				if bits <= 16 {
					lb, pos, bits, marker = refillFast(data, dataLen, pos, lb, bits, marker)
				}
				v := int32(lb >> ((64 - s) & 63))
				lb <<= s & 63
				bits -= int(s)
				s = huffExtend(v, s)
			}
		}
		s += dcs[k]
		blk[0] = int16(s)
		dcs[k] = s

		kk := 1
		for ; kk < 64; kk++ {
			if bits <= 16 {
				lb, pos, bits, marker = refillFast(data, dataLen, pos, lb, bits, marker)
			}
			if ce := a.combined[lb>>(64-combBits)]; ce != 0 {
				fill := (!combZero || ce&64 == 0) && bits-int(ce>>8&15) <= 16 // turbo refills before the value
				lb <<= ce & 63
				bits -= int(ce & 63)
				if fill {
					lb, pos, bits, marker = refillFast(data, dataLen, pos, lb, bits, marker)
				}
				if combZero && ce&64 != 0 && ce>>12&15 != 15 {
					break
				}
				kk += int(ce >> 12 & 15)
				if combZero && ce&64 != 0 {
					continue
				}
				blk[naturalOrderFast[kk&127]&63] = int16(int32(ce) >> 16)
				continue
			}
			entry := a.lookup[lb>>56]
			var rs byte
			if nb := int(entry >> lookAheadBits); nb >= 1 && nb <= lookAheadBits {
				lb <<= nb & 63
				bits -= nb
				rs = byte(entry & (1<<lookAheadBits - 1))
			} else {
				rs, pos, lb, bits, marker, insufficient, lost, eof = slowHuffL(a, data, pos, lb, bits, marker, insufficient, lost, eof)
			}
			run, size := int(rs>>4), int(rs&15)
			if size == 0 {
				if run != 15 {
					break
				}
				kk += 15
				continue
			}
			kk += run
			if bits <= 16 {
				lb, pos, bits, marker = refillFast(data, dataLen, pos, lb, bits, marker)
			}
			v := int32(lb >> ((64 - size) & 63))
			lb <<= size & 63
			bits -= size
			blk[naturalOrderFast[kk&127]&63] = int16(huffExtend(v, int32(size)))
		}
		hasAC[i] = kk != 1
	}

	if marker {
		return false
	}
	r.decoded += pos - r.pos
	r.pos, r.buf, r.bitsLeft, r.marker = pos, lb>>uint(64-bits), bits, marker
	r.insufficient, r.lost, r.eof = insufficient, lost, eof
	*lastDC = dcs
	return true
}

// decodeMCUFastSkip is decodeMCUFast for an MCU whose blocks are never read
// (a banded scan's MCUs outside the Region, P5 attempt 1): it walks the same
// bits, so DC prediction for later, needed blocks in these components stays
// correct, but skips every AC coefficient store (no blk, no naturalOrderFast
// index) and the DC store, since nothing reads them. blockK is comps[k]'s
// component index per block, precomputed once per scan (attempt 2): the
// caller needs no plane pointer or scratch block for this MCU at all.
func decodeMCUFastSkip(r *bitReader, dc, ac *[4]derivedTable, lastDC *[4]int32, blockK []int) bool {
	data := r.data
	dataLen := len(data)
	pos := r.pos
	bits := r.bitsLeft
	lb := r.buf << uint(64-bits)
	marker := r.marker
	insufficient := r.insufficient
	lost := r.lost
	eof := r.eof
	dcs := *lastDC

	for _, kk0 := range blockK {
		k := kk0 & 3 // ScanN <= 4
		d, a := &dc[k], &ac[k]

		if bits <= 16 {
			lb, pos, bits, marker = refillFast(data, dataLen, pos, lb, bits, marker)
		}
		var s int32
		if ce := d.combined[lb>>(64-combBits)]; ce != 0 {
			fill := (!combZero || ce&64 == 0) && bits-int(ce>>8&15) <= 16 // turbo refills before the value
			lb <<= ce & 63
			bits -= int(ce & 63)
			if fill {
				lb, pos, bits, marker = refillFast(data, dataLen, pos, lb, bits, marker)
			}
			s = int32(ce) >> 16
		} else {
			entry := d.lookup[lb>>56]
			if nb := int(entry >> lookAheadBits); nb >= 1 && nb <= lookAheadBits {
				lb <<= nb & 63
				bits -= nb
				s = int32(entry & (1<<lookAheadBits - 1))
			} else {
				var sym byte
				sym, pos, lb, bits, marker, insufficient, lost, eof = slowHuffL(d, data, pos, lb, bits, marker, insufficient, lost, eof)
				s = int32(sym)
			}
			if s != 0 {
				if bits <= 16 {
					lb, pos, bits, marker = refillFast(data, dataLen, pos, lb, bits, marker)
				}
				v := int32(lb >> ((64 - s) & 63))
				lb <<= s & 63
				bits -= int(s)
				s = huffExtend(v, s)
			}
		}
		dcs[k] += s

		for kk := 1; kk < 64; kk++ {
			if bits <= 16 {
				lb, pos, bits, marker = refillFast(data, dataLen, pos, lb, bits, marker)
			}
			if ce := a.combined[lb>>(64-combBits)]; ce != 0 {
				fill := (!combZero || ce&64 == 0) && bits-int(ce>>8&15) <= 16 // turbo refills before the value
				lb <<= ce & 63
				bits -= int(ce & 63)
				if fill {
					lb, pos, bits, marker = refillFast(data, dataLen, pos, lb, bits, marker)
				}
				if combZero && ce&64 != 0 && ce>>12&15 != 15 {
					break
				}
				kk += int(ce >> 12 & 15)
				continue
			}
			entry := a.lookup[lb>>56]
			var rs byte
			if nb := int(entry >> lookAheadBits); nb >= 1 && nb <= lookAheadBits {
				lb <<= nb & 63
				bits -= nb
				rs = byte(entry & (1<<lookAheadBits - 1))
			} else {
				rs, pos, lb, bits, marker, insufficient, lost, eof = slowHuffL(a, data, pos, lb, bits, marker, insufficient, lost, eof)
			}
			run, size := int(rs>>4), int(rs&15)
			if size == 0 {
				if run != 15 {
					break
				}
				kk += 15
				continue
			}
			kk += run
			if bits <= 16 {
				lb, pos, bits, marker = refillFast(data, dataLen, pos, lb, bits, marker)
			}
			lb <<= size & 63 // value discarded: nothing reads this block
			bits -= size
		}
	}

	if marker {
		return false
	}
	r.decoded += pos - r.pos
	r.pos, r.buf, r.bitsLeft, r.marker = pos, lb>>uint(64-bits), bits, marker
	r.insufficient, r.lost, r.eof = insufficient, lost, eof
	*lastDC = dcs
	return true
}

// decodeMCUSlow is turbo's decode_mcu_slow (jdhuff.c:560-650).
func decodeMCUSlow(r *bitReader, dc, ac *[4]derivedTable, lastDC *[4]int32, blks *[maxMCUBlocks]*Block, ks *[maxMCUBlocks]int, n int, hasAC *[maxMCUBlocks]bool) {
	buf, bits := r.buf, r.bitsLeft
	for i := range ks[:n] {
		c := ks[i] & 3 // ScanN <= 4
		var blk *Block
		if blks != nil {
			blk = blks[i]
		}
		t := &dc[c]
		kk := 0
		for ; kk < 64; kk++ {
			// HUFF_DECODE fills below the lookahead.
			if bits < lookAheadBits {
				r.buf, r.bitsLeft = buf, bits
				r.fill(0)
				buf, bits = r.buf, r.bitsLeft
			}
			var sym int
			if bits < lookAheadBits {
				r.buf, r.bitsLeft = buf, bits
				sym = int(r.slowHuff(t, 1))
				buf, bits = r.buf, r.bitsLeft
			} else if e := t.lookup[buf>>((bits-lookAheadBits)&63)&(1<<lookAheadBits-1)]; e>>lookAheadBits != 0 {
				bits -= int(e >> lookAheadBits)
				sym = int(e & (1<<lookAheadBits - 1))
			} else {
				r.buf, r.bitsLeft = buf, bits
				sym = int(r.slowHuff(t, lookAheadBits+1))
				buf, bits = r.buf, r.bitsLeft
			}
			size := sym
			if kk > 0 {
				size = sym & 15
				if size == 0 {
					if sym>>4 != 15 {
						break
					}
					kk += 15
					continue
				}
				kk += sym >> 4
			}
			var v int32
			if size != 0 {
				if bits < size { // CHECK_BIT_BUFFER
					r.buf, r.bitsLeft = buf, bits
					r.fill(size)
					buf, bits = r.buf, r.bitsLeft
				}
				bits -= size
				v = huffExtend(int32(buf>>(bits&63))&(1<<(size&31)-1), int32(size))
			}
			if kk == 0 {
				v += lastDC[c]
				lastDC[c] = v
				if blk != nil {
					blk[0] = int16(v)
				}
				t = &ac[c]
			} else if blk != nil {
				blk[naturalOrderFast[kk&127]&63] = int16(v)
			}
		}
		if hasAC != nil {
			hasAC[i] = kk != 1
		}
	}
	r.buf, r.bitsLeft = buf, bits
}

// processRestart advances past the expected RSTn marker and resets
// per-restart state (turbo jdhuff.c process_restart, jdhuff.c:512-541).
func processRestart(r *bitReader, lastDC []int32, expected *int) {
	r.bitsLeft = 0 // discard leftover bits, turbo jdhuff.c:515-517
	if readRestartMarker(r, expected) {
		r.lost = true // turbo always warns on resync
	}
	for i := range lastDC {
		lastDC[i] = 0
	}
	if !r.marker {
		r.insufficient = false
	}
}

// readRestartMarker consumes the next RSTn marker, resyncing on mismatch
// (turbo jdmarker.c read_restart_marker). It reports whether the marker
// didn't match expected and had to be resynced.
func readRestartMarker(r *bitReader, expected *int) bool {
	if !r.marker {
		c, ok := nextMarker(r)
		r.marker = true
		if ok {
			r.markerByte = c
		} else {
			r.markerByte = markerEOI
		}
	}
	resynced := false
	if r.markerByte == byte(0xd0+*expected) {
		r.marker = false
	} else {
		resyncToRestart(r, *expected)
		resynced = true
	}
	*expected = (*expected + 1) & 7
	return resynced
}

// nextMarker scans for the next marker code, skipping stuffed zero bytes
// and padding FFs (turbo jdmarker.c next_marker).
func nextMarker(r *bitReader) (byte, bool) {
	p := r.pos
	c, ok := scanMarker(r)
	r.scanned += r.pos - p
	return c, ok
}

func scanMarker(r *bitReader) (byte, bool) {
	for {
		if r.pos >= len(r.data) {
			r.eof = true
			return 0, false
		}
		c := r.data[r.pos]
		r.pos++
		if c != 0xff {
			continue
		}
		for r.pos < len(r.data) && r.data[r.pos] == 0xff {
			r.pos++
		}
		if r.pos >= len(r.data) {
			r.eof = true
			return 0, false
		}
		c = r.data[r.pos]
		r.pos++
		if c != 0 {
			return c, true
		}
	}
}

// resyncToRestart is turbo's default jpeg_resync_to_restart: it tolerates a
// missing or off-by-one-or-two restart marker (jdmarker.c
// jpeg_resync_to_restart).
func resyncToRestart(r *bitReader, desired int) {
	for {
		m := int(r.markerByte)
		var action int
		switch {
		case m < 0xc0:
			action = 2 // invalid marker
		case m < 0xd0 || m > 0xd7:
			action = 3 // valid non-restart marker
		case m == 0xd0+(desired+1)&7, m == 0xd0+(desired+2)&7:
			action = 3 // one of the next two expected restarts
		case m == 0xd0+(desired-1)&7, m == 0xd0+(desired-2)&7:
			action = 2 // a prior restart, so advance
		default:
			action = 1 // desired restart or too far away
		}
		switch action {
		case 1:
			r.marker = false
			return
		case 2:
			c, ok := nextMarker(r)
			if !ok {
				r.markerByte = markerEOI
				return
			}
			r.markerByte = c
		case 3:
			return
		}
	}
}
