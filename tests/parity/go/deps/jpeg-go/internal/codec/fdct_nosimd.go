//go:build purego || !(amd64 || arm64)

package codec

func fdctQuantRows(pix []byte, stride int, quant *[64]int32, _ *fdctDivisors, coef *[64]int16) {
	fdctQuantRowsGo(pix, stride, quant, coef)
}
