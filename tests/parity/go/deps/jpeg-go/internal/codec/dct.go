// Package codec: islow IDCT, ported from turbo jidctint.c and jidctred.c.
package codec

import "encoding/binary"

// CONST_BITS/PASS1_BITS for 8-bit samples (jidctint.c:103-104).
const (
	constBits = 13
	pass1Bits = 2
)

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

// FIX_* constants for the reduced sizes, jidctred.c:61-74.
const (
	fix0_211164243 = 1730
	fix0_509795579 = 4176
	fix0_601344887 = 4926
	fix0_720959822 = 5906
	fix0_850430095 = 6967
	fix1_061594337 = 8697
	fix1_272758580 = 10426
	fix1_451774981 = 11893
	fix2_172734803 = 17799
	fix3_624509785 = 29692
)

// rangeLimit mirrors turbo's post-IDCT range-limit table, indexed by
// x&1023 so out-of-range coefficients wrap the way C does (jdmaster.c).
var rangeLimit = buildRangeLimit()

func buildRangeLimit() [1024]byte {
	var t [1024]byte
	for k := 0; k < 1024; k++ {
		switch {
		case k < 128:
			t[k] = byte(k + 128)
		case k < 512:
			t[k] = 255
		case k < 896:
			t[k] = 0
		default:
			t[k] = byte(k - 896)
		}
	}
	return t
}

// dequantize matches ISLOW_MULT_TYPE=int dequantization: a 32-bit
// truncating multiply, widened to int64 for the caller.
func dequantize(coef int16, quant uint16) int64 {
	return int64(int32(coef) * int32(quant))
}

// descale is DESCALE(x, n): round and arithmetic-shift a JLONG value.
func descale(x int64, n uint) int64 {
	return (x + (1 << (n - 1))) >> n
}

