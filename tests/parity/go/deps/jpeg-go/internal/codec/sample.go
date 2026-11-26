// sample.go: upsampling and colour conversion, ported from turbo
// jdsample.c and jdcolor.c/jdcolext.c. T06, row path T20/P1.
package codec

import (
	"encoding/binary"
	"image"
	"sync"
)

// FIX(x) constants for YCC<->RGB, SCALEBITS=16 (jdcolor.c:82-84).
const (
	fixYCC1_40200 = 91881
	fixYCC1_77200 = 116130
	fixYCC0_71414 = 46802
	fixYCC0_34414 = 22554
	yccOneHalf    = 1 << 15
)

// crRTab, cbBTab, cbGTab, crGTab are jdcolor.c's build_ycc_rgb_table,
// indexed directly by the sample byte (x = i-128 folded in).
var crRTab, cbBTab, cbGTab, crGTab = buildYCCRGBTables()

func buildYCCRGBTables() (crR, cbB, cbG, crG [256]int32) {
	for i := 0; i < 256; i++ {
		x := int64(i) - 128
		crR[i] = int32((fixYCC1_40200*x + yccOneHalf) >> 16)
		cbB[i] = int32((fixYCC1_77200*x + yccOneHalf) >> 16)
		crG[i] = int32(-fixYCC0_71414 * x)
		cbG[i] = int32(-fixYCC0_34414*x + yccOneHalf)
	}
	return
}

// rangeLimitTable is turbo's sample_range_limit as a table lookup instead
// of branches: a plain clamp, not the wraparound post-IDCT range_limit
// table. YCbCr->RGB sums stay within
// [-384, 639] (crRTab/cbBTab/cbGTab/crGTab's magnitudes plus a byte y), so
// the +384 bias and &1023 mask never wrap.
var rangeLimitTable = buildRangeLimitTable()

func buildRangeLimitTable() (t [1024]byte) {
	for i := range t {
		v := i - 384
		switch {
		case v < 0:
			t[i] = 0
		case v > 255:
			t[i] = 255
		default:
			t[i] = byte(v)
		}
	}
	return
}

// clampByte is turbo's sample_range_limit: a plain clamp, not the
// wraparound post-IDCT range_limit table.
func clampByte(v int32) byte {
	return rangeLimitTable[uint32(v+384)&1023]
}

// clampIdx duplicates row/column 0 or n-1 for an out-of-range index; this
// is the edge context turbo gets from jdmainct.c's row duplication
// (:209-249) and jdsample.c's first/last column special cases, folded into
// one clamp since jpeg-go holds whole planes rather than a sliding window.
func clampIdx(i, n int) int {
	switch {
	case i < 0:
		return 0
	case i >= n:
		return n - 1
	default:
		return i
	}
}

// ycbcrToRGBAPixel is jdcolext.c's ycc_rgb_convert body for one sample.
func ycbcrToRGBAPixel(dst []byte, y, cb, cr byte) {
	yy := int32(y)
	dst[0] = clampByte(yy + crRTab[cr])
	dst[1] = clampByte(yy + (cbGTab[cb]+crGTab[cr])>>16)
	dst[2] = clampByte(yy + cbBTab[cb])
	dst[3] = 255
}

// colorRowGo is ycbcrToRGBAPixel over one row, one binary.LittleEndian
// PutUint32 per pixel instead of four byte stores: the row hook every
// sampling now shares (444 direct, 422/420 after row upsampling), and
// T19/T20's dispatched kernel on arm64/amd64.
func colorRowGo(dst []byte, y, cb, cr []byte) {
	for x, yv := range y {
		yy := int32(yv)
		r := clampByte(yy + crRTab[cr[x]])
		g := clampByte(yy + (cbGTab[cb[x]]+crGTab[cr[x]])>>16)
		b := clampByte(yy + cbBTab[cb[x]])
		binary.LittleEndian.PutUint32(dst[x*4:], uint32(r)|uint32(g)<<8|uint32(b)<<16|0xff000000)
	}
}

