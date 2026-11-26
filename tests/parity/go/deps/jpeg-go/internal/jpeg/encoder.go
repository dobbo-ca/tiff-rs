package jpeg

import (
	"bytes"
	"context"
	"image"
	"image/draw"
	"io"
	"sync"

	"github.com/dobbo-ca/jpeg-go/internal/codec"
)

// EncodeOptions configures Encoder.Encode and Encode.
type EncodeOptions struct {
	Quality   int  // 1..100, libjpeg scaling; 0 means 75
	Chroma444 bool // RGBA input only; default 4:2:0 like jpeg_set_defaults
}

// Encoder keeps its buffers between calls; the zero value is ready.
type Encoder struct {
	buf  bytes.Buffer
	rgba *image.RGBA

	yBuf, fullCbBuf, fullCrBuf, cbBuf, crBuf []byte
	lumaQ, chromaQ                           [64]int32
	comps                                    [3]codec.EncComponent
	scan                                     codec.Scan
	planes                                   [3]codec.EncPlane

	uniformRow             []byte
	uniformKey             uniformKey
	uniformValid           bool
	uniformOut             []byte
	disableUniformFastPath bool // TestUniformSameBytes only
}

// Encode encodes m into w. nil options mean Quality 75, 4:2:0.
func (e *Encoder) Encode(ctx context.Context, w io.Writer, m image.Image, o *EncodeOptions) error {
	quality, chroma444 := 75, false
	if o != nil {
		if o.Quality != 0 {
			quality = o.Quality
		}
		chroma444 = o.Chroma444
	}

	e.buf.Reset()
	if err := e.encode(ctx, m, quality, chroma444); err != nil {
		return err
	}
	_, err := w.Write(e.buf.Bytes())
	return err
}

var encoderPool = sync.Pool{New: func() any { return new(Encoder) }}

// Encode is Encoder.Encode on pooled scratch.
func Encode(ctx context.Context, w io.Writer, m image.Image, o *EncodeOptions) error {
	e := encoderPool.Get().(*Encoder)
	defer encoderPool.Put(e)
	return e.Encode(ctx, w, m, o)
}

func (e *Encoder) encode(ctx context.Context, m image.Image, quality int, chroma444 bool) error {
	if !e.disableUniformFastPath {
		if key, ok := e.uniformKeyFor(m, quality, chroma444); ok {
			return e.encodeUniform(ctx, key)
		}
	}

	b := m.Bounds()
	width, height := b.Dx(), b.Dy()

	var comps []codec.EncComponent
	gray := false

	switch img := m.(type) {
	case *image.YCbCr:
		if !directYCbCr(img) {
			rgba := e.drawRGBA(m, b)
			return e.encode4Byte(ctx, rgba.Pix, rgba.Stride, width, height, quality, chroma444)
		}
		chroma444 = img.SubsampleRatio == image.YCbCrSubsampleRatio444
		y := codec.EncPlane{Pix: img.Y[img.YOffset(b.Min.X, b.Min.Y):], Stride: img.YStride, W: width, H: height}
		// Chroma width/height come from the real bounds, not CStride: a
		// SubImage's stride can exceed its width, and reading past the
		// real edge must replicate, not read the parent's neighbour pixels.
		cw, ch := width, height
		if !chroma444 {
			cw, ch = ceilDiv(width, 2), ceilDiv(height, 2)
		}
		cOff := img.COffset(b.Min.X, b.Min.Y)
		cb := codec.EncPlane{Pix: img.Cb[cOff:], Stride: img.CStride, W: cw, H: ch}
		cr := codec.EncPlane{Pix: img.Cr[cOff:], Stride: img.CStride, W: cw, H: ch}
		comps = e.ycbcrComponents(y, cb, cr, quality, chroma444)

	case *image.Gray:
		gray = true
		y := codec.EncPlane{Pix: img.Pix[img.PixOffset(b.Min.X, b.Min.Y):], Stride: img.Stride, W: width, H: height}
		comps = e.grayComponents(y, quality)

	case *image.RGBA:
		return e.encode4Byte(ctx, img.Pix[img.PixOffset(b.Min.X, b.Min.Y):], img.Stride, width, height, quality, chroma444)

	case *image.NRGBA:
		return e.encode4Byte(ctx, img.Pix[img.PixOffset(b.Min.X, b.Min.Y):], img.Stride, width, height, quality, chroma444)

	default:
		rgba := e.drawRGBA(m, b)
		return e.encode4Byte(ctx, rgba.Pix, rgba.Stride, width, height, quality, chroma444)
	}

	codec.WriteHeader(&e.buf, codec.HeaderParams{
		Width: width, Height: height, Quality: quality, Gray: gray, Chroma444: chroma444,
	})
	if err := codec.WriteScan(ctx, &e.buf, width, height, comps); err != nil {
		return err
	}
	e.buf.Write([]byte{0xff, 0xd9}) // EOI
	return nil
}

// drawRGBA draws m into e's reused RGBA scratch buffer.
func (e *Encoder) drawRGBA(m image.Image, b image.Rectangle) *image.RGBA {
	if e.rgba == nil || e.rgba.Bounds().Dx() != b.Dx() || e.rgba.Bounds().Dy() != b.Dy() {
		e.rgba = image.NewRGBA(image.Rect(0, 0, b.Dx(), b.Dy()))
	}
	draw.Draw(e.rgba, e.rgba.Bounds(), m, b.Min, draw.Src)
	return e.rgba
}

