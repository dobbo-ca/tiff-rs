package codec

import (
	"bytes"
	"context"
	"sort"
	"sync"
	"sync/atomic"
)

// bandsUsed and bandsRedone count decodes whose bands stood or went
// back to serial, for tests.
var bandsUsed, bandsRedone atomic.Int64

// BandsUsed is bandsUsed, for package jpeg's tests.
func BandsUsed() int64 { return bandsUsed.Load() }

// span is the restart intervals [k0, k1) decodeOneScan decodes; the zero
// span is the whole scan.
type span struct{ k0, k1 int }

// bandable reports whether f's scan can be decoded in restart-interval
// bands: one sequential scan over every component, with a DRI, writing
// only 8x8 Pix blocks, which never touch a neighbour MCU's bytes.
func bandable(f *Frame, planes []Plane) bool {
	if f.Progressive || f.ScanN != f.NComp || f.RestartInterval == 0 || probeBits || forceSkip {
		return false
	}
	if f.ScanSs != 0 || f.ScanSe != 63 || f.ScanAh != 0 || f.ScanAl != 0 {
		return false
	}
	for i := range planes {
		p := &planes[i]
		if p.Coef || p.ReconIDCT != nil || !p.Skip && p.blockSize() != 8 {
			return false
		}
	}
	return true
}

// scanMCUs is decodeOneScan's MCU count for a scan over every
// component; a lone component scans its own block grid.
func scanMCUs(f *Frame, hmax, vmax int) int {
	if f.ScanN > 1 {
		return ceilDiv(f.Width, hmax*8) * ceilDiv(f.Height, vmax*8)
	}
	c := f.Comp[f.ScanComp[0]]
	return ceilDiv(f.Width*int(c.H), hmax*8) * ceilDiv(f.Height*int(c.V), vmax*8)
}

// restartStarts returns the first entropy byte of each of the n restart
// intervals from off, or nil unless the n-1 markers between them are
// RST0..7 in order. The bit reader cannot pass a marker and lexes FF00
// and FF fill as scanMarker does, so a serial decode meets these same
// markers with no resync, and leaves interval k in the state band k
// starts from (DC zero, no bits, insufficient clear: processRestart).
func restartStarts(src []byte, off, n int) []int {
	if n < 2 || off > len(src) {
		return nil
	}
	starts := make([]int, n)
	starts[0] = off
	pos := off
	for k := 1; k < n; k++ {
		for { // scanMarker, with IndexByte
			i := bytes.IndexByte(src[pos:], 0xff)
			if i < 0 {
				return nil
			}
			pos += i + 1
			for pos < len(src) && src[pos] == 0xff {
				pos++
			}
			if pos >= len(src) {
				return nil
			}
			c := src[pos]
			pos++
			if c == 0 {
				continue
			}
			if c != 0xd0+byte((k-1)&7) {
				return nil
			}
			break
		}
		starts[k] = pos
	}
	return starts
}

// decodeBands decodes starts' intervals in up to workers contiguous
// bands of about equal entropy bytes, the last on this goroutine. The
// bands write disjoint MCUs of the same planes. ok is false when any band
// lost data or did not end on its RSTn; the caller then decodes the scan
// serially, which rewrites every MCU. r is the last band's reader.
func decodeBands(ctx context.Context, f *Frame, src []byte, hmax, vmax int, planes []Plane, starts []int, workers int) (r bitReader, ok bool, err error) {
	if err := latchQuant(f, planes); err != nil {
		return r, false, err
	}
	n := len(starts)
	k := min(workers, n)
	cuts := make([]int, 1, k+1)
	for b := 1; b < k; b++ {
		c := sort.SearchInts(starts, starts[0]+(len(src)-starts[0])*b/k)
		if c > cuts[len(cuts)-1] && c < n {
			cuts = append(cuts, c)
		}
	}
	cuts = append(cuts, n)

	rs := make([]bitReader, len(cuts)-1)
	errs := make([]error, len(rs))
	var wg sync.WaitGroup
	for i := range rs {
		run := func() {
			rs[i] = bitReader{data: src, pos: starts[cuts[i]]}
			_, errs[i] = decodeOneScan(ctx, f, src, hmax, vmax, planes, &rs[i], false, false, 0, 0, nil, span{cuts[i], cuts[i+1]})
		}
		if i == len(rs)-1 {
			run()
			break
		}
		wg.Add(1)
		go func() {
			defer wg.Done()
			run()
		}()
	}
	wg.Wait()
	ok = true
	for i := range rs {
		if errs[i] != nil {
			return r, false, errs[i]
		}
		ok = ok && !rs[i].lost && !rs[i].insufficient && !rs[i].eof
	}
	if ok {
		bandsUsed.Add(1)
	} else {
		bandsRedone.Add(1)
	}
	return rs[len(rs)-1], ok, nil
}
