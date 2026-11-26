//go:build amd64 && !purego

package codec

// useAVX2 picks the AVX2 kernel over the Go body; tests clear it to
// cover the no-AVX2 fallback (copied cpuid/xgetbv check from
// jp2k-go/internal/dsp/dwt_amd64.go:16-30).
var useAVX2 = !noSIMD && hasAVX2()

func hasAVX2() bool {
	maxID, _, _, _ := cpuid(0, 0)
	if maxID < 7 {
		return false
	}
	_, _, ecx1, _ := cpuid(1, 0)
	if ecx1&(1<<27) == 0 { // OSXSAVE
		return false
	}
	if xcr0, _ := xgetbv(); xcr0&6 != 6 { // OS saves XMM and YMM state
		return false
	}
	_, ebx7, _, _ := cpuid(7, 0)
	return ebx7&(1<<5) != 0 // AVX2
}

// huffAsm: decodeMCUAsm shifts with BMI2.
var huffAsm = !noSIMD && hasBMI2()

func hasBMI2() bool {
	if maxID, _, _, _ := cpuid(0, 0); maxID < 7 {
		return false
	}
	_, ebx7, _, _ := cpuid(7, 0)
	return ebx7&(1<<8) != 0
}

func cpuid(eaxArg, ecxArg uint32) (eax, ebx, ecx, edx uint32)

func xgetbv() (eax, edx uint32)

// idct8 dispatches to idct8Go's islow IDCT, fused with dequantize,
// level shift and clamp, in AVX2 (kernel_amd64.s) when the CPU has it.
// Matches idct8Go only on in-range blocks (docs/design.md section 4),
// the same contract turbo's own AVX2 kernel has.
func idct8(coef *Block, quant *[64]uint16, dst []byte, stride int) {
	if !useAVX2 {
		idct8Go(coef, quant, dst, stride)
		return
	}
	idct8AVX2(coef, quant, dst, stride)
}

// idct8Kernel mirrors idct8's selection as a func variable, so
// TestNoSIMDSelectsGo can tell which body a fresh process picked.
var idct8Kernel = pickKernel(idct8AVX2, idct8Go)

// IDCT8SIMD reports whether idct8 is a SIMD kernel, not idct8Go: tests
// use it to skip fixtures (evil_q255.jpg) that are only guaranteed to
// match idct8Go in range (docs/design.md section 4).
func IDCT8SIMD() bool { return useAVX2 }

// colorRow dispatches to colorRowGo's YCbCr->RGBA row body in AVX2
// (kernel_amd64.s) when the CPU has it.
func colorRow(dst []byte, y, cb, cr []byte) {
	if !useAVX2 {
		colorRowGo(dst, y, cb, cr)
		return
	}
	colorRowAVX2(dst, y, cb, cr)
}

// colorRowAVX2Asm processes n pixels (n a multiple of 16) in AVX2.
// colorRowAVX2 below covers a remainder by redoing the last 16 pixels,
// and uses colorRowGo only for rows shorter than 16.
//
//go:noescape
func colorRowAVX2Asm(y, cb, cr, dst *byte, n int)

func colorRowAVX2(dst []byte, y, cb, cr []byte) {
	n := len(y)
	full := n &^ 15
	if full > 0 {
		_ = cb[full-1]
		_ = cr[full-1]
		_ = dst[full*4-1]
		colorRowAVX2Asm(&y[0], &cb[0], &cr[0], &dst[0], full)
		if full < n {
			// Redo the last 16 pixels: overlap rewrites equal bytes.
			c := n - 16
			_ = cb[n-1]
			_ = cr[n-1]
			_ = dst[n*4-1]
			colorRowAVX2Asm(&y[c], &cb[c], &cr[c], &dst[c*4], 16)
		}
		return
	}
	colorRowGo(dst[:n*4], y, cb, cr)
}

// idct8AVX2Asm is idct8Go in AVX2. dst must have room through
// [7*stride+7]; idct8AVX2 below checks that before the call.
//
//go:noescape
func idct8AVX2Asm(coef *[64]int16, quant *[64]uint16, dst *byte, stride int)

func idct8AVX2(coef *[64]int16, quant *[64]uint16, dst []byte, stride int) {
	_ = dst[7*stride+7]
	idct8AVX2Asm(coef, quant, &dst[0], stride)
}

