package encref

import "math/bits"

// islow FDCT constants (jfdctint.c:81-107); FIX values are in dct.go.
const (
	fdctConstBits = 13
	fdctPass1Bits = 2
)

// fdctDescale is DESCALE(x,n), arithmetic round-shift (jdct.h:216).
func fdctDescale(x int32, n uint) int32 {
	return (x + 1<<(n-1)) >> n
}

// fdctQuantGo runs the islow FDCT (jfdctint.c:142-286) then quantizes
// (jcdctmgr.c, reciprocal multiply, verified equal to
// sign(x)*((|x|+4q)/(8q))). samples are pixel values minus 128, natural
// (row-major) order. quant is the natural-order quant table. coef
// receives quantized coefficients, natural order.
func fdctQuantGo(samples *[64]int32, quant *[64]int32, coef *[64]int16) {
	d := *samples
	fdctIslowGo(&d)
	for i, x := range d {
		coef[i] = int16(quantizeIslow(x, quant[i]))
	}
}

// fdctIslowGo is turbo's _jpeg_fdct_islow (jfdctint.c:142-286), in place.
// data is pixel values minus 128 in, the true DCT scaled up by 8 out,
// both natural (row-major) order.
func fdctIslowGo(d *[64]int32) {
	// Pass 1: rows.
	for r := 0; r < 8; r++ {
		i := r * 8
		s0, s1, s2, s3, s4, s5, s6, s7 := d[i], d[i+1], d[i+2], d[i+3], d[i+4], d[i+5], d[i+6], d[i+7]

		tmp0 := s0 + s7
		tmp7 := s0 - s7
		tmp1 := s1 + s6
		tmp6 := s1 - s6
		tmp2 := s2 + s5
		tmp5 := s2 - s5
		tmp3 := s3 + s4
		tmp4 := s3 - s4

		tmp10 := tmp0 + tmp3
		tmp13 := tmp0 - tmp3
		tmp11 := tmp1 + tmp2
		tmp12 := tmp1 - tmp2

		d[i] = (tmp10 + tmp11) << fdctPass1Bits
		d[i+4] = (tmp10 - tmp11) << fdctPass1Bits

		z1 := (tmp12 + tmp13) * fix0_541196100
		d[i+2] = fdctDescale(z1+tmp13*fix0_765366865, fdctConstBits-fdctPass1Bits)
		d[i+6] = fdctDescale(z1-tmp12*fix1_847759065, fdctConstBits-fdctPass1Bits)

		z1o := tmp4 + tmp7
		z2o := tmp5 + tmp6
		z3o := tmp4 + tmp6
		z4o := tmp5 + tmp7
		z5o := (z3o + z4o) * fix1_175875602

		t4 := tmp4 * fix0_298631336
		t5 := tmp5 * fix2_053119869
		t6 := tmp6 * fix3_072711026
		t7 := tmp7 * fix1_501321110
		z1o = -z1o * fix0_899976223
		z2o = -z2o * fix2_562915447
		z3o = -z3o*fix1_961570560 + z5o
		z4o = -z4o*fix0_390180644 + z5o

		d[i+7] = fdctDescale(t4+z1o+z3o, fdctConstBits-fdctPass1Bits)
		d[i+5] = fdctDescale(t5+z2o+z4o, fdctConstBits-fdctPass1Bits)
		d[i+3] = fdctDescale(t6+z2o+z3o, fdctConstBits-fdctPass1Bits)
		d[i+1] = fdctDescale(t7+z1o+z4o, fdctConstBits-fdctPass1Bits)
	}

	// Pass 2: columns.
	for c := 0; c < 8; c++ {
		s0, s1, s2, s3 := d[c], d[8+c], d[16+c], d[24+c]
		s4, s5, s6, s7 := d[32+c], d[40+c], d[48+c], d[56+c]

		tmp0 := s0 + s7
		tmp7 := s0 - s7
		tmp1 := s1 + s6
		tmp6 := s1 - s6
		tmp2 := s2 + s5
		tmp5 := s2 - s5
		tmp3 := s3 + s4
		tmp4 := s3 - s4

		tmp10 := tmp0 + tmp3
		tmp13 := tmp0 - tmp3
		tmp11 := tmp1 + tmp2
		tmp12 := tmp1 - tmp2

		d[c] = fdctDescale(tmp10+tmp11, fdctPass1Bits)
		d[32+c] = fdctDescale(tmp10-tmp11, fdctPass1Bits)

		z1 := (tmp12 + tmp13) * fix0_541196100
		d[16+c] = fdctDescale(z1+tmp13*fix0_765366865, fdctConstBits+fdctPass1Bits)
		d[48+c] = fdctDescale(z1-tmp12*fix1_847759065, fdctConstBits+fdctPass1Bits)

		z1o := tmp4 + tmp7
		z2o := tmp5 + tmp6
		z3o := tmp4 + tmp6
		z4o := tmp5 + tmp7
		z5o := (z3o + z4o) * fix1_175875602

		t4 := tmp4 * fix0_298631336
		t5 := tmp5 * fix2_053119869
		t6 := tmp6 * fix3_072711026
		t7 := tmp7 * fix1_501321110
		z1o = -z1o * fix0_899976223
		z2o = -z2o * fix2_562915447
		z3o = -z3o*fix1_961570560 + z5o
		z4o = -z4o*fix0_390180644 + z5o

		d[56+c] = fdctDescale(t4+z1o+z3o, fdctConstBits+fdctPass1Bits)
		d[40+c] = fdctDescale(t5+z2o+z4o, fdctConstBits+fdctPass1Bits)
		d[24+c] = fdctDescale(t6+z2o+z3o, fdctConstBits+fdctPass1Bits)
		d[8+c] = fdctDescale(t7+z1o+z4o, fdctConstBits+fdctPass1Bits)
	}
}

