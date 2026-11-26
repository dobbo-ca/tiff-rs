// Package jpeg implements github.com/dobbo-ca/jpeg-go, which
// re-exports its API. See docs/design.md.
package jpeg

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"image"
	"sync"

	"github.com/dobbo-ca/jpeg-go/internal/codec"
)

var (
	// ErrUnsupported is returned for streams or options out of scope; see
	// docs/design.md section 1.
	ErrUnsupported = codec.ErrUnsupported
	// ErrTruncated wraps a decode error from entropy data that ran out,
	// held a Huffman code no table matches, or whose restart markers were
	// out of sequence (turbo's warnings). The image still comes back.
	ErrTruncated = codec.ErrTruncated
	// ErrAfterScan wraps a fatal marker after a single-scan frame's last
	// row. Decode still returns the image, as libjpeg-turbo has filled
	// every row before jpeg_finish_decompress fails. The error also wraps
	// its cause (ErrUnsupported for a SOFn, say) and ErrTruncated if the
	// scan lost data, so check ErrAfterScan first to keep the image.
	ErrAfterScan = codec.ErrAfterScan
)

// Format selects Decoder's output type; see docs/design.md section 2.
type Format uint8

const (
	FormatYCbCr Format = iota // *image.YCbCr at the stream's sampling; *image.Gray for 1 component
	FormatRGBA                // *image.RGBA, alpha 255 (T06)
	FormatGray                // *image.Gray: Y for YCbCr streams, turbo's rgb_gray for RGB streams
)

// Recon replaces reconstruction steps so a caller can match another
// library bit for bit; see docs/design.md section 5.
type Recon struct {
	IDCT   func(dst []byte, stride int, coef *[64]int32)
	ToRGBA func(dst *image.RGBA, src *image.YCbCr)
}

// DecodeOptions configures Decoder.Decode; see docs/design.md section 2.
// Scale must be 0, 1, 2, 4 or 8.
type DecodeOptions struct {
	Format  Format
	Scale   int
	Region  image.Rectangle
	Tables  []byte
	Restart []int // trusted when RST(k-1) precedes offset k
	Recon   Recon
	// CheckFrame, with Region, entropy-decodes every MCU of the frame,
	// not just the Region's, so the error is a whole-frame Decode's.
	CheckFrame bool
	// Owned, with FormatRGBA, decodes into a new *image.RGBA the
	// Decoder does not keep, so package Decode returns it uncopied.
	Owned bool
	// Dst, with FormatRGBA at Scale 1 and no Region, Recon or Owned,
	// receives the image: Decode returns Dst and writes only its first
	// Rect.Dx() pixels of each of Rect.Dy() rows, at Dst.Stride. Dst must
	// match the frame's size, with Stride >= 4*width and Pix reaching the
	// last row; else ErrUnsupported, before any write. The
	// Decoder does not keep Dst, and package Decode does not copy it.
	Dst *image.RGBA
	// Workers above 1 lets a whole-image decode use up to Workers
	// goroutines, counting the caller's: restart-interval bands at Scale
	// 1 with a DRI, and colour conversion in row stripes. Output and
	// errors do not change.
	Workers int
	// Limits bounds this call, checked before any allocation; else ErrLimit.
	Limits Limits
	// Reserved says the caller already reserved this call's bytes from its
	// own budget: the process-wide limiter is skipped, so nothing counts
	// twice. Limits.MaxBytes and the 1 GiB cap still apply.
	Reserved bool
}

// Decoder keeps its buffers between calls; the zero value is ready. The
// image returned by Decode aliases these buffers until the next call.
type Decoder struct {
	frame       codec.Frame
	tablesFrame codec.Frame // ParseTables output only; copied into frame every call
	haveTables  bool
	tablesBuf   []byte

	planes [3]codec.Plane
	band   codec.Band // Region's MCU-row band (T08); reused so it never escapes to the heap

	coefs Coefficients // DecodeCoefficients' result, reused

	ycbcrImg image.YCbCr
	grayImg  image.Gray
	rgbaImg  image.RGBA

	cbRow, crRow []byte // upsampled chroma row scratch for YCbCrUpsampleConvertGo (P1)

	limWait  limWaiter // memory-limiter FIFO node (T09); lives here so re-waiting allocates nothing
	cloneOut bool      // package Decode will copy the image: need counts the copy
	lastNeed int64     // last reserve() estimate, for TestEstimateCoversAlloc

	// *Crop hold a Region's view onto the *Img buffers above (T08): a
	// separate struct so cropping never reslices Pix in place, which
	// would shrink *Img's own capacity and defeat reuse on the next call.
	ycbcrCrop image.YCbCr
	grayCrop  image.Gray
	rgbaCrop  image.RGBA
	padPix    []byte // Region partly outside the image
}

