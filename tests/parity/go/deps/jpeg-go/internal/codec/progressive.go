package codec

import (
	"context"
	"errors"
	"math"
)

// Preallocated so failures allocate nothing.
var (
	errBadProgression = errors.New("jpeg: invalid progressive parameters")
	errBadDCTCoef     = errors.New("jpeg: DC coefficient out of range")
)

// progState is turbo's cross-scan state.
type progState struct {
	coefBits [3][64]int8
	prevBits [3][10]int8
	scans    int // cinfo->input_scan_number
	lastGood int // master->last_good_iMCU_row
}

func newProgState() progState {
	var ps progState
	for ci := range ps.coefBits {
		for i := range ps.coefBits[ci] {
			ps.coefBits[ci][i] = -1
		}
	}
	return ps
}

// latchQuant is turbo latch_quant_tables.
func latchQuant(f *Frame, planes []Plane) error {
	for k := 0; k < f.ScanN; k++ {
		ci := f.ScanComp[k]
		if p := &planes[ci]; !p.latched {
			tq := f.Comp[ci].Tq
			if tq > 3 || !f.HasQuant[tq] {
				return errNoQuantTable
			}
			p.quant, p.latched = f.Quant[tq], true
		}
	}
	return nil
}

// decodeProgressive is decodeScan for SOF2.
func decodeProgressive(ctx context.Context, f *Frame, src []byte, planes []Plane) (bool, int, int, error) {
	hmax, vmax := MaxSampling(f)
	truncated, decoded, scannedBytes, ps, scanned, err := decodeProgScans(ctx, f, src, planes)
	if err != nil {
		return truncated, decoded, scannedBytes, err
	}
	if err := idctProgressive(ctx, f, planes, &scanned, &ps, hmax, vmax); err != nil {
		return true, 0, 0, err
	}
	for ci := 0; ci < f.NComp; ci++ {
		if scanned[ci] {
			continue
		}
		if p := &planes[ci]; !p.Skip && !p.Coef {
			for i := range p.Pix {
				p.Pix[i] = 128
			}
		}
	}
	return truncated, decoded, scannedBytes, nil
}

// decodeProgScans runs every scan of a SOF2 frame into planes' Blocks,
// leaving the IDCT to the caller, with the state smoothing needs.
func decodeProgScans(ctx context.Context, f *Frame, src []byte, planes []Plane) (bool, int, int, progState, [3]bool, error) {
	hmax, vmax := MaxSampling(f)
	last := ceilDiv(f.Height, vmax*8) - 1
	for ci := range planes {
		c := f.Comp[ci]
		w := ceilDiv(f.Width, hmax*8) * int(c.H)
		h := (last + 1) * int(c.V)
		p := &planes[ci]
		bs := p.blockSize()
		if err := f.Ledger.Charge(int64(w) * int64(h) * blockBytes); err != nil {
			return false, 0, 0, progState{}, [3]bool{}, err
		}
		if cap(p.Blocks) < w*h {
			p.Blocks = make([]Block, w*h)
		} else {
			p.Blocks = p.Blocks[:w*h]
			clear(p.Blocks)
		}
		if !p.Coef && !p.Skip {
			n := w * h * bs * bs
			if err := f.Ledger.Charge(int64(n)); err != nil {
				return false, 0, 0, progState{}, [3]bool{}, err
			}
			if cap(p.Pix) < n {
				p.Pix = make([]byte, n)
			}
			p.Pix = p.Pix[:n]
		}
		p.BlocksWide, p.BlocksHigh = w, h
		p.RealWide = ceilDiv(f.Width*int(c.H), hmax*8)
		p.RealHigh = clampInt(ceilDiv(f.Height*int(c.V), vmax*8), 0, h)
		p.latched = false
		if !p.Skip && !p.Coef {
			rw, rh := DownsampledDims(f, ci, hmax, vmax, bs)
			fillPadding(p, rw, clampInt(rh, 0, h*bs))
		}
	}

	var scanned [3]bool
	ps := newProgState()
	var r bitReader
	offset := f.ScanOffset
	truncated := false
	for {
		r = bitReader{data: src, pos: offset}
		if err := decodeProgScan(ctx, f, hmax, vmax, planes, &r, &ps); err != nil {
			return true, 0, 0, progState{}, [3]bool{}, err
		}
		for k := 0; k < f.ScanN; k++ {
			scanned[f.ScanComp[k]] = true
		}
		if !r.marker {
			c, ok := nextMarker(&r)
			r.marker, r.markerByte = ok, c
		}
		truncated = truncated || r.insufficient || r.eof || r.lost
		if !r.marker || r.markerByte == markerEOI {
			break
		}
		sos, next, err := f.consumeFromMarker(src, r.markerByte, r.pos, true)
		if err != nil {
			return true, r.decoded, r.scanned, progState{}, [3]bool{}, err
		}
		if !sos {
			truncated = truncated || f.Overran
			break
		}
		offset = next
	}
	return truncated, r.decoded, r.scanned, ps, scanned, nil
}