// h2v1FancySample is jdsample.c h2v1_fancy_upsample (:295-325) for output
// column x of a row downsampled 2:1 horizontally to width dsw. The
// first/last column special cases fall out of clampIdx duplicating the
// missing neighbour.
func h2v1FancySample(row []byte, dsw, x int) byte {
	c := x / 2
	v := int32(row[c]) * 3
	if x%2 == 0 {
		return byte((v + int32(row[clampIdx(c-1, dsw)]) + 1) >> 2)
	}
	return byte((v + int32(row[clampIdx(c+1, dsw)]) + 2) >> 2)
}

// h2v2FancySample is jdsample.c h2v2_fancy_upsample (:381-429) for output
// column x, given the row nearest the output (rowCur) and the adjacent
// chroma row above or below it (rowOther, already picked by the caller per
// jdmainct.c's row-group logic).
func h2v2FancySample(rowCur, rowOther []byte, dsw, x int) byte {
	colsum := func(cc int) int32 {
		cc = clampIdx(cc, dsw)
		return int32(rowCur[cc])*3 + int32(rowOther[cc])
	}
	c := x / 2
	v := colsum(c) * 3
	if x%2 == 0 {
		return byte((v + colsum(c-1) + 8) >> 4)
	}
	return byte((v + colsum(c+1) + 7) >> 4)
}

// h2v1FancyRowGo is jdsample.c h2v1_fancy_upsample (:295-325) for a whole
// row: one sliding "invalue*3" per input column instead of h2v1FancySample
// recomputing it for both outputs. dst must have room for w bytes, w being
// 2*dsw or 2*dsw-1; dsw must be >= 2 (the decoder's fancy path gates on
// dsw > 2; H2V1FancyRow also takes dsw == 2). h2v1FancyRow
// dispatches to this or T25's NEON kernel.
func h2v1FancyRowGo(dst, row []byte, dsw, w int) {
	v := int32(row[0])
	dst[0] = byte(v)
	dst[1] = byte((v*3 + int32(row[1]) + 2) >> 2)
	o := 2
	for c := 1; c < dsw-1; c++ {
		v = int32(row[c]) * 3
		dst[o] = byte((v + int32(row[c-1]) + 1) >> 2)
		dst[o+1] = byte((v + int32(row[c+1]) + 2) >> 2)
		o += 2
	}
	v = int32(row[dsw-1])
	dst[o] = byte((v*3 + int32(row[dsw-2]) + 1) >> 2)
	if o+1 < w {
		dst[o+1] = byte(v)
	}
}

// h2v2FancyRowGo is jdsample.c h2v2_fancy_upsample (:381-429) for a whole
// row: one sliding column sum per input column, carried as
// lastcolsum/thiscolsum/nextcolsum, instead of h2v2FancySample
// recomputing colsum from scratch for every output. dsw must be >= 2, as
// for h2v1FancyRowGo.
// h2v2FancyRow dispatches to this or T25's NEON kernel.
func h2v2FancyRowGo(dst, rowCur, rowOther []byte, dsw, w int) {
	colsum := func(c int) int32 { return int32(rowCur[c])*3 + int32(rowOther[c]) }
	thiscolsum := colsum(0)
	nextcolsum := colsum(1)
	dst[0] = byte((thiscolsum*4 + 8) >> 4)
	dst[1] = byte((thiscolsum*3 + nextcolsum + 7) >> 4)
	lastcolsum := thiscolsum
	thiscolsum = nextcolsum
	o := 2
	for c := 1; c < dsw-1; c++ {
		nextcolsum = colsum(c + 1)
		dst[o] = byte((thiscolsum*3 + lastcolsum + 8) >> 4)
		dst[o+1] = byte((thiscolsum*3 + nextcolsum + 7) >> 4)
		lastcolsum = thiscolsum
		thiscolsum = nextcolsum
		o += 2
	}
	dst[o] = byte((thiscolsum*3 + lastcolsum + 8) >> 4)
	if o+1 < w {
		dst[o+1] = byte((thiscolsum*4 + 7) >> 4)
	}
}