// ensureTables reparses tables into d.tablesFrame only when they differ
// from the last call's (docs/design.md T05 spec), so a Decoder decoding
// many tiles that share an abbreviated tables stream skips the reparse.
// Decode copies d.tablesFrame into d.frame on every call: a Frame carries
// no slices or pointers, so the copy is cheap and keeps a src's own
// DQT/DHT (written into d.frame by ParseFrame) from leaking into the next
// call, even when Tables is byte-identical or nil both times.
func (d *Decoder) ensureTables(tables []byte) error {
	if d.haveTables && bytes.Equal(tables, d.tablesBuf) {
		return nil
	}
	d.tablesFrame = codec.Frame{}
	if tables != nil {
		if err := d.tablesFrame.ParseTables(tables); err != nil {
			d.haveTables = false
			return err
		}
	}
	d.tablesBuf = append(d.tablesBuf[:0], tables...)
	d.haveTables = true
	return nil
}

// Decode decodes src. The image aliases d's buffers until d's next call.
// Truncated entropy data decodes like libjpeg-turbo: missing blocks are
// gray until the next restart, and the image comes back with an error
// wrapping ErrTruncated.
func (d *Decoder) Decode(ctx context.Context, src []byte, o *DecodeOptions) (image.Image, error) {
	if o == nil {
		o = &DecodeOptions{}
	}
	scale := o.Scale
	if scale == 0 {
		scale = 1
	}
	if scale != 1 && scale != 2 && scale != 4 && scale != 8 {
		return nil, ErrUnsupported
	}
	if (o.Recon.IDCT != nil || o.Recon.ToRGBA != nil) && (scale != 1 || !o.Region.Empty()) {
		return nil, ErrUnsupported // Recon is 8x8 only, whole image: docs/design.md section 5
	}

	if err := d.ensureTables(o.Tables); err != nil {
		return nil, err
	}
	d.frame = d.tablesFrame
	if err := d.frame.ParseFrame(src); err != nil {
		return nil, err
	}
	f := &d.frame
	f.Ledger = o.Limits.Ledger()
	if err := o.Limits.checkPixels(f.Width, f.Height); err != nil {
		return nil, err
	}

	if f.NComp == 3 && f.RGB {
		if o.Format == FormatYCbCr {
			return nil, ErrUnsupported
		}
		if f.Comp[1].H != f.Comp[0].H || f.Comp[1].V != f.Comp[0].V ||
			f.Comp[2].H != f.Comp[0].H || f.Comp[2].V != f.Comp[0].V {
			return nil, ErrUnsupported // non-uniform RGB sampling, not in the corpus
		}
	}

	hmax, vmax := codec.MaxSampling(f)
	minSize := 8 / scale
	raw := o.Format == FormatYCbCr // chroma uses Y's scaled IDCT: docs/design.md section 2

	w, h := ceilDiv(f.Width, scale), ceilDiv(f.Height, scale)
	full := image.Rect(0, 0, w, h)
	if dst := o.Dst; dst != nil && (o.Format != FormatRGBA || scale != 1 || !o.Region.Empty() || o.Owned ||
		o.Recon.IDCT != nil || o.Recon.ToRGBA != nil ||
		dst.Rect.Dx() != w || dst.Rect.Dy() != h || dst.Stride < 4*w || len(dst.Pix) < 4*w ||
		(h > 1 && dst.Stride > (len(dst.Pix)-4*w)/(h-1))) { // no (h-1)*Stride overflow
		return nil, ErrUnsupported
	}
	eff := full
	if r := o.Region; !r.Empty() {
		if r.Min.X < -maxRegion || r.Min.Y < -maxRegion || r.Max.X > maxRegion || r.Max.Y > maxRegion {
			return nil, fmt.Errorf("jpeg: region %v outside +-%d", r, maxRegion)
		}
		eff = r.Intersect(full)
		if eff.Empty() {
			return nil, fmt.Errorf("jpeg: region %v does not intersect image %v", o.Region, full)
		}
	}

	// Gray from YCbCr skips the chroma IDCT.
	grayOnly := f.NComp == 1 || (o.Format == FormatGray && !f.RGB)

	// Mirrors decodeScan's own banding gate exactly (same f.ScanN/NComp,
	// read right after the first scan's SOS): a frame whose first scan
	// doesn't cover every component decodes in full regardless of Region,
	// so the Pix buffers stay full-size. Narrowing bandRect anyway would
	// misalign every row against them (T08 fix: jpeg.go must agree with
	// the codec on whether banding actually happened, not just ask for
	// it, or a Region on such a frame reads the wrong rows).
	bandOK := !f.Progressive && (f.NComp == 1 || f.ScanN == f.NComp)

	var band *codec.Band
	bandRect := full
	if !o.Region.Empty() && bandOK {
		bandRect = d.planBand(f, o, eff, w, h, hmax, vmax, minSize)
		band = &d.band
	}

	// need is computed before any allocation (docs/design.md section 6),
	// from the same first/last rows decodeScan itself will use.
	first, last := 0, ceilDiv(f.Height, vmax*8)-1
	if band != nil {
		first, last = d.band.First, d.band.Last
	}
	need, out, ext := decodeNeed(f, hmax, vmax, minSize, raw, grayOnly, first, last, o.Format == FormatRGBA && o.Dst == nil, bandRect.Dx(), bandRect.Dy())
	if o.Format == FormatRGBA {
		need += 2 * int64(bandRect.Dx()) // ensureChromaRows' cb and cr rows
		ext += 2 * int64(bandRect.Dx())
	}
	if !o.Region.Empty() && eff != o.Region {
		out = padNeed(o, f, grayOnly) // pad's own buffer, past the image edge
		need += out
	}
	if d.cloneOut {
		need += out + 3*8192 // package Decode's copy: up to 3 buffers, each rounded to 8 KiB pages
		ext += out + 3*8192
	}
	if err := o.Limits.checkBytes(need); err != nil {
		return nil, err
	}
	workers := 0
	if band == nil && o.Workers > 1 {
		n := min(o.Workers, maxWorkers)
		extra := int64(n) * (2*int64(w) + 32<<10 + codec.BandTableExtra) // rows, tables, blocks, stack
		if ri := f.RestartInterval; ri > 0 {
			extra += 8 * int64(ceilDiv(f.Width, 8)*ceilDiv(f.Height, 8)/ri+1) // interval starts, at most
		}
		fits := o.Reserved && need+extra-ext <= maxNeed || !o.Reserved && fitsLimit(need+extra-ext)
		if fits && o.Limits.checkBytes(need+extra) == nil { // else serial, so errors do not change
			need, workers = need+extra, n
		}
	}
	d.lastNeed = need
	if o.Reserved {
		if need-ext > maxNeed {
			return nil, fmt.Errorf("jpeg: decode needs %d bytes, over the 1 GiB cap; use Region", need-ext)
		}
	} else if err := d.reserve(ctx, need-ext); err != nil {
		return nil, err
	}
	defer d.releaseReservation()

	for ci := 0; ci < f.NComp; ci++ {
		bs := minSize
		if !raw {
			bs = codec.ComponentBlockSize(int(f.Comp[ci].H), int(f.Comp[ci].V), hmax, vmax, minSize)
		}
		d.planes[ci].Skip = grayOnly && ci > 0
		d.planes[ci].ReconIDCT = o.Recon.IDCT
		d.planes[ci].BlockSize = bs
	}
	var truncated bool
	var err error
	if workers > 1 {
		truncated, err = codec.DecodeScanN(ctx, f, src, d.planes[:f.NComp], workers)
	} else {
		truncated, _, _, err = codec.DecodeScanBand(ctx, f, src, d.planes[:f.NComp], band)
	}
	if err != nil && !errors.Is(err, ErrAfterScan) {
		return nil, err
	}
	if err != nil && eff.Max.Y < h && !o.CheckFrame {
		err = nil // turbo skips the rows below a crop, and never reads the marker
	}

	owned := o.Owned && o.Format == FormatRGBA
	padded := !o.Region.Empty() && eff != o.Region
	var img image.Image
	switch {
	case o.Format == FormatRGBA && o.Dst != nil:
		saved := d.rgbaImg                   // never keep the caller's buffer
		defer func() { d.rgbaImg = saved }() // restore even on panic
		d.rgbaImg = *o.Dst
		if _, err := d.rgba(f, grayOnly, nil, bandRect, band, hmax, vmax, minSize, workers, true); err != nil {
			return nil, err
		}
		img = o.Dst
	case o.Format == FormatRGBA:
		if owned && !padded { // pad makes the owned copy
			d.rgbaImg = image.RGBA{} // ensureRGBA allocates afresh
		}
		var rerr error
		if img, rerr = d.rgba(f, grayOnly, o.Recon.ToRGBA, bandRect, band, hmax, vmax, minSize, workers, false); rerr != nil {
			return nil, rerr
		}
	case grayOnly:
		img = d.gray(&d.planes[0], bandRect)
	case f.RGB:
		img = d.rgbGray(bandRect)
	default:
		if scale == 8 && bandRect.Max.Y == h {
			patchYUVOverread(&d.planes[0], &d.planes[1], int(f.Comp[0].H), vmax, w, bandRect.Dy())
		}
		img = d.ycbcr(f, bandRect)
	}

	if !o.Region.Empty() {
		// SubImage's own logic (offset then narrow Rect), but written
		// into a separate *Crop struct instead of img itself: bandRect.Min
		// may be an earlier MCU row than eff.Min (rounded down to the
		// band, plus fancy-upsampling context), and reslicing Pix in
		// place on the shared *Img struct would shrink its capacity for
		// the next call (T08).
		switch im := img.(type) {
		case *image.Gray:
			off := im.PixOffset(eff.Min.X, eff.Min.Y)
			d.grayCrop = image.Gray{Pix: im.Pix[off:], Stride: im.Stride, Rect: eff}
			img = &d.grayCrop
		case *image.RGBA:
			off := im.PixOffset(eff.Min.X, eff.Min.Y)
			d.rgbaCrop = image.RGBA{Pix: im.Pix[off:], Stride: im.Stride, Rect: eff}
			img = &d.rgbaCrop
		case *image.YCbCr:
			yOff, cOff := im.YOffset(eff.Min.X, eff.Min.Y), im.COffset(eff.Min.X, eff.Min.Y)
			d.ycbcrCrop = image.YCbCr{
				Y: im.Y[yOff:], Cb: im.Cb[cOff:], Cr: im.Cr[cOff:],
				YStride: im.YStride, CStride: im.CStride, SubsampleRatio: im.SubsampleRatio, Rect: eff,
			}
			img = &d.ycbcrCrop
		}
		if padded {
			if owned {
				d.padPix = nil // padBuf allocates afresh
			}
			var perr error
			if img, perr = d.pad(img, o.Region); perr != nil {
				return nil, perr
			}
		}
	}
	if owned {
		c := *img.(*image.RGBA)
		img = &c
		d.rgbaCrop = image.RGBA{}
		if padded {
			d.padPix = nil
		} else {
			d.rgbaImg = image.RGBA{}
		}
	}

	if err != nil && truncated {
		return img, fmt.Errorf("%w; %w", err, ErrTruncated)
	}
	if err != nil {
		return img, err
	}
	if truncated {
		return img, fmt.Errorf("%w: entropy data ran out, had a bad Huffman code, or restart markers out of sequence", ErrTruncated)
	}
	return img, nil
}