// idct8Go ports _jpeg_idct_islow (jidctint.c:218-434). coef is natural
// order; dst gets 8x8 samples at stride.
func idct8Go(coef *[64]int16, quant *[64]uint16, dst []byte, stride int) {
	var ws [64]int32

	// Pass 1: columns.
	for col := 0; col < 8; col++ {
		if coef[8+col] == 0 && coef[16+col] == 0 && coef[24+col] == 0 &&
			coef[32+col] == 0 && coef[40+col] == 0 && coef[48+col] == 0 &&
			coef[56+col] == 0 {
			dc := int32(dequantize(coef[col], quant[col]) << pass1Bits)
			ws[col] = dc
			ws[8+col] = dc
			ws[16+col] = dc
			ws[24+col] = dc
			ws[32+col] = dc
			ws[40+col] = dc
			ws[48+col] = dc
			ws[56+col] = dc
			continue
		}

		z2 := dequantize(coef[16+col], quant[16+col])
		z3 := dequantize(coef[48+col], quant[48+col])

		z1 := (z2 + z3) * fix0_541196100
		tmp2 := z1 + z3*(-fix1_847759065)
		tmp3 := z1 + z2*fix0_765366865

		z2 = dequantize(coef[col], quant[col])
		z3 = dequantize(coef[32+col], quant[32+col])

		tmp0 := (z2 + z3) << constBits
		tmp1 := (z2 - z3) << constBits

		tmp10 := tmp0 + tmp3
		tmp13 := tmp0 - tmp3
		tmp11 := tmp1 + tmp2
		tmp12 := tmp1 - tmp2

		tmp0 = dequantize(coef[56+col], quant[56+col])
		tmp1 = dequantize(coef[40+col], quant[40+col])
		tmp2 = dequantize(coef[24+col], quant[24+col])
		tmp3 = dequantize(coef[8+col], quant[8+col])

		z1 = tmp0 + tmp3
		z2 = tmp1 + tmp2
		z3 = tmp0 + tmp2
		z4 := tmp1 + tmp3
		z5 := (z3 + z4) * fix1_175875602

		tmp0 = tmp0 * fix0_298631336
		tmp1 = tmp1 * fix2_053119869
		tmp2 = tmp2 * fix3_072711026
		tmp3 = tmp3 * fix1_501321110
		z1 = z1 * (-fix0_899976223)
		z2 = z2 * (-fix2_562915447)
		z3 = z3 * (-fix1_961570560)
		z4 = z4 * (-fix0_390180644)

		z3 += z5
		z4 += z5

		tmp0 += z1 + z3
		tmp1 += z2 + z4
		tmp2 += z2 + z3
		tmp3 += z1 + z4

		ws[col] = int32(descale(tmp10+tmp3, constBits-pass1Bits))
		ws[56+col] = int32(descale(tmp10-tmp3, constBits-pass1Bits))
		ws[8+col] = int32(descale(tmp11+tmp2, constBits-pass1Bits))
		ws[48+col] = int32(descale(tmp11-tmp2, constBits-pass1Bits))
		ws[16+col] = int32(descale(tmp12+tmp1, constBits-pass1Bits))
		ws[40+col] = int32(descale(tmp12-tmp1, constBits-pass1Bits))
		ws[24+col] = int32(descale(tmp13+tmp0, constBits-pass1Bits))
		ws[32+col] = int32(descale(tmp13-tmp0, constBits-pass1Bits))
	}

	// Pass 2: rows.
	for row := 0; row < 8; row++ {
		base := 8 * row
		out := dst[row*stride:]

		if ws[base+1] == 0 && ws[base+2] == 0 && ws[base+3] == 0 &&
			ws[base+4] == 0 && ws[base+5] == 0 && ws[base+6] == 0 &&
			ws[base+7] == 0 {
			dc := rangeLimit[int32(descale(int64(ws[base]), pass1Bits+3))&1023]
			for i := 0; i < 8; i++ {
				out[i] = dc
			}
			continue
		}

		z2 := int64(ws[base+2])
		z3 := int64(ws[base+6])

		z1 := (z2 + z3) * fix0_541196100
		tmp2 := z1 + z3*(-fix1_847759065)
		tmp3 := z1 + z2*fix0_765366865

		tmp0 := (int64(ws[base]) + int64(ws[base+4])) << constBits
		tmp1 := (int64(ws[base]) - int64(ws[base+4])) << constBits

		tmp10 := tmp0 + tmp3
		tmp13 := tmp0 - tmp3
		tmp11 := tmp1 + tmp2
		tmp12 := tmp1 - tmp2

		tmp0 = int64(ws[base+7])
		tmp1 = int64(ws[base+5])
		tmp2 = int64(ws[base+3])
		tmp3 = int64(ws[base+1])

		z1 = tmp0 + tmp3
		z2 = tmp1 + tmp2
		z3 = tmp0 + tmp2
		z4 := tmp1 + tmp3
		z5 := (z3 + z4) * fix1_175875602

		tmp0 = tmp0 * fix0_298631336
		tmp1 = tmp1 * fix2_053119869
		tmp2 = tmp2 * fix3_072711026
		tmp3 = tmp3 * fix1_501321110
		z1 = z1 * (-fix0_899976223)
		z2 = z2 * (-fix2_562915447)
		z3 = z3 * (-fix1_961570560)
		z4 = z4 * (-fix0_390180644)

		z3 += z5
		z4 += z5

		tmp0 += z1 + z3
		tmp1 += z2 + z4
		tmp2 += z2 + z3
		tmp3 += z1 + z4

		const shift = constBits + pass1Bits + 3
		out[0] = rangeLimit[int32(descale(tmp10+tmp3, shift))&1023]
		out[7] = rangeLimit[int32(descale(tmp10-tmp3, shift))&1023]
		out[1] = rangeLimit[int32(descale(tmp11+tmp2, shift))&1023]
		out[6] = rangeLimit[int32(descale(tmp11-tmp2, shift))&1023]
		out[2] = rangeLimit[int32(descale(tmp12+tmp1, shift))&1023]
		out[5] = rangeLimit[int32(descale(tmp12-tmp1, shift))&1023]
		out[3] = rangeLimit[int32(descale(tmp13+tmp0, shift))&1023]
		out[4] = rangeLimit[int32(descale(tmp13-tmp0, shift))&1023]
	}
}

