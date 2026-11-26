//go:build arm64 && !purego

package codec

import (
	"encoding/binary"
	"os"
)

// idct8 dispatches to idct8Go's islow IDCT, fused with dequantize,
// level shift and clamp, in NEON (kernel_arm64.s). Matches idct8Go only
// on in-range blocks (docs/design.md section 4); JPEG_NOSIMD selects
// the Go body. A direct call, not a func variable: idctBS's callers
// need this off the heap (decode.go's idctBS comment), and escape
// analysis can't see through an indirect call.
func idct8(coef *Block, quant *[64]uint16, dst []byte, stride int) {
	if noSIMD {
		idct8Go(coef, quant, dst, stride)
		return
	}
	idct8NEON(coef, quant, dst, stride)
}

// huffAsm selects decodeMCUAsm.
var huffAsm = !noSIMD

// idct8Kernel mirrors idct8's selection as a func variable, so
// TestNoSIMDSelectsGo can tell which body a fresh process picked.
var idct8Kernel = pickKernel(idct8NEON, idct8Go)

// IDCT8SIMD reports whether idct8 is a SIMD kernel, not idct8Go: tests
// use it to skip fixtures (evil_q255.jpg) that are only guaranteed to
// match idct8Go in range (docs/design.md section 4).
func IDCT8SIMD() bool { return !noSIMD }

// idct8NEONAsm is idct8Go in NEON. dst must have room through
// [7*stride+7]; idct8NEON below checks that before the call.
//
//go:noescape
func idct8NEONAsm(coef *[64]int16, quant *[64]uint16, dst *byte, stride int)

func idct8NEON(coef *[64]int16, quant *[64]uint16, dst []byte, stride int) {
	_ = dst[7*stride+7]
	idct8NEONAsm(coef, quant, &dst[0], stride)
}

// idct4 dispatches to idct4Go's scaled islow IDCT (jidctred.c:143-293,
// simd/arm/jidctred-neon.c) in NEON (kernel_arm64.s). Matches idct4Go
// only on in-range blocks; JPEG_NOSIMD selects the Go body. T29.
func idct4(coef *Block, quant *[64]uint16, dst []byte, stride int) {
	if noSIMD {
		idct4Go(coef, quant, dst, stride)
		return
	}
	idct4NEON(coef, quant, dst, stride)
}

// idct4Kernel mirrors idct4's selection as a func variable, so
// TestNoSIMDSelectsGo can tell which body a fresh process picked.
var idct4Kernel = pickKernel(idct4NEON, idct4Go)

// IDCT4SIMD reports whether idct4 is a SIMD kernel, not idct4Go.
func IDCT4SIMD() bool { return !noSIMD }

// idct4NEONAsm is idct4Go in NEON. dst must have room through
// [3*stride+3]; idct4NEON below checks that before the call.
//
//go:noescape
func idct4NEONAsm(coef *[64]int16, quant *[64]uint16, dst *byte, stride int)

func idct4NEON(coef *[64]int16, quant *[64]uint16, dst []byte, stride int) {
	_ = dst[3*stride+3]
	idct4NEONAsm(coef, quant, &dst[0], stride)
}

// colorRow dispatches to colorRowGo's YCbCr->RGBA row body in NEON
// (kernel_arm64.s), used by the full-size (unsubsampled) plane path.
func colorRow(dst []byte, y, cb, cr []byte) {
	if noSIMD {
		colorRowGo(dst, y, cb, cr)
		return
	}
	colorRowNEON(dst, y, cb, cr)
}

// colorRowNEONAsm processes n pixels (n a multiple of 16) in NEON.
// colorRowNEON below covers a remainder by redoing the last 16 pixels,
// and uses colorRowGo only for rows shorter than 16.
//
//go:noescape
func colorRowNEONAsm(y, cb, cr, dst *byte, n int)

// colorRowNEONRDMAsm needs FEAT_RDM.
//
//go:noescape
func colorRowNEONRDMAsm(y, cb, cr, dst *byte, n int)

var useRDM = hwcap()&(1<<12) != 0

// hwcap reads AT_HWCAP.
func hwcap() uint64 {
	b, _ := os.ReadFile("/proc/self/auxv")
	for ; len(b) >= 16; b = b[16:] {
		if binary.LittleEndian.Uint64(b) == 16 {
			return binary.LittleEndian.Uint64(b[8:])
		}
	}
	return 0
}

func colorBody(y, cb, cr, dst *byte, n int) {
	if useRDM {
		colorRowNEONRDMAsm(y, cb, cr, dst, n)
		return
	}
	colorRowNEONAsm(y, cb, cr, dst, n)
}

func colorRowNEON(dst []byte, y, cb, cr []byte) {
	n := len(y)
	full := n &^ 15
	if full > 0 {
		_ = cb[full-1]
		_ = cr[full-1]
		_ = dst[full*4-1]
		colorBody(&y[0], &cb[0], &cr[0], &dst[0], full)
		if full < n {
			// Redo the last 16 pixels: overlap rewrites equal bytes.
			c := n - 16
			_ = cb[n-1]
			_ = cr[n-1]
			_ = dst[n*4-1]
			colorBody(&y[c], &cb[c], &cr[c], &dst[c*4], 16)
		}
		return
	}
	colorRowGo(dst[:n*4], y, cb, cr)
}