// boxRow is jdsample.c h2v1/h2v2_upsample's row body: plain duplication of
// each downsampled column into two output columns.
func boxRow(dst, row []byte, dsw, w int) {
	o := 0
	for c := 0; c < dsw && o < w; c++ {
		dst[o] = row[c]
		o++
		if o < w {
			dst[o] = row[c]
			o++
		}
	}
}

// upsampleRowH fills dst (w bytes) from one downsampled row (h2v1: chroma
// at Y's height, half Y's width).
func upsampleRowH(dst, row []byte, dsw, w int, fancy bool) {
	if fancy {
		h2v1FancyRow(dst, row, dsw, w)
	} else {
		boxRow(dst, row, dsw, w)
	}
}

// upsampleRowV fills dst (w bytes) from the two downsampled rows h2v2
// needs (chroma at half Y's width and height); rowOther is the box
// filter's don't-care when !fancy.
func upsampleRowV(dst, rowCur, rowOther []byte, dsw, w int, fancy bool) {
	if fancy {
		h2v2FancyRow(dst, rowCur, rowOther, dsw, w)
	} else {
		boxRow(dst, rowCur, dsw, w)
	}
}

// h1v2FancyRow is jdsample.c h1v2_fancy_upsample for one output row: 3/4 of
// the nearest chroma row plus 1/4 of the one beside it. bias is 1 for the
// even output row (neighbour above) and 2 for the odd (below).
func h1v2FancyRow(dst, cur, other []byte, bias int32) {
	for x := range cur {
		dst[x] = byte((int32(cur[x])*3 + int32(other[x]) + bias) >> 2)
	}
}

// intRow is jdsample.c int_upsample's row body: each downsampled column
// repeated hExp times, the 4:1:1 and 4:1:0 case.
func intRow(dst, row []byte, dsw, w, hExp int) {
	o := 0
	for c := 0; c < dsw; c++ {
		for k := 0; k < hExp && o < w; k++ {
			dst[o] = row[c]
			o++
		}
	}
}

// chromaDims is jdmaster.c's downsampled_width/height at scale 1
// (docs/design.md section 4), i.e. ceil(dim/n) when subsampled by n.
func chromaDims(w, h int, ratio image.YCbCrSubsampleRatio) (int, int) {
	switch ratio {
	case image.YCbCrSubsampleRatio422:
		return (w + 1) / 2, h
	case image.YCbCrSubsampleRatio420:
		return (w + 1) / 2, (h + 1) / 2
	case image.YCbCrSubsampleRatio440:
		return w, (h + 1) / 2
	case image.YCbCrSubsampleRatio411:
		return (w + 3) / 4, h
	case image.YCbCrSubsampleRatio410:
		return (w + 3) / 4, (h + 1) / 2
	default: // 444
		return w, h
	}
}

// v2Rows returns the chroma row indices h2v2 needs for output row y: the
// nearest row and the row above (y even, "v==0") or below (y odd) it,
// clamped to the plane's real height (jdsample.c h2v2_fancy_upsample's
// alternating v loop, :381-429).
func v2Rows(y, dsh int) (cur, other int) {
	p := y / 2
	other = p - 1
	if y%2 == 1 {
		other = p + 1
	}
	return clampIdx(p, dsh), clampIdx(other, dsh)
}