// idct4Go ports _jpeg_idct_4x4 (jidctred.c:143-293). Column 4 is unused
// by the reduced output and is skipped, per the source.
func idct4Go(coef *[64]int16, quant *[64]uint16, dst []byte, stride int) {
	var ws [64]int32

	for col := 0; col < 8; col++ {
		if col == 4 {
			continue
		}
		if coef[8+col] == 0 && coef[16+col] == 0 && coef[24+col] == 0 &&
			coef[40+col] == 0 && coef[48+col] == 0 && coef[56+col] == 0 {
			dc := int32(dequantize(coef[col], quant[col]) << pass1Bits)
			ws[col] = dc
			ws[8+col] = dc
			ws[16+col] = dc
			ws[24+col] = dc
			continue
		}

		tmp0 := dequantize(coef[col], quant[col]) << (constBits + 1)

		z2 := dequantize(coef[16+col], quant[16+col])
		z3 := dequantize(coef[48+col], quant[48+col])
		tmp2 := z2*fix1_847759065 + z3*(-fix0_765366865)

		tmp10 := tmp0 + tmp2
		tmp12 := tmp0 - tmp2

		z1 := dequantize(coef[56+col], quant[56+col])
		z2 = dequantize(coef[40+col], quant[40+col])
		z3 = dequantize(coef[24+col], quant[24+col])
		z4 := dequantize(coef[8+col], quant[8+col])

		oddA := z1*(-fix0_211164243) + z2*fix1_451774981 + z3*(-fix2_172734803) + z4*fix1_061594337
		oddB := z1*(-fix0_509795579) + z2*(-fix0_601344887) + z3*fix0_899976223 + z4*fix2_562915447

		const shift = constBits - pass1Bits + 1
		ws[col] = int32(descale(tmp10+oddB, shift))
		ws[24+col] = int32(descale(tmp10-oddB, shift))
		ws[8+col] = int32(descale(tmp12+oddA, shift))
		ws[16+col] = int32(descale(tmp12-oddA, shift))
	}

	for row := 0; row < 4; row++ {
		base := 8 * row
		out := dst[row*stride:]

		if ws[base+1] == 0 && ws[base+2] == 0 && ws[base+3] == 0 &&
			ws[base+5] == 0 && ws[base+6] == 0 && ws[base+7] == 0 {
			dc := rangeLimit[int32(descale(int64(ws[base]), pass1Bits+3))&1023]
			out[0], out[1], out[2], out[3] = dc, dc, dc, dc
			continue
		}

		tmp0 := int64(ws[base]) << (constBits + 1)
		tmp2 := int64(ws[base+2])*fix1_847759065 + int64(ws[base+6])*(-fix0_765366865)
		tmp10 := tmp0 + tmp2
		tmp12 := tmp0 - tmp2

		z1 := int64(ws[base+7])
		z2 := int64(ws[base+5])
		z3 := int64(ws[base+3])
		z4 := int64(ws[base+1])

		oddA := z1*(-fix0_211164243) + z2*fix1_451774981 + z3*(-fix2_172734803) + z4*fix1_061594337
		oddB := z1*(-fix0_509795579) + z2*(-fix0_601344887) + z3*fix0_899976223 + z4*fix2_562915447

		const shift = constBits + pass1Bits + 3 + 1
		out[0] = rangeLimit[int32(descale(tmp10+oddB, shift))&1023]
		out[3] = rangeLimit[int32(descale(tmp10-oddB, shift))&1023]
		out[1] = rangeLimit[int32(descale(tmp12+oddA, shift))&1023]
		out[2] = rangeLimit[int32(descale(tmp12-oddA, shift))&1023]
	}
}

