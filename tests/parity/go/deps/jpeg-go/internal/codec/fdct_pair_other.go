//go:build purego || !amd64

package codec

const fdctPairSIMD = false

func fdctQuantPair([]byte, []byte, int, int, *fdctPairDivisors, *[64]int16, *[64]int16) {}