// pad copies a *Crop image into zeroed Decoder memory bounded by r, so
// Bounds() == Region even where it leaves the image (design.md section 2).
func (d *Decoder) pad(img image.Image, r image.Rectangle) (image.Image, error) {
	in := img.Bounds()
	w, h := r.Dx(), r.Dy()
	switch im := img.(type) {
	case *image.Gray:
		pix, err := d.padBuf(w * h)
		if err != nil {
			return nil, err
		}
		p := image.Gray{Pix: pix, Stride: w, Rect: r}
		for y := in.Min.Y; y < in.Max.Y; y++ {
			copy(p.Pix[p.PixOffset(in.Min.X, y):], im.Pix[im.PixOffset(in.Min.X, y):][:in.Dx()])
		}
		d.grayCrop = p
		return &d.grayCrop, nil
	case *image.RGBA:
		pix, err := d.padBuf(4 * w * h)
		if err != nil {
			return nil, err
		}
		p := image.RGBA{Pix: pix, Stride: 4 * w, Rect: r}
		for y := in.Min.Y; y < in.Max.Y; y++ {
			copy(p.Pix[p.PixOffset(in.Min.X, y):], im.Pix[im.PixOffset(in.Min.X, y):][:4*in.Dx()])
		}
		d.rgbaCrop = p
		return &d.rgbaCrop, nil
	case *image.YCbCr:
		// Chroma size as image.NewYCbCr computes it.
		cw, ch := chromaRect(im.SubsampleRatio, r)
		buf, err := d.padBuf(w*h + 2*cw*ch)
		if err != nil {
			return nil, err
		}
		p := image.YCbCr{
			Y: buf[:w*h], Cb: buf[w*h : w*h+cw*ch], Cr: buf[w*h+cw*ch:],
			YStride: w, CStride: cw, SubsampleRatio: im.SubsampleRatio, Rect: r,
		}
		for y := in.Min.Y; y < in.Max.Y; y++ {
			copy(p.Y[p.YOffset(in.Min.X, y):], im.Y[im.YOffset(in.Min.X, y):][:in.Dx()])
			for x := in.Min.X; x < in.Max.X; x++ {
				p.Cb[p.COffset(x, y)] = im.Cb[im.COffset(x, y)]
				p.Cr[p.COffset(x, y)] = im.Cr[im.COffset(x, y)]
			}
		}
		d.ycbcrCrop = p
		return &d.ycbcrCrop, nil
	}
	return img, nil
}