// FlatBlockDC is fdctQuantGo's DC output for a block of one repeated
// pixel value (T14 uniform fast path): every AC coefficient of a flat
// block's FDCT is zero, so only index 0 is needed.
func FlatBlockDC(pixel byte, quant *[64]int32) int32 {
	var samples [64]int32
	var coef [64]int16
	v := int32(pixel) - 128
	for i := range samples {
		samples[i] = v
	}
	fdctQuantGo(&samples, quant, &coef)
	return int32(coef[0])
}

// quantizeIslow is jcdctmgr.c's reciprocal-multiply quantizer, verified
// exhaustively equal to sign(x)*((|x|+4q)/(8q)).
func quantizeIslow(x, q int32) int32 {
	if x < 0 {
		return -((-x + 4*q) / (8 * q))
	}
	return (x + 4*q) / (8 * q)
}

// fdctQuantRowsGo is fdctQuantGo on 8 rows of 8 pixels at pix, stride
// apart, level-shifted by 128 (jcdctmgr.c convsamp).
func fdctQuantRowsGo(pix []byte, stride int, quant *[64]int32, coef *[64]int16) {
	var s [64]int32
	for r := 0; r < 8; r++ {
		for c, v := range pix[r*stride : r*stride+8] {
			s[r*8+c] = int32(v) - 128
		}
	}
	fdctQuantGo(&s, quant, coef)
}

// fdctDivisors is a quant table as jcdctmgr.c's reciprocal divisors,
// natural order: |x| quantizes to ((|x|+corr)*recip>>16)*scale>>16.
type fdctDivisors struct {
	recip, corr, scale [64]uint16
}

// set is jcdctmgr.c compute_reciprocal for divisor 8q; scale is
// 1<<(16-shift) so kernels shift by multiplying. q must be 1..255.
func (d *fdctDivisors) set(quant *[64]int32) {
	for i, q := range quant {
		div := uint32(q) << 3
		r := uint(bits.Len32(div)) + 15
		fq, fr := uint32(1)<<r/div, uint32(1)<<r%div
		c := div / 2
		if fr == 0 {
			fq >>= 1
			r--
		} else if fr <= div/2 {
			c++
		} else {
			fq++
		}
		d.recip[i], d.corr[i], d.scale[i] = uint16(fq), uint16(c), uint16(1)<<(32-r)
	}
}
