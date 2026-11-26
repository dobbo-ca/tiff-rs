//go:build !purego

package codec

// encColorAVX512 selects rgbaToYCbCrAsm's AVX-512 loop.
var encColorAVX512 = hasAVX512()