// chromaRect is image.NewYCbCr's chroma plane size for r.
func chromaRect(ratio image.YCbCrSubsampleRatio, r image.Rectangle) (cw, ch int) {
	cw, ch = r.Dx(), r.Dy()
	switch ratio {
	case image.YCbCrSubsampleRatio422, image.YCbCrSubsampleRatio420:
		cw = (r.Max.X+1)/2 - r.Min.X/2
	case image.YCbCrSubsampleRatio411, image.YCbCrSubsampleRatio410:
		cw = (r.Max.X+3)/4 - r.Min.X/4
	}
	switch ratio {
	case image.YCbCrSubsampleRatio420, image.YCbCrSubsampleRatio440, image.YCbCrSubsampleRatio410:
		ch = (r.Max.Y+1)/2 - r.Min.Y/2
	}
	return
}

// padBuf returns d.padPix resized to n and zeroed, reusing its array.
func (d *Decoder) padBuf(n int) ([]byte, error) {
	if err := d.frame.Ledger.Charge(int64(n)); err != nil {
		return nil, err
	}
	if cap(d.padPix) < n {
		d.padPix = make([]byte, n)
	}
	d.padPix = d.padPix[:n]
	clear(d.padPix)
	return d.padPix, nil
}

// planBand fills d.band with the MCU-row band DecodeScanBand needs to
// cover eff (T08): the region's own rows and columns, plus one chroma
// row/column of context on each side when fancy upsampling applies (RGBA,
// h2v2/h2v1). The returned rect is that band's row bounds in the same
// scaled output coordinates as eff, at full width. A column band skips
// transforming MCUs outside it (and, with a DRI, entropy-decoding them);
// rgba then narrows its own buffer to the band's columns, while gray and
// YCbCr alias the full-width planes.
func (d *Decoder) planBand(f *codec.Frame, o *DecodeOptions, eff image.Rectangle, w, h, hmax, vmax, minSize int) image.Rectangle {
	outStep := vmax * minSize
	mcuRows := ceilDiv(f.Height, vmax*8)
	first, last := eff.Min.Y/outStep, (eff.Max.Y-1)/outStep
	fancy := o.Format == FormatRGBA && minSize > 1
	if fancy && vmax == 2 {
		if first > 0 {
			first--
		}
		if last < mcuRows-1 {
			last++
		}
	}

	outStepX := hmax * minSize
	mcusPerRow := ceilDiv(f.Width, hmax*8)
	colFirst, colLast := eff.Min.X/outStepX, (eff.Max.X-1)/outStepX
	if fancy && hmax > 1 {
		if colFirst > 0 {
			colFirst--
		}
		if colLast < mcusPerRow-1 {
			colLast++
		}
	}
	cols := colFirst > 0 || colLast < mcusPerRow-1 // no-op band skipped entirely (T08: avoid a needless jump per row)

	d.band = codec.Band{First: first, Last: last, Cols: cols, ColFirst: colFirst, ColLast: colLast, Restart: o.Restart, CheckFrame: o.CheckFrame}
	return image.Rect(0, first*outStep, w, min((last+1)*outStep, h))
}