// decodeProgScan follows jdphuff.c, jdcoefct.c.
func decodeProgScan(ctx context.Context, f *Frame, hmax, vmax int, planes []Plane, r *bitReader, ps *progState) error {
	ps.scans++
	if err := latchQuant(f, planes); err != nil {
		return err
	}

	ss, se, ah, al := int(f.ScanSs), int(f.ScanSe), int(f.ScanAh), int(f.ScanAl)
	dcBand := ss == 0
	bad := al > 13 || ah != 0 && al != ah-1
	if dcBand {
		bad = bad || se != 0
	} else {
		bad = bad || ss > se || se >= 64 || f.ScanN != 1
	}
	if bad {
		return errBadProgression
	}
	for k := 0; k < f.ScanN; k++ {
		ci := f.ScanComp[k]
		cb, pb := &ps.coefBits[ci], &ps.prevBits[ci]
		if !dcBand && cb[0] < 0 {
			r.lost = true // AC without a prior DC scan
		}
		for c := min(ss, 1); c <= min(max(se, 9), 9); c++ {
			if ps.scans > 1 {
				pb[c] = cb[c]
			} else {
				pb[c] = 0
			}
		}
		for c := ss; c <= se; c++ {
			if ah != max(int(cb[c]), 0) {
				r.lost = true // inconsistent progression
			}
			cb[c] = int8(al)
		}
	}

	var dc [4]derivedTable
	var ac derivedTable
	if dcBand {
		if ah == 0 { // DC refinement needs no table
			for k := 0; k < f.ScanN; k++ {
				if f.ScanDC[k] > 3 || !f.HasDC[f.ScanDC[k]] {
					return errNoHuffTable
				}
				if err := dc[k].build(&f.DC[f.ScanDC[k]], true); err != nil {
					return err
				}
			}
		}
	} else {
		if f.ScanAC[0] > 3 || !f.HasAC[f.ScanAC[0]] {
			return errNoHuffTable
		}
		if err := ac.build(&f.AC[f.ScanAC[0]], false); err != nil {
			return err
		}
	}

	// Non-interleaved scans skip MCU padding.
	interleaved := f.ScanN > 1
	var comps [4]scanComp
	for k := 0; k < f.ScanN; k++ {
		ci := f.ScanComp[k]
		comps[k] = scanComp{ci, 1, 1}
		if interleaved {
			comps[k] = scanComp{ci, int(f.Comp[ci].H), int(f.Comp[ci].V)}
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
	var blockK, blockX, blockY [maxMCUBlocks]int
	nBlk := 0
	for k := 0; k < f.ScanN; k++ {
		for yy := 0; yy < comps[k].mcuH; yy++ {
			for xx := 0; xx < comps[k].mcuW; xx++ {
				blockK[nBlk], blockX[nBlk], blockY[nBlk] = k, xx, yy
				nBlk++
			}
		}
	}

	var lastDC [4]int32
	var mcu [maxMCUBlocks]*Block
	var eobrun uint32
	ri := f.RestartInterval
	restartsToGo, nextRestart := ri, 0
	p1, m1 := int16(1)<<al, -(int16(1) << al)

	for my := 0; my < mcuRows; my++ {
		select {
		case <-ctx.Done():
			return ctx.Err()
		default:
		}
		imcu := my
		if !interleaved {
			imcu = my / int(f.Comp[comps[0].ci].V)
		}
		for mx := 0; mx < mcusPerRow; mx++ {
			if !r.insufficient {
				ps.lastGood = imcu
			}
			if ri != 0 && restartsToGo == 0 {
				processRestart(r, lastDC[:f.ScanN], &nextRestart)
				eobrun = 0
				restartsToGo = ri
			}
			for i := 0; i < nBlk; i++ {
				sc := comps[blockK[i]]
				mcu[i] = planes[sc.ci].at(mx*sc.mcuW+blockX[i], my*sc.mcuH+blockY[i])
			}
			switch {
			case dcBand && ah == 0:
				if !r.insufficient {
					if err := dcFirst(r, &dc, &lastDC, mcu[:nBlk], blockK[:nBlk], al); err != nil {
						return err
					}
				}
			case dcBand:
				for _, blk := range mcu[:nBlk] {
					r.ensure(1)
					if r.getBits(1) != 0 {
						blk[0] |= p1
					}
				}
			case ah == 0:
				if !r.insufficient {
					eobrun = acFirst(r, &ac, mcu[0], eobrun, ss, se, al)
				}
			default:
				if !r.insufficient {
					eobrun = acRefine(r, &ac, mcu[0], eobrun, ss, se, p1, m1)
				}
			}
			restartsToGo--
		}
	}
	return nil
}

// dcFirst is decode_mcu_DC_first.
func dcFirst(r *bitReader, dc *[4]derivedTable, lastDC *[4]int32, mcu []*Block, ks []int, al int) error {
	for i, blk := range mcu {
		k := ks[i]
		s := int32(r.decodeHuff(&dc[k]))
		if s != 0 {
			r.ensure(int(s))
			s = huffExtend(r.getBits(int(s)), s)
		}
		if l := lastDC[k]; l >= 0 && s > math.MaxInt32-l || l < 0 && s < math.MinInt32-l {
			return errBadDCTCoef
		}
		s += lastDC[k]
		lastDC[k] = s
		blk[0] = int16(s << al)
	}
	return nil
}

// acFirst is decode_mcu_AC_first.
func acFirst(r *bitReader, ac *derivedTable, blk *Block, eobrun uint32, ss, se, al int) uint32 {
	if eobrun > 0 {
		return eobrun - 1
	}
	for k := ss; k <= se; k++ {
		sym := int(r.decodeHuff(ac))
		run, s := sym>>4, sym&15
		if s != 0 {
			k += run
			r.ensure(s)
			v := huffExtend(r.getBits(s), int32(s))
			blk[naturalOrderFast[k&127]&63] = int16(v << al)
		} else if run == 15 {
			k += 15
		} else {
			eobrun = 1 << run
			if run != 0 {
				r.ensure(run)
				eobrun += uint32(r.getBits(run))
			}
			return eobrun - 1
		}
	}
	return 0
}

// acRefine is decode_mcu_AC_refine.
func acRefine(r *bitReader, ac *derivedTable, blk *Block, eobrun uint32, ss, se int, p1, m1 int16) uint32 {
	k := ss
	refine := func(c *int16) {
		r.ensure(1)
		if r.getBits(1) != 0 && *c&p1 == 0 {
			if *c >= 0 {
				*c += p1
			} else {
				*c += m1
			}
		}
	}
	if eobrun == 0 {
		for ; k <= se; k++ {
			sym := int(r.decodeHuff(ac))
			run, s := sym>>4, sym&15
			var nv int16
			if s != 0 {
				if s != 1 {
					r.lost = true // new coefficient is always 1
				}
				r.ensure(1)
				if r.getBits(1) != 0 {
					nv = p1
				} else {
					nv = m1
				}
			} else if run != 15 {
				eobrun = 1 << run
				if run != 0 {
					r.ensure(run)
					eobrun += uint32(r.getBits(run))
				}
				break
			}
			for {
				c := &blk[naturalOrderFast[k&127]&63]
				if *c != 0 {
					refine(c)
				} else if run--; run < 0 {
					break
				}
				if k++; k > se {
					break
				}
			}
			if nv != 0 {
				blk[naturalOrderFast[k&127]&63] = nv
			}
		}
	}
	if eobrun > 0 {
		for ; k <= se; k++ {
			if c := &blk[naturalOrderFast[k&127]&63]; *c != 0 {
				refine(c)
			}
		}
		eobrun--
	}
	return eobrun
}

// smoothPos: Q01_POS..Q30_POS in natural order.
var smoothPos = [9]int{1, 8, 16, 9, 2, 3, 10, 17, 24}

// smoothPred caps at 1<<al.
func smoothPred(num, q int64, al int) int16 {
	neg := num < 0
	if neg {
		num = -num
	}
	pred := (q<<7 + num) / (q << 8)
	if al > 0 && pred >= 1<<al {
		pred = 1<<al - 1
	}
	if neg {
		pred = -pred
	}
	return int16(pred)
}

// smoothingOK is turbo smoothing_ok.
func (ps *progState) smoothingOK(f *Frame, planes []Plane) bool {
	useful := false
	for ci := 0; ci < f.NComp; ci++ {
		p := &planes[ci]
		if !p.latched {
			return false
		}
		q := &p.quant
		for _, pos := range [...]int{0, 1, 8, 16, 9, 2, 3, 10, 17, 24} {
			if q[pos] == 0 {
				return false
			}
		}
		if ps.coefBits[ci][0] < 0 {
			return false
		}
		for c := 1; c < 10; c++ {
			if ps.coefBits[ci][c] != 0 {
				useful = true
			}
		}
	}
	return useful
}

// idctProgressive waits for every scan.
func idctProgressive(ctx context.Context, f *Frame, planes []Plane, scanned *[3]bool, ps *progState, hmax, vmax int) error {
	smooth := ps.smoothingOK(f, planes)
	for ci := 0; ci < f.NComp; ci++ {
		p := &planes[ci]
		if p.Skip || p.Coef || !scanned[ci] {
			continue
		}
		var sm smoother
		if smooth {
			sm = newSmoother(f, ps, ci, p, vmax)
		}
		for by := 0; by < p.RealHigh; by++ {
			if by%int(f.Comp[ci].V) == 0 {
				select {
				case <-ctx.Done():
					return ctx.Err()
				default:
				}
			}
			var smp *smoother
			if smooth {
				smp = &sm
			}
			idctRow(p, smp, by, p.RealWide, p.Pix, 0)
		}
	}
	return nil
}

// idctRow IDCTs blocks [0, width) of block row by into pix, whose row 0 is
// block row pixBy0; sm, when set, smooths them first.
func idctRow(p *Plane, sm *smoother, by, width int, pix []byte, pixBy0 int) {
	bs, stride := p.blockSize(), p.Stride()
	if sm != nil {
		sm.startRow(by)
	}
	var ws Block
	for bx := 0; bx < width; bx++ {
		blk := p.at(bx, by)
		if sm != nil {
			ws = *blk
			sm.estimate(&ws, bx)
			blk = &ws
		}
		dst := pix[(by-pixBy0)*bs*stride+bx*bs:]
		if p.ReconIDCT != nil {
			var deq [64]int32
			for j, c := range blk {
				deq[j] = int32(dequantize(c, p.quant[j]))
			}
			p.ReconIDCT(dst, stride, &deq)
			continue
		}
		idctBS(bs, blk, true, &p.quant, dst, stride)
	}
}

// smoother is per-component turbo smoothing.
type smoother struct {
	q            *[64]uint16
	cur, prv     [10]int8 // this and previous scan
	lastGood     int
	v, totalRows int // iMCU geometry
	high, width  int // real size in blocks
	blocks       []Block
	stride       int

	bits   [10]int8 // per-row precision
	change bool
	rows   [5][]Block // rows -2 to +2
	d      [25]int64  // 5x5 DC window
}

func newSmoother(f *Frame, ps *progState, ci int, p *Plane, vmax int) smoother {
	s := smoother{
		q: &p.quant, lastGood: ps.lastGood, v: int(f.Comp[ci].V),
		totalRows: ceilDiv(f.Height, vmax*8), high: p.RealHigh, width: p.RealWide,
		blocks: p.Blocks, stride: p.BlocksWide,
	}
	for c := range s.cur {
		s.cur[c], s.prv[c] = ps.coefBits[ci][c], -1
		if ps.scans > 1 {
			s.prv[c] = ps.prevBits[ci][c]
		}
	}
	return s
}

func (s *smoother) startRow(by int) {
	imcu := by / s.v
	blockRows := s.v
	if imcu == s.totalRows-1 {
		if blockRows = s.high % s.v; blockRows == 0 {
			blockRows = s.v
		}
	}
	ibr, ibrs := imcu*blockRows+by%s.v, blockRows*s.totalRows
	prev, next := by, by
	if ibr > 0 {
		prev = by - 1
	}
	if ibr < ibrs-1 {
		next = by + 1
	}
	pp, nn := prev, next
	if ibr > 1 {
		pp = by - 2
	}
	if ibr < ibrs-2 {
		nn = by + 2
	}
	for i, row := range [5]int{pp, prev, by, next, nn} {
		s.rows[i] = s.blocks[row*s.stride:][:s.width]
		v := int64(s.rows[i][0][0])
		s.d[i*5], s.d[i*5+1], s.d[i*5+2], s.d[i*5+3], s.d[i*5+4] = v, v, v, v, v
	}
	s.bits = s.cur
	if imcu > s.lastGood {
		s.bits = s.prv
	}
	s.change = true
	for _, b := range s.bits[1:] {
		s.change = s.change && b == -1
	}
}

// estimate slides the DC window.
func (s *smoother) estimate(ws *Block, bx int) {
	d := &s.d
	for r := 0; r < 5; r++ {
		if bx == 0 && bx < s.width-1 {
			d[r*5+3], d[r*5+4] = int64(s.rows[r][1][0]), int64(s.rows[r][1][0])
		}
		if bx+1 < s.width-1 {
			d[r*5+4] = int64(s.rows[r][bx+2][0])
		}
	}
	q00 := int64(s.q[0])
	for i := 0; i < 9 && (i < 5 || s.change); i++ {
		pos := smoothPos[i]
		if al := int(s.bits[i+1]); al != 0 && ws[pos] == 0 {
			ws[pos] = smoothPred(q00*smoothNum(i, s.change, d), int64(s.q[pos]), al)
		}
	}
	if s.change {
		ws[0] = smoothPred(q00*smoothNum(9, true, d), q00, 0)
	}
	for r := 0; r < 25; r += 5 {
		copy(d[r:r+4], d[r+1:r+5])
	}
}

// Kernel; i == 9 is DC.
func smoothNum(i int, change bool, d *[25]int64) int64 {
	switch i {
	case 0:
		if change {
			return -d[0] - d[1] + d[3] + d[4] - 3*d[5] + 13*d[6] - 13*d[8] +
				3*d[9] - 3*d[10] + 38*d[11] - 38*d[13] + 3*d[14] -
				3*d[15] + 13*d[16] - 13*d[18] + 3*d[19] - d[20] - d[21] +
				d[23] + d[24]
		}
		return -7*d[10] + 50*d[11] - 50*d[13] + 7*d[14]
	case 1:
		if change {
			return -d[0] - 3*d[1] - 3*d[2] - 3*d[3] - d[4] - d[5] + 13*d[6] +
				38*d[7] + 13*d[8] - d[9] + d[15] - 13*d[16] - 38*d[17] -
				13*d[18] + d[19] + d[20] + 3*d[21] + 3*d[22] + 3*d[23] +
				d[24]
		}
		return -7*d[2] + 50*d[7] - 50*d[17] + 7*d[22]
	case 2:
		if change {
			return d[2] + 2*d[6] + 7*d[7] + 2*d[8] - 5*d[11] - 14*d[12] -
				5*d[13] + 2*d[16] + 7*d[17] + 2*d[18] + d[22]
		}
		return -d[2] + 13*d[7] - 24*d[12] + 13*d[17] - d[22]
	case 3:
		if change {
			return -d[0] + d[4] + 9*d[6] - 9*d[8] - 9*d[16] + 9*d[18] + d[20] -
				d[24]
		}
		return d[9] + d[15] - 10*d[16] + 10*d[18] - d[1] - d[19] + d[21] -
			d[23] + d[3] - d[5] + 10*d[6] - 10*d[8]
	case 4:
		if change {
			return 2*d[6] - 5*d[7] + 2*d[8] + d[10] + 7*d[11] - 14*d[12] +
				7*d[13] + d[14] + 2*d[16] - 5*d[17] + 2*d[18]
		}
		return -d[10] + 13*d[11] - 24*d[12] + 13*d[13] - d[14]
	case 5:
		return d[6] - d[8] + 2*d[11] - 2*d[13] + d[16] - d[18]
	case 6:
		return d[6] - 3*d[7] + d[8] - d[16] + 3*d[17] - d[18]
	case 7:
		return d[6] - d[8] - 3*d[11] + 3*d[13] + d[16] - d[18]
	case 8:
		return d[6] + 2*d[7] + d[8] - d[16] - 2*d[17] - d[18]
	}
	return -2*d[0] - 6*d[1] - 8*d[2] - 6*d[3] - 2*d[4] - 6*d[5] +
		6*d[6] + 42*d[7] + 6*d[8] - 6*d[9] - 8*d[10] +
		42*d[11] + 152*d[12] + 42*d[13] - 8*d[14] - 6*d[15] +
		6*d[16] + 42*d[17] + 6*d[18] - 6*d[19] - 2*d[20] -
		6*d[21] - 8*d[22] - 6*d[23] - 2*d[24]
}