// Odd or negative origins split chroma pairs.
func directYCbCr(img *image.YCbCr) bool {
	switch img.SubsampleRatio {
	case image.YCbCrSubsampleRatio444:
		return true
	case image.YCbCrSubsampleRatio420:
		m := img.Rect.Min
		return m.X >= 0 && m.Y >= 0 && (m.X|m.Y)&1 == 0
	}
	return false
}

// ycbcrComponents builds the 3-component list (jcparam.c:379-381), reusing
// e's quant-table and component-list scratch.
func (e *Encoder) ycbcrComponents(y, cb, cr codec.EncPlane, quality int, chroma444 bool) []codec.EncComponent {
	e.lumaQ = codec.QuantNatural(codec.LumaQuantTable(quality))
	e.chromaQ = codec.QuantNatural(codec.ChromaQuantTable(quality))
	yh, yv := 2, 2
	if chroma444 {
		yh, yv = 1, 1
	}
	e.comps[0] = codec.EncComponent{Plane: y, HSamp: yh, VSamp: yv, Quant: &e.lumaQ, DC: &codec.StdDCLumaCodes, AC: &codec.StdACLumaCodes}
	e.comps[1] = codec.EncComponent{Plane: cb, HSamp: 1, VSamp: 1, Quant: &e.chromaQ, DC: &codec.StdDCChromaCodes, AC: &codec.StdACChromaCodes}
	e.comps[2] = codec.EncComponent{Plane: cr, HSamp: 1, VSamp: 1, Quant: &e.chromaQ, DC: &codec.StdDCChromaCodes, AC: &codec.StdACChromaCodes}
	return e.comps[:3]
}

func (e *Encoder) grayComponents(y codec.EncPlane, quality int) []codec.EncComponent {
	e.lumaQ = codec.QuantNatural(codec.LumaQuantTable(quality))
	e.comps[0] = codec.EncComponent{Plane: y, HSamp: 1, VSamp: 1, Quant: &e.lumaQ, DC: &codec.StdDCLumaCodes, AC: &codec.StdACLumaCodes}
	return e.comps[:1]
}

// encode4Byte converts per MCU row.
func (e *Encoder) encode4Byte(ctx context.Context, pix []byte, stride, w, h, quality int, chroma444 bool) error {
	cw, ch, rows := w, h, 8
	if !chroma444 {
		cw, ch, rows = ceilDiv(w, 16)*8, ceilDiv(h, 2), 16
	}
	e.yBuf = codec.GrowBytes(e.yBuf, rows*w)
	e.cbBuf = codec.GrowBytes(e.cbBuf, 8*cw)
	e.crBuf = codec.GrowBytes(e.crBuf, 8*cw)
	comps := e.ycbcrComponents(codec.EncPlane{W: w, H: h}, codec.EncPlane{W: cw, H: ch}, codec.EncPlane{W: cw, H: ch}, quality, chroma444)

	codec.WriteHeader(&e.buf, codec.HeaderParams{Width: w, Height: h, Quality: quality, Chroma444: chroma444})
	s := &e.scan
	s.Start(&e.buf, w, h, comps)
	for my := 0; my < s.MCURows(); my++ {
		if err := ctx.Err(); err != nil {
			return err
		}
		y0 := my * rows
		if chroma444 {
			for r := y0; r < min(y0+rows, h); r++ {
				o := (r - y0) * w
				codec.RGBAToYCbCrRow(pix[r*stride:], e.yBuf[o:o+w], e.cbBuf[o:o+w], e.crBuf[o:o+w])
			}
		} else {
			e.stripH2V2(pix, stride, w, h, cw, my)
		}
		e.planes = [3]codec.EncPlane{
			{Pix: e.yBuf, Stride: w, W: w, H: h - y0},
			{Pix: e.cbBuf, Stride: cw, W: cw, H: ch - my*8},
			{Pix: e.crBuf, Stride: cw, W: cw, H: ch - my*8},
		}
		s.Row(my, e.planes[:])
	}
	s.Finish()
	e.buf.Write([]byte{0xff, 0xd9}) // EOI
	return nil
}

// stripH2V2 fills strips (jcsample.c).
func (e *Encoder) stripH2V2(pix []byte, stride, w, h, cw, my int) {
	fw := 2 * cw // chroma padded to block edge
	e.fullCbBuf = codec.GrowBytes(e.fullCbBuf, 2*fw)
	e.fullCrBuf = codec.GrowBytes(e.fullCrBuf, 2*fw)
	ch := ceilDiv(h, 2)
	for oy := my * 8; oy < min(my*8+8, ch); oy++ {
		for k := 0; k < 2; k++ {
			row := min(2*oy+k, h-1) // odd last row repeats
			o := (row - my*16) * w
			fcb, fcr := e.fullCbBuf[k*fw:k*fw+fw], e.fullCrBuf[k*fw:k*fw+fw]
			codec.RGBAToYCbCrRow(pix[row*stride:], e.yBuf[o:o+w], fcb[:w], fcr[:w])
			for x := w; x < fw; x++ {
				fcb[x], fcr[x] = fcb[w-1], fcr[w-1]
			}
		}
		o := (oy - my*8) * cw
		codec.DownsampleH2V2Row(e.cbBuf[o:o+cw], e.fullCbBuf[:fw], e.fullCbBuf[fw:])
		codec.DownsampleH2V2Row(e.crBuf[o:o+cw], e.fullCrBuf[:fw], e.fullCrBuf[fw:])
	}
}