// gray wraps an IDCT'd plane as d's Gray image, bounded by r (the scaled
// output rect, docs/design.md section 2; a band's rect when Region is set,
// T08).
func (d *Decoder) gray(p *codec.Plane, r image.Rectangle) *image.Gray {
	d.grayImg = image.Gray{Pix: p.Pix, Stride: p.Stride(), Rect: r}
	return &d.grayImg
}

// ycbcr wraps the IDCT'd planes as d's YCbCr image, bounded by r. Strides
// are block-padded and the padding holds real IDCT output (docs/design.md
// section 4). FormatYCbCr gives every component the same scaled IDCT, so
// the ratio between planes matches the stream's own sampling at any Scale.
func (d *Decoder) ycbcr(f *codec.Frame, r image.Rectangle) *image.YCbCr {
	d.ycbcrImg = image.YCbCr{
		Y: d.planes[0].Pix, Cb: d.planes[1].Pix, Cr: d.planes[2].Pix,
		YStride: d.planes[0].Stride(), CStride: d.planes[1].Stride(),
		SubsampleRatio: subsampleRatio(f.Comp[0].H, f.Comp[0].V),
		Rect:           r,
	}
	return &d.ycbcrImg
}

// patchYUVOverread reproduces two tj3DecompressToYUVPlanes8 quirks at
// Scale 8 for odd Y dimensions in 4:2:0/4:2:2 (turbojpeg.c:2251-2328):
//
//   - A 4:2:0 iMCU row beyond Y's real height_in_blocks is never refilled
//     by jpeg_read_raw_data, so that dummy row's tmpbuf slot is stale:
//     whatever the previous iMCU row's same slot held. With no
//     previous iMCU row turbo reads uninitialised memory, so skip it.
//   - Y's tmpbuf row is exactly the unpadded width, one short of the YUV
//     plane's padded width, for odd width; the memcpy of the padded width
//     spills into the next tmpbuf row, or (the last Y row of an iMCU row)
//     into Cb's row for that iMCU row, both at column 0.
func patchYUVOverread(y, cb *codec.Plane, hsub, vmax, w, h int) {
	ys, cs := y.Stride(), cb.Stride()
	lh := ceilDiv(h, vmax) * vmax
	if lh > h && h > vmax {
		copy(y.Pix[h*ys:h*ys+w], y.Pix[(h-vmax)*ys:(h-vmax)*ys+w])
	}
	if hsub != 2 || w%2 == 0 {
		return
	}
	for row := 0; row < lh; row++ {
		g, j := row/vmax, row%vmax
		v := cb.Pix[g*cs]
		if j < vmax-1 {
			v = y.Pix[(row+1)*ys]
		}
		y.Pix[row*ys+w] = v
	}
}

