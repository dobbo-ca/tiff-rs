//go:build (arm64 || amd64) && !purego

package codec

import "os"

// noSIMD makes every kernel use its Go body, for hosts where the
// assembly is suspect. Read at init, like JP2K_NOSIMD
// (jp2k-go/internal/dsp/simd.go:9).
var noSIMD = os.Getenv("JPEG_NOSIMD") != ""

func pickKernel[F any](simd, goKernel F) F {
	if noSIMD {
		return goKernel
	}
	return simd
}
