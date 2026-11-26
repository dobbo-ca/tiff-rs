//go:build !purego && (amd64 || arm64)

package codec

import "os"

// fdctSIMD selects fdctQuantAsm; JPEG_NOSIMD is read once, at init.
var fdctSIMD = os.Getenv("JPEG_NOSIMD") == "" && encHasSIMD()

// fdctQuantRows is fdctQuantRowsGo, in NEON or AVX2 when fdctSIMD.
func fdctQuantRows(pix []byte, stride int, quant *[64]int32, div *fdctDivisors, coef *[64]int16) {
	if !fdctSIMD {
		fdctQuantRowsGo(pix, stride, quant, coef)
		return
	}
	_ = pix[7*stride+7]
	fdctQuantAsm(&pix[0], stride, div, coef)
}

//go:noescape
func fdctQuantAsm(pix *byte, stride int, div *fdctDivisors, coef *[64]int16)