// ensureRGBA sizes d's RGBA image without shrinking its backing array, so
// a warmed-up Decoder allocates nothing (docs/design.md T06 spec).
func (d *Decoder) ensureRGBA(r image.Rectangle) error {
	w, h := r.Dx(), r.Dy()
	stride := w * 4
	need := stride * h
	if err := d.frame.Ledger.Charge(int64(need)); err != nil {
		return err
	}
	if cap(d.rgbaImg.Pix) < need {
		d.rgbaImg.Pix = make([]byte, need)
	}
	d.rgbaImg.Pix = d.rgbaImg.Pix[:need]
	d.rgbaImg.Stride = stride
	d.rgbaImg.Rect = r
	return nil
}

// ensureChromaRows sizes d's upsampled chroma row scratch without
// shrinking its backing arrays, so a warmed-up Decoder allocates nothing
// (docs/design.md P1 spec).
func (d *Decoder) ensureChromaRows(w int) error {
	if err := d.frame.Ledger.Charge(2 * int64(w)); err != nil {
		return err
	}
	if cap(d.cbRow) < w {
		d.cbRow = make([]byte, w)
		d.crRow = make([]byte, w)
	}
	d.cbRow, d.crRow = d.cbRow[:w], d.crRow[:w]
	return nil
}

// rgba builds d's RGBA image, bounded by r, from the IDCT'd planes: gray
// replication for a 1-component stream, a null convert for an RGB stream,
// or upsampling plus colour conversion for YCbCr (docs/design.md section
// 5). At Scale 1 a Recon hook, when set, takes the exported YCbCr seam;
// otherwise every scale runs codec.YCbCrUpsampleConvertGo on d's own
// scratch rows (a Recon hook never fires above Scale 1, checked in
// Decode), so a larger scale always uses the component's own scaled
// downsampled size (hmax, vmax, minSize; T07). keepDst means d.rgbaImg
// already holds the caller's Dst, which ensureRGBA must not resize.
func (d *Decoder) rgba(f *codec.Frame, grayOnly bool, hook func(dst *image.RGBA, src *image.YCbCr), r image.Rectangle, band *codec.Band, hmax, vmax, minSize int, workers int, keepDst bool) (*image.RGBA, error) {
	hooked := minSize == 8 && hook != nil
	x0 := 0
	if band != nil && band.Cols && !hooked {
		// Only the band's MCU columns were transformed.
		step := hmax * minSize
		x0 = band.ColFirst * step
		r.Min.X, r.Max.X = x0, min((band.ColLast+1)*step, r.Max.X)
	}
	if !keepDst {
		if err := d.ensureRGBA(r); err != nil {
			return nil, err
		}
	}
	w, h := r.Dx(), r.Dy()
	switch {
	case grayOnly:
		p := &d.planes[0]
		codec.GrayToRGBAGo(&d.rgbaImg, p.Pix[x0:], p.Stride(), w, h)
	case f.RGB:
		stride := d.planes[0].Stride()
		codec.RGBToRGBAGo(&d.rgbaImg, d.planes[0].Pix[x0:], d.planes[1].Pix[x0:], d.planes[2].Pix[x0:], stride, w, h)
	case hooked:
		hook(&d.rgbaImg, d.ycbcr(f, r))
	default:
		dsw, dsh := codec.DownsampledDims(f, 1, hmax, vmax, d.planes[1].BlockSize)
		// A band starts partway into the chroma plane (T08); dsh above is
		// the full image's chroma height, so shift it to band-local rows.
		first := r.Min.Y / (vmax * minSize)
		bs1 := d.planes[1].BlockSize
		dsh = min(max(dsh-first*int(f.Comp[1].V)*bs1, 0), d.planes[1].BlocksHigh*bs1)
		if err := d.ensureChromaRows(w); err != nil {
			return nil, err
		}
		c1 := f.Comp[1]
		hExp, vExp := hmax*minSize/(int(c1.H)*bs1), vmax*minSize/(int(c1.V)*bs1)
		fancy := minSize > 1 && (dsw > 2 || hExp == 1) // turbo decides on the whole width; h1v2 has no width test
		cx0 := x0 * int(c1.H) * bs1 / (hmax * minSize)
		cw := min(dsw-cx0, ceilDiv(w, hExp))
		if band == nil && workers > 1 {
			codec.YCbCrUpsampleConvertN(&d.rgbaImg, d.planes[0].Pix, d.planes[0].Stride(),
				d.planes[1].Pix, d.planes[2].Pix, d.planes[1].Stride(), w, h, cw, dsh, hExp, vExp, fancy, d.cbRow, d.crRow, workers)
			break
		}
		codec.YCbCrUpsampleConvertGo(&d.rgbaImg, d.planes[0].Pix[x0:], d.planes[0].Stride(),
			d.planes[1].Pix[cx0:], d.planes[2].Pix[cx0:], d.planes[1].Stride(), w, h, cw, dsh, hExp, vExp, fancy, d.cbRow, d.crRow)
	}
	return &d.rgbaImg, nil
}

