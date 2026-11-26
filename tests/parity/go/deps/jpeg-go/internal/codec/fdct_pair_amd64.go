//go:build !purego

package codec

// fdctPairSIMD selects fdctQuantPairAsm: two blocks per ZMM register.
var fdctPairSIMD = fdctSIMD && hasAVX512()

// fdctQuantPair is fdctQuantRows on blocks a and b at once.
func fdctQuantPair(a, b []byte, sa, sb int, div *fdctPairDivisors, ca, cb *[64]int16) {
	_, _ = a[7*sa+7], b[7*sb+7]
	fdctQuantPairAsm(&a[0], &b[0], sa, sb, div, ca, cb)
}

//go:noescape
func fdctQuantPairAsm(a, b *byte, sa, sb int, div *fdctPairDivisors, ca, cb *[64]int16)