// YCbCrToRGBAGo is the default Recon.ToRGBA: fancy (or box, for a
// downsampled_width <= 2 component) upsampling plus YCbCr->RGB colour
// conversion, matching jdsample.c and jdcolext.c's ycc_extrgbx_convert.
// src keeps block-padded strides; only src.Rect's samples are read. Scratch
// rows are allocated here; callers on a hot path (jpeg.Decoder) call
// YCbCrUpsampleConvertGo directly with rows they hold across calls.
func YCbCrToRGBAGo(dst *image.RGBA, src *image.YCbCr) {
	w, h := src.Rect.Dx(), src.Rect.Dy()
	dsw, dsh := chromaDims(w, h, src.SubsampleRatio)
	hExp, vExp := 1, 1
	switch src.SubsampleRatio {
	case image.YCbCrSubsampleRatio422:
		hExp = 2
	case image.YCbCrSubsampleRatio420:
		hExp, vExp = 2, 2
	case image.YCbCrSubsampleRatio440:
		vExp = 2
	case image.YCbCrSubsampleRatio411:
		hExp = 4
	case image.YCbCrSubsampleRatio410:
		hExp, vExp = 4, 2
	}
	cbRow, crRow := make([]byte, w), make([]byte, w)
	YCbCrUpsampleConvertGo(dst, src.Y, src.YStride, src.Cb, src.Cr, src.CStride, w, h, dsw, dsh,
		hExp, vExp, true, cbRow, crRow)
}

// YCbCrUpsampleConvertGo is YCbCrToRGBAGo's body, parameterized by the
// chroma plane's real pixel size instead of a fixed image.YCbCr ratio: at
// Scale 2/4/8 (T07) a component's own IDCT scaling can make dsw/dsh equal
// w/h (no upsampling needed, jdmaster.c:290-305), which chromaDims's
// scale-1 halving cannot express. hExp/vExp are turbo's h_expand and
// v_expand: a 1-row 4:2:0 image has dsh == h yet still takes h2v2
// with its one chroma row as both neighbours. allowFancy is
// do_fancy_upsampling && min_DCT_scaled_size > 1 (jdsample.c:478): Scale 8
// (minSize 1) always uses box sampling, whatever dsw is.
//
// cbRow and crRow are full-width (>= w) scratch rows the caller owns
// across calls, so a reused Decoder allocates nothing here (docs/design.md
// P1 spec); every sampling upsamples (or, at 4:4:4, just aliases) into
// them and then converts colour through the one colorRow hook (dispatched
// to T19's SIMD kernel), instead of a per-pixel loop that only used
// colorRow for 4:4:4.
func YCbCrUpsampleConvertGo(dst *image.RGBA, yPix []byte, yStride int, cb, cr []byte, cStride int, w, h, dsw, dsh int, hExp, vExp int, allowFancy bool, cbRow, crRow []byte) {
	ycbcrRows(dst, yPix, yStride, cb, cr, cStride, w, 0, h, dsw, dsh, hExp, vExp, allowFancy, cbRow, crRow)
}

// YCbCrUpsampleConvertN is YCbCrUpsampleConvertGo on up to workers
// goroutines, each converting a stripe of rows with its own chroma rows.
// An output row reads only the finished planes, so stripes are exact.
func YCbCrUpsampleConvertN(dst *image.RGBA, yPix []byte, yStride int, cb, cr []byte, cStride int, w, h, dsw, dsh int, hExp, vExp int, allowFancy bool, cbRow, crRow []byte, workers int) {
	k := min(workers, h/16)
	var wg sync.WaitGroup
	for i := 1; i < k; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			ycbcrRows(dst, yPix, yStride, cb, cr, cStride, w, h*i/k, h*(i+1)/k, dsw, dsh, hExp, vExp, allowFancy, make([]byte, w), make([]byte, w))
		}()
	}
	ycbcrRows(dst, yPix, yStride, cb, cr, cStride, w, 0, h/max(k, 1), dsw, dsh, hExp, vExp, allowFancy, cbRow, crRow)
	wg.Wait()
}