// rgbGray combines an RGB stream's planes (uniform sampling, checked in
// Decode) in place with turbo's rgb_gray_convert weights (jdcolor.c).
func (d *Decoder) rgbGray(r image.Rectangle) *image.Gray {
	rp, g, b := d.planes[0].Pix, d.planes[1].Pix, d.planes[2].Pix
	for i := range rp {
		rp[i] = byte((int32(rp[i])*19595 + int32(g[i])*38470 + int32(b[i])*7471 + 32768) >> 16)
	}
	return d.gray(&d.planes[0], r)
}

var decoderPool = sync.Pool{New: func() any { return new(Decoder) }}

// Decode is Decoder.Decode on pooled scratch; the returned image is a copy
// the caller owns, since the pooled Decoder is reused by the next call.
// With Owned and FormatRGBA it is the decode's own image, not a copy;
// with Dst it is Dst.
func Decode(ctx context.Context, src []byte, o *DecodeOptions) (image.Image, error) {
	d := decoderPool.Get().(*Decoder)
	clone := o == nil || (o.Dst == nil && (!o.Owned || o.Format != FormatRGBA))
	d.cloneOut = clone
	img, err := d.Decode(ctx, src, o)
	d.cloneOut = false
	out := img
	if clone && img != nil {
		if cerr := d.frame.Ledger.Charge(imageBytes(img)); cerr != nil {
			decoderPool.Put(d)
			return nil, cerr
		}
		out = cloneImage(img)
	}
	decoderPool.Put(d)
	return out, err
}

