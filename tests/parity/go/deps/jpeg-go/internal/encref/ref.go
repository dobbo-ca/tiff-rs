// Package encref: frozen v0.4.0 Go encoder.
package encref

// FIX_* constants, jidctint.c:118-129.
const (
	fix0_298631336 = 2446
	fix0_390180644 = 3196
	fix0_541196100 = 4433
	fix0_765366865 = 6270
	fix0_899976223 = 7373
	fix1_175875602 = 9633
	fix1_501321110 = 12299
	fix1_847759065 = 15137
	fix1_961570560 = 16069
	fix2_053119869 = 16819
	fix2_562915447 = 20995
	fix3_072711026 = 25172
)

func ceilDiv(a, b int) int { return (a + b - 1) / b }