// idct4 dispatches to idct4Go's scaled islow IDCT (jidctred.c:143-293,
// simd/x86_64/jidctred-sse2.asm) in AVX2 (kernel_amd64.s) when the CPU
// has it. Matches idct4Go only on in-range blocks (docs/design.md
// section 4). T30.
func idct4(coef *Block, quant *[64]uint16, dst []byte, stride int) {
	if !useAVX2 {
		idct4Go(coef, quant, dst, stride)
		return
	}
	idct4AVX2(coef, quant, dst, stride)
}

// idct4Kernel mirrors idct4's selection, like idct8Kernel, for
// TestNoSIMDSelectsGo.
var idct4Kernel = pickKernel(idct4AVX2, idct4Go)

// IDCT4SIMD reports whether idct4 is a SIMD kernel, not idct4Go.
func IDCT4SIMD() bool { return useAVX2 }

// idct4AVX2Asm is idct4Go in AVX2. dst must have room through
// [3*stride+3]; idct4AVX2 below checks that before the call.
//
//go:noescape
func idct4AVX2Asm(coef *[64]int16, quant *[64]uint16, dst *byte, stride int)

func idct4AVX2(coef *[64]int16, quant *[64]uint16, dst []byte, stride int) {
	_ = dst[3*stride+3]
	idct4AVX2Asm(coef, quant, &dst[0], stride)
}

// h2v1FancyRow dispatches to h2v1FancyRowGo's interior-column loop
// (jdsample.c :295-325) in AVX2 (kernel_amd64.s) when the CPU has it;
// the two edge columns stay scalar since AVX2 buys nothing on a single
// sample. T26.
func h2v1FancyRow(dst, row []byte, dsw, w int) {
	if !useAVX2 {
		h2v1FancyRowGo(dst, row, dsw, w)
		return
	}
	v := int32(row[0])
	dst[0] = byte(v)
	dst[1] = byte((v*3 + int32(row[1]) + 2) >> 2)

	n := dsw - 2 // interior columns c = 1..dsw-2
	full := n &^ 31
	if full > 0 {
		_ = row[full+1]
		_ = dst[2+full*2-1]
		h2v1FancyRowAVX2Asm(&row[0], &row[1], &row[2], &dst[2], full)
		if full < n {
			// Redo the last 32 columns: overlap rewrites equal bytes.
			c := n - 32
			_ = row[c+33]
			_ = dst[2+n*2-1]
			h2v1FancyRowAVX2Asm(&row[c], &row[c+1], &row[c+2], &dst[2+c*2], 32)
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

// h2v1FancyRowAVX2Asm processes n (a multiple of 32) interior columns
// starting at c=1: a/b/c are row[c-1]/row[c]/row[c+1], dst is the output
// pair for c=1. h2v1FancyRow above runs the two edge columns, redoes
// the last 32 columns for a remainder, and uses Go only below 32.
//
//go:noescape
func h2v1FancyRowAVX2Asm(a, b, c, dst *byte, n int)

// h2v2FancyRow dispatches to h2v2FancyRowGo's interior-column loop
// (jdsample.c :381-429) in AVX2 (kernel_amd64.s) when the CPU has it;
// the two edge columns stay scalar. T26.
func h2v2FancyRow(dst, rowCur, rowOther []byte, dsw, w int) {
	if !useAVX2 {
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
		h2v2FancyRowAVX2Asm(&rowCur[0], &rowCur[1], &rowCur[2],
			&rowOther[0], &rowOther[1], &rowOther[2], &dst[2], full)
		if full < n {
			// Redo the last 16 columns: overlap rewrites equal bytes.
			c := n - 16
			_ = rowCur[c+17]
			_ = rowOther[c+17]
			_ = dst[2+n*2-1]
			h2v2FancyRowAVX2Asm(&rowCur[c], &rowCur[c+1], &rowCur[c+2],
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

// h2v2FancyRowAVX2Asm processes n (a multiple of 16) interior columns
// starting at c=1: curA/curB/curC are rowCur[c-1]/[c]/[c+1], likewise
// otherA/otherB/otherC for rowOther, dst is the output pair for c=1.
// h2v2FancyRow above runs the edges, redoes the last 16 columns for a
// remainder, and uses Go only below 16.
//
//go:noescape
func h2v2FancyRowAVX2Asm(curA, curB, curC, otherA, otherB, otherC, dst *byte, n int)