// h2v1FancyRow dispatches to h2v1FancyRowGo's interior-column loop
// (jdsample.c :295-325) in NEON (kernel_arm64.s); the two edge columns
// stay scalar since NEON buys nothing on a single sample. T25.
func h2v1FancyRow(dst, row []byte, dsw, w int) {
	if noSIMD {
		h2v1FancyRowGo(dst, row, dsw, w)
		return
	}
	v := int32(row[0])
	dst[0] = byte(v)
	dst[1] = byte((v*3 + int32(row[1]) + 2) >> 2)

	n := dsw - 2 // interior columns c = 1..dsw-2
	full := n &^ 15
	if full > 0 {
		_ = row[full+1]
		_ = dst[2+full*2-1]
		h2v1FancyRowNEONAsm(&row[0], &row[1], &row[2], &dst[2], full)
		if full < n {
			// Redo the last 16 columns: overlap rewrites equal bytes.
			c := n - 16
			_ = row[c+17]
			_ = dst[2+n*2-1]
			h2v1FancyRowNEONAsm(&row[c], &row[c+1], &row[c+2], &dst[2+c*2], 16)
			full = n
		}
	}
	o := 2 + full*2
	for c := full + 1; c < dsw-1; c++ {
		v = int32(row[c]) * 3
		dst[o] = byte((v + int32(row[c-1]) + 1) >> 2)
		dst[o+1] = byte((v + int32(row[c+1]) + 2) >> 2)
		o += 2
	}
	v = int32(row[dsw-1])
	dst[o] = byte((v*3 + int32(row[dsw-2]) + 1) >> 2)
	if o+1 < w {
		dst[o+1] = byte(v)
	}
}

// h2v1FancyRowNEONAsm processes n (a multiple of 16) interior columns
// starting at c=1: a/b/c are row[c-1]/row[c]/row[c+1], dst is the output
// pair for c=1. h2v1FancyRow above runs the two edge columns, redoes
// the last 16 columns for a remainder, and uses Go only below 16.
//
//go:noescape
func h2v1FancyRowNEONAsm(a, b, c, dst *byte, n int)

// h2v2FancyRow dispatches to h2v2FancyRowGo's interior-column loop
// (jdsample.c :381-429) in NEON (kernel_arm64.s); the two edge columns
// stay scalar. T25.
func h2v2FancyRow(dst, rowCur, rowOther []byte, dsw, w int) {
	if noSIMD {
		h2v2FancyRowGo(dst, rowCur, rowOther, dsw, w)
		return
	}
	colsum := func(c int) int32 { return int32(rowCur[c])*3 + int32(rowOther[c]) }
	thiscolsum := colsum(0)
	nextcolsum := colsum(1)
	dst[0] = byte((thiscolsum*4 + 8) >> 4)
	dst[1] = byte((thiscolsum*3 + nextcolsum + 7) >> 4)

	n := dsw - 2 // interior columns c = 1..dsw-2
	full := n &^ 15
	if full > 0 {
		_ = rowCur[full+1]
		_ = rowOther[full+1]
		_ = dst[2+full*2-1]
		h2v2FancyRowNEONAsm(&rowCur[0], &rowCur[1], &rowCur[2],
			&rowOther[0], &rowOther[1], &rowOther[2], &dst[2], full)
		if full < n {
			// Redo the last 16 columns: overlap rewrites equal bytes.
			c := n - 16
			_ = rowCur[c+17]
			_ = rowOther[c+17]
			_ = dst[2+n*2-1]
			h2v2FancyRowNEONAsm(&rowCur[c], &rowCur[c+1], &rowCur[c+2],
				&rowOther[c], &rowOther[c+1], &rowOther[c+2], &dst[2+c*2], 16)
			full = n
		}
	}
	lastcolsum := colsum(full)
	thiscolsum = colsum(full + 1)
	o := 2 + full*2
	for c := full + 1; c < dsw-1; c++ {
		nextcolsum = colsum(c + 1)
		dst[o] = byte((thiscolsum*3 + lastcolsum + 8) >> 4)
		dst[o+1] = byte((thiscolsum*3 + nextcolsum + 7) >> 4)
		lastcolsum = thiscolsum
		thiscolsum = nextcolsum
		o += 2
	}
	dst[o] = byte((thiscolsum*3 + lastcolsum + 8) >> 4)
	if o+1 < w {
		dst[o+1] = byte((thiscolsum*4 + 7) >> 4)
	}
}

// h2v2FancyRowNEONAsm processes n (a multiple of 16) interior columns
// starting at c=1: curA/curB/curC are rowCur[c-1]/[c]/[c+1], likewise
// otherA/otherB/otherC for rowOther, dst is the output pair for c=1.
// h2v2FancyRow above runs the edges, redoes the last 16 columns for a
// remainder, and uses Go only below 16.
//
//go:noescape
func h2v2FancyRowNEONAsm(curA, curB, curC, otherA, otherB, otherC, dst *byte, n int)