// idct2Go ports _jpeg_idct_2x2 (jidctred.c:302-405). Only rows/cols
// 0,1,3,5,7 are used, per the source.
func idct2Go(coef *[64]int16, quant *[64]uint16, dst []byte, stride int) {
	var ws [16]int32

	for col := 0; col < 8; col++ {
		if col == 2 || col == 4 || col == 6 {
			continue
		}
		if coef[8+col] == 0 && coef[24+col] == 0 && coef[40+col] == 0 && coef[56+col] == 0 {
			dc := int32(dequantize(coef[col], quant[col]) << pass1Bits)
			ws[col] = dc
			ws[8+col] = dc
			continue
		}

		tmp10 := dequantize(coef[col], quant[col]) << (constBits + 2)

		tmp0 := dequantize(coef[56+col], quant[56+col]) * (-fix0_720959822)
		tmp0 += dequantize(coef[40+col], quant[40+col]) * fix0_850430095
		tmp0 += dequantize(coef[24+col], quant[24+col]) * (-fix1_272758580)
		tmp0 += dequantize(coef[8+col], quant[8+col]) * fix3_624509785

		const shift = constBits - pass1Bits + 2
		ws[col] = int32(descale(tmp10+tmp0, shift))
		ws[8+col] = int32(descale(tmp10-tmp0, shift))
	}

	for row := 0; row < 2; row++ {
		base := 8 * row
		out := dst[row*stride:]

		if ws[base+1] == 0 && ws[base+3] == 0 && ws[base+5] == 0 && ws[base+7] == 0 {
			dc := rangeLimit[int32(descale(int64(ws[base]), pass1Bits+3))&1023]
			out[0], out[1] = dc, dc
			continue
		}

		tmp10 := int64(ws[base]) << (constBits + 2)
		tmp0 := int64(ws[base+7])*(-fix0_720959822) + int64(ws[base+5])*fix0_850430095 +
			int64(ws[base+3])*(-fix1_272758580) + int64(ws[base+1])*fix3_624509785

		const shift = constBits + pass1Bits + 3 + 2
		out[0] = rangeLimit[int32(descale(tmp10+tmp0, shift))&1023]
		out[1] = rangeLimit[int32(descale(tmp10-tmp0, shift))&1023]
	}
}

// idct1Go ports _jpeg_idct_1x1 (jidctred.c:414-432): the DC average.
func idct1Go(coef *[64]int16, quant *[64]uint16, dst []byte, stride int) {
	dc := int32(descale(dequantize(coef[0], quant[0]), 3))
	dst[0] = rangeLimit[dc&1023]
}

// dcOnly reports whether coef has no AC coefficients.
func dcOnly(coef *Block) bool {
	for _, c := range coef[1:] {
		if c != 0 {
			return false
		}
	}
	return true
}

// idctDCConst is idct8/4/2Go's output for a DC-only block.
func idctDCConst(dc int16, quant uint16) byte {
	v := int32(dequantize(dc, quant) << pass1Bits) // pass 1 stores int32
	return rangeLimit[int32(descale(int64(v), pass1Bits+3))&1023]
}

// fillConst writes a bs x bs block of v at stride. bs 8 (the unscaled
// case, dominant on real tiles) stores one word per row instead of 8
// byte stores.
func fillConst(bs int, dst []byte, stride int, v byte) {
	if bs == 8 {
		w := uint64(v) * 0x0101010101010101
		for r := 0; r < 8; r++ {
			binary.LittleEndian.PutUint64(dst[r*stride:], w)
		}
		return
	}
	for r := 0; r < bs; r++ {
		row := dst[r*stride : r*stride+bs]
		for i := range row {
			row[i] = v
		}
	}
}
