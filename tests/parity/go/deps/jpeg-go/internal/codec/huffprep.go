package codec

import "math/bits"

// huffPrepGo reorders blk to zigzag. Bit k of the result is set when
// zigzag coefficient k is nonzero, and then vals[k] holds its value bits
// (v, or v-1 when negative, unmasked) and nb[k] its bit length
// (jchuff-neon.c). Other entries are left as they were.
func huffPrepGo(blk *[64]int16, vals *[64]uint16, nb *[64]uint8) uint64 {
	var mask uint64
	for k, n := range zigzagNatural {
		if v := int32(blk[n]); v != 0 {
			a := v >> 31
			vals[k] = uint16(v + a)
			nb[k] = uint8(bits.Len32(uint32((v ^ a) - a)))
			mask |= 1 << k
		}
	}
	return mask
}
