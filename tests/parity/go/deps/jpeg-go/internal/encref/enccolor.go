// enccolor.go: encoder RGB(A)->YCbCr rows and h2v2 downsample rows, Go
// bodies for design.md section 8 kernel 4 (T23/T24).
package encref

// RGBToYCbCr is jccolor.c's rgb_ycc_convert weights (T13 spec), shared by
// the row conversion and the uniform fast path's single sample.
func RGBToYCbCr(r, g, bl int32) (y, cb, cr byte) {
	y = byte((19595*r + 38470*g + 7471*bl + 32768) >> 16)
	cb = byte((-11059*r - 21709*g + 32768*bl + (128 << 16) + 32767) >> 16)
	cr = byte((32768*r - 27439*g - 5329*bl + (128 << 16) + 32767) >> 16)
	return y, cb, cr
}

// rgbaToYCbCrRowGo converts len(y) RGB(A)-ordered 4-byte pixels, alpha
// ignored (jccolext.c rgb_ycc_convert_internal).
func rgbaToYCbCrRowGo(src, y, cb, cr []byte) {
	for i := range y {
		p := src[4*i : 4*i+4]
		y[i], cb[i], cr[i] = RGBToYCbCr(int32(p[0]), int32(p[1]), int32(p[2]))
	}
}

// downsampleH2V2RowGo is jcsample.c h2v2_downsample (:264-296) for one
// output row: dst[i] = (2x2 sum + bias)>>2, bias 1,2,1,2 from dst[0].
// r0 and r1 hold 2*len(dst) samples, already edge-padded.
func downsampleH2V2RowGo(dst, r0, r1 []byte) {
	bias := int32(1)
	for i := range dst {
		s := int32(r0[2*i]) + int32(r0[2*i+1]) + int32(r1[2*i]) + int32(r1[2*i+1])
		dst[i] = byte((s + bias) >> 2)
		bias ^= 3
	}
}