func ycbcrRows(dst *image.RGBA, yPix []byte, yStride int, cb, cr []byte, cStride int, w, y0, y1, dsw, dsh int, hExp, vExp int, allowFancy bool, cbRow, crRow []byte) {
	fancy := allowFancy && dsw > 2 // h2 only; h1v2 fancy has no width test
	cbRow, crRow = cbRow[:w], crRow[:w]

	for y := y0; y < y1; y++ {
		yRow := yPix[y*yStride : y*yStride+w]
		outRow := dst.Pix[y*dst.Stride : y*dst.Stride+w*4]

		switch {
		case hExp == 1 && vExp == 1:
			cbSrc, crSrc := cb[y*cStride:y*cStride+w], cr[y*cStride:y*cStride+w]
			colorRow(outRow, yRow, cbSrc, crSrc)
		case hExp == 2 && vExp == 1:
			cbSrc, crSrc := cb[y*cStride:y*cStride+dsw], cr[y*cStride:y*cStride+dsw]
			upsampleRowH(cbRow, cbSrc, dsw, w, fancy)
			upsampleRowH(crRow, crSrc, dsw, w, fancy)
			colorRow(outRow, yRow, cbRow, crRow)
		case hExp == 2 && vExp == 2:
			cur, other := v2Rows(y, dsh)
			cbCur, cbOther := cb[cur*cStride:cur*cStride+dsw], cb[other*cStride:other*cStride+dsw]
			crCur, crOther := cr[cur*cStride:cur*cStride+dsw], cr[other*cStride:other*cStride+dsw]
			upsampleRowV(cbRow, cbCur, cbOther, dsw, w, fancy)
			upsampleRowV(crRow, crCur, crOther, dsw, w, fancy)
			colorRow(outRow, yRow, cbRow, crRow)
		case hExp == 1 && vExp == 2:
			cur, other := v2Rows(y, dsh)
			cbCur, crCur := cb[cur*cStride:cur*cStride+w], cr[cur*cStride:cur*cStride+w]
			if allowFancy {
				h1v2FancyRow(cbRow, cbCur, cb[other*cStride:other*cStride+w], int32(1+y&1))
				h1v2FancyRow(crRow, crCur, cr[other*cStride:other*cStride+w], int32(1+y&1))
				cbCur, crCur = cbRow, crRow
			}
			colorRow(outRow, yRow, cbCur, crCur)
		default: // int_upsample: 4:1:1, 4:1:0 and their scaled forms
			r := y / vExp
			intRow(cbRow, cb[r*cStride:r*cStride+dsw], dsw, w, hExp)
			intRow(crRow, cr[r*cStride:r*cStride+dsw], dsw, w, hExp)
			colorRow(outRow, yRow, cbRow, crRow)
		}
	}
}

// RGBToRGBAGo is jdcolext.c rgb_rgb_convert: a null colour convert for
// Adobe/ids-RGB streams, alpha forced opaque.
func RGBToRGBAGo(dst *image.RGBA, r, g, b []byte, stride, w, h int) {
	for y := 0; y < h; y++ {
		rRow, gRow, bRow := r[y*stride:], g[y*stride:], b[y*stride:]
		outRow := dst.Pix[y*dst.Stride:]
		for x := 0; x < w; x++ {
			o := outRow[x*4:]
			o[0], o[1], o[2], o[3] = rRow[x], gRow[x], bRow[x], 255
		}
	}
}

// GrayToRGBAGo is jdcolext.c gray_rgb_convert: replicate Y into R, G, B.
func GrayToRGBAGo(dst *image.RGBA, gray []byte, stride, w, h int) {
	for y := 0; y < h; y++ {
		grayRow := gray[y*stride:]
		outRow := dst.Pix[y*dst.Stride:]
		for x := 0; x < w; x++ {
			o := outRow[x*4:]
			v := grayRow[x]
			o[0], o[1], o[2], o[3] = v, v, v, 255
		}
	}
}
