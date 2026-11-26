//go:build !purego

package codec

// encHasSIMD reports AVX2, via the decoder's check.
func encHasSIMD() bool { return hasAVX2() }
