//go:build !purego && (amd64 || arm64)

package codec

import "os"

// huffPrepSIMD is read at init; JPEG_NOSIMD selects the Go body. The
// amd64 kernel is VEX-encoded.
var huffPrepSIMD = os.Getenv("JPEG_NOSIMD") == "" && encHasSIMD()

func huffPrep(blk *[64]int16, vals *[64]uint16, nb *[64]uint8) uint64 {
	if huffPrepSIMD {
		return huffPrepAsm(blk, vals, nb)
	}
	return huffPrepGo(blk, vals, nb)
}

// huffPrepAsm is huffPrepGo in SSE2 or NEON.
//
//go:noescape
func huffPrepAsm(blk *[64]int16, vals *[64]uint16, nb *[64]uint8) uint64
