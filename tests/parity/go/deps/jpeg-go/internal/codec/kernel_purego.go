//go:build !(arm64 || amd64) || purego

package codec

// idct8 is idct8Go: no SIMD kernel on this arch, or purego forced it.
func idct8(coef *Block, quant *[64]uint16, dst []byte, stride int) {
	idct8Go(coef, quant, dst, stride)
}

// IDCT8SIMD reports whether idct8 is a SIMD kernel, not idct8Go: tests
// use it to skip fixtures (evil_q255.jpg) that are only guaranteed to
// match idct8Go in range (docs/design.md section 4).
func IDCT8SIMD() bool { return false }

// idct4 is idct4Go: no SIMD kernel on this arch, or purego forced it.
func idct4(coef *Block, quant *[64]uint16, dst []byte, stride int) {
	idct4Go(coef, quant, dst, stride)
}

// IDCT4SIMD reports whether idct4 is a SIMD kernel, not idct4Go.
func IDCT4SIMD() bool { return false }

// colorRow is colorRowGo: no SIMD kernel on this arch, or purego forced it.
func colorRow(dst []byte, y, cb, cr []byte) { colorRowGo(dst, y, cb, cr) }

// h2v1FancyRow is h2v1FancyRowGo: no SIMD kernel on this arch, or purego forced it.
func h2v1FancyRow(dst, row []byte, dsw, w int) { h2v1FancyRowGo(dst, row, dsw, w) }

// h2v2FancyRow is h2v2FancyRowGo: no SIMD kernel on this arch, or purego forced it.
func h2v2FancyRow(dst, rowCur, rowOther []byte, dsw, w int) {
	h2v2FancyRowGo(dst, rowCur, rowOther, dsw, w)
}