// imageBytes is the size of the buffers cloneImage copies.
func imageBytes(img image.Image) int64 {
	switch im := img.(type) {
	case *image.Gray:
		return int64(len(im.Pix))
	case *image.RGBA:
		return int64(len(im.Pix))
	case *image.YCbCr:
		return int64(len(im.Y) + len(im.Cb) + len(im.Cr))
	}
	return 0
}

func cloneImage(img image.Image) image.Image {
	switch im := img.(type) {
	case *image.Gray:
		return &image.Gray{Pix: append([]byte(nil), im.Pix...), Stride: im.Stride, Rect: im.Rect}
	case *image.RGBA:
		return &image.RGBA{Pix: append([]byte(nil), im.Pix...), Stride: im.Stride, Rect: im.Rect}
	case *image.YCbCr:
		return &image.YCbCr{
			Y: append([]byte(nil), im.Y...), Cb: append([]byte(nil), im.Cb...), Cr: append([]byte(nil), im.Cr...),
			YStride: im.YStride, CStride: im.CStride, SubsampleRatio: im.SubsampleRatio, Rect: im.Rect,
		}
	default:
		return nil
	}
}

// Config is a JPEG frame's shape and colour space, from DecodeConfig.
type Config struct {
	Width, Height   int
	Components      int                       // 1 or 3
	Ratio           image.YCbCrSubsampleRatio // 3 components only
	RGB             bool                      // stored as RGB (Adobe transform 0 or ids R,G,B)
	RestartInterval int                       // MCUs, 0 for none
}

// DecodeConfig parses src (an abbreviated tables stream, when non-nil,
// supplies DQT/DHT) up to SOS and returns the frame's shape and colour
// space. It allocates nothing.
func DecodeConfig(src, tables []byte) (Config, error) {
	return DecodeConfigLimits(src, tables, Limits{})
}

// DecodeConfigLimits is DecodeConfig that refuses, with ErrLimit, a frame
// over l.MaxPixels. MaxBytes does not apply: the call allocates nothing.
func DecodeConfigLimits(src, tables []byte, l Limits) (Config, error) {
	var f codec.Frame
	if err := f.Parse(src, tables); err != nil {
		return Config{}, err
	}
	if err := l.checkPixels(f.Width, f.Height); err != nil {
		return Config{}, err
	}
	return configOf(&f), nil
}

func configOf(f *codec.Frame) Config {
	c := Config{
		Width:           f.Width,
		Height:          f.Height,
		Components:      f.NComp,
		RGB:             f.RGB,
		RestartInterval: f.RestartInterval,
	}
	if f.NComp == 3 {
		c.Ratio = subsampleRatio(f.Comp[0].H, f.Comp[0].V)
	}
	return c
}

func ceilDiv(a, b int) int { return (a + b - 1) / b }

func subsampleRatio(h, v byte) image.YCbCrSubsampleRatio {
	switch {
	case h == 1 && v == 1:
		return image.YCbCrSubsampleRatio444
	case h == 2 && v == 1:
		return image.YCbCrSubsampleRatio422
	case h == 1 && v == 2:
		return image.YCbCrSubsampleRatio440
	case h == 4 && v == 1:
		return image.YCbCrSubsampleRatio411
	case h == 4 && v == 2:
		return image.YCbCrSubsampleRatio410
	default: // h == 2 && v == 2, the only other case validateSampling allows
		return image.YCbCrSubsampleRatio420
	}
}
