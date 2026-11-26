//go:build !purego && (amd64 || arm64)

package codec

import "os"

// encSIMD also gates on encHasSIMD: amd64 needs AVX2.
var encSIMD = os.Getenv("JPEG_NOSIMD") == "" && encHasSIMD()

// RGBAToYCbCrRow and DownsampleH2V2Row are the encoder colour kernels.
var RGBAToYCbCrRow, DownsampleH2V2Row = rgbaToYCbCrRowGo, downsampleH2V2RowGo

func init() {
	if encSIMD {
		RGBAToYCbCrRow, DownsampleH2V2Row = rgbaToYCbCrRowSIMD, downsampleH2V2RowSIMD
	}
}

// rgbaToYCbCrAsm is rgbaToYCbCrRowGo in NEON or AVX2, n%16 == 0.
//
//go:noescape
func rgbaToYCbCrAsm(src, y, cb, cr *byte, n int)

// downsampleH2V2Asm is downsampleH2V2RowGo in NEON or AVX2, n%32 == 0.
//
//go:noescape
func downsampleH2V2Asm(dst, r0, r1 *byte, n int)

func rgbaToYCbCrRowSIMD(src, y, cb, cr []byte) {
	n := len(y) &^ 15
	if n > 0 {
		_, _, _ = src[4*n-1], cb[n-1], cr[n-1]
		rgbaToYCbCrAsm(&src[0], &y[0], &cb[0], &cr[0], n)
	}
	rgbaToYCbCrRowGo(src[4*n:], y[n:], cb[n:], cr[n:])
}

// Tail starts at an even column, so bias restarts at 1.
func downsampleH2V2RowSIMD(dst, r0, r1 []byte) {
	n := len(dst) &^ 31
	if n > 0 {
		_, _ = r0[2*n-1], r1[2*n-1]
		downsampleH2V2Asm(&dst[0], &r0[0], &r1[0], n)
	}
	downsampleH2V2RowGo(dst[n:], r0[2*n:], r1[2*n:])
}
