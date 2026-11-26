package encref

import (
	"bytes"
	"context"
	"image"
	"image/draw"
	"io"
	"sync"
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
	comps                                    [3]EncComponent

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

	var comps []EncComponent
	gray := false

	switch img := m.(type) {
	case *image.YCbCr:
		if !directYCbCr(img) {
			rgba := e.drawRGBA(m, b)
			yP, cbP, crP := e.planesFrom4Byte(rgba.Pix, rgba.Stride, width, height, chroma444)
			comps = e.ycbcrComponents(yP, cbP, crP, quality, chroma444)
			break
		}
		chroma444 = img.SubsampleRatio == image.YCbCrSubsampleRatio444
		y := EncPlane{Pix: img.Y[img.YOffset(b.Min.X, b.Min.Y):], Stride: img.YStride, W: width, H: height}
		// Chroma width/height come from the real bounds, not CStride: a
		// SubImage's stride can exceed its width, and reading past the
		// real edge must replicate, not read the parent's neighbour pixels.
		cw, ch := width, height
		if !chroma444 {
			cw, ch = ceilDiv(width, 2), ceilDiv(height, 2)
		}
		cOff := img.COffset(b.Min.X, b.Min.Y)
		cb := EncPlane{Pix: img.Cb[cOff:], Stride: img.CStride, W: cw, H: ch}
		cr := EncPlane{Pix: img.Cr[cOff:], Stride: img.CStride, W: cw, H: ch}
		comps = e.ycbcrComponents(y, cb, cr, quality, chroma444)

	case *image.Gray:
		gray = true
		y := EncPlane{Pix: img.Pix[img.PixOffset(b.Min.X, b.Min.Y):], Stride: img.Stride, W: width, H: height}
		comps = e.grayComponents(y, quality)

	case *image.RGBA:
		yP, cbP, crP := e.planesFrom4Byte(img.Pix[img.PixOffset(b.Min.X, b.Min.Y):], img.Stride, width, height, chroma444)
		comps = e.ycbcrComponents(yP, cbP, crP, quality, chroma444)

	case *image.NRGBA:
		yP, cbP, crP := e.planesFrom4Byte(img.Pix[img.PixOffset(b.Min.X, b.Min.Y):], img.Stride, width, height, chroma444)
		comps = e.ycbcrComponents(yP, cbP, crP, quality, chroma444)

	default:
		rgba := e.drawRGBA(m, b)
		yP, cbP, crP := e.planesFrom4Byte(rgba.Pix, rgba.Stride, width, height, chroma444)
		comps = e.ycbcrComponents(yP, cbP, crP, quality, chroma444)
	}

	WriteHeader(&e.buf, HeaderParams{
		Width: width, Height: height, Quality: quality, Gray: gray, Chroma444: chroma444,
	})
	if err := WriteScan(ctx, &e.buf, width, height, comps); err != nil {
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
func (e *Encoder) ycbcrComponents(y, cb, cr EncPlane, quality int, chroma444 bool) []EncComponent {
	e.lumaQ = QuantNatural(LumaQuantTable(quality))
	e.chromaQ = QuantNatural(ChromaQuantTable(quality))
	yh, yv := 2, 2
	if chroma444 {
		yh, yv = 1, 1
	}
	e.comps[0] = EncComponent{Plane: y, HSamp: yh, VSamp: yv, Quant: &e.lumaQ, DC: &StdDCLumaCodes, AC: &StdACLumaCodes}
	e.comps[1] = EncComponent{Plane: cb, HSamp: 1, VSamp: 1, Quant: &e.chromaQ, DC: &StdDCChromaCodes, AC: &StdACChromaCodes}
	e.comps[2] = EncComponent{Plane: cr, HSamp: 1, VSamp: 1, Quant: &e.chromaQ, DC: &StdDCChromaCodes, AC: &StdACChromaCodes}
	return e.comps[:3]
}

func (e *Encoder) grayComponents(y EncPlane, quality int) []EncComponent {
	e.lumaQ = QuantNatural(LumaQuantTable(quality))
	e.comps[0] = EncComponent{Plane: y, HSamp: 1, VSamp: 1, Quant: &e.lumaQ, DC: &StdDCLumaCodes, AC: &StdACLumaCodes}
	return e.comps[:1]
}

// planesFrom4Byte converts RGB(A)-ordered pixels (alpha ignored) to
// Y/Cb/Cr planes (jccolor.c:70-73, 227-243), downsampling chroma unless
// chroma444. Scratch buffers are e's, reused across calls.
func (e *Encoder) planesFrom4Byte(pix []byte, stride, w, h int, chroma444 bool) (y, cb, cr EncPlane) {
	e.yBuf = GrowBytes(e.yBuf, w*h)
	yPlane := EncPlane{Pix: e.yBuf, Stride: w, W: w, H: h}
	if chroma444 {
		e.cbBuf = GrowBytes(e.cbBuf, w*h)
		e.crBuf = GrowBytes(e.crBuf, w*h)
		for row := 0; row < h; row++ {
			o := row * w
			rgbaToYCbCrRowGo(pix[row*stride:], e.yBuf[o:o+w], e.cbBuf[o:o+w], e.crBuf[o:o+w])
		}
		return yPlane, EncPlane{Pix: e.cbBuf, Stride: w, W: w, H: h}, EncPlane{Pix: e.crBuf, Stride: w, W: w, H: h}
	}

	// Chroma rows padded to the block edge (jcsample.c:279-280).
	fw := ceilDiv(w, 16) * 16
	cw, ch := fw/2, ceilDiv(h, 2)
	e.fullCbBuf = GrowBytes(e.fullCbBuf, 2*fw)
	e.fullCrBuf = GrowBytes(e.fullCrBuf, 2*fw)
	e.cbBuf = GrowBytes(e.cbBuf, cw*ch)
	e.crBuf = GrowBytes(e.crBuf, cw*ch)
	for oy := 0; oy < ch; oy++ {
		for k := 0; k < 2; k++ {
			row := min(2*oy+k, h-1) // odd last row repeats
			fcb, fcr := e.fullCbBuf[k*fw:k*fw+fw], e.fullCrBuf[k*fw:k*fw+fw]
			rgbaToYCbCrRowGo(pix[row*stride:], e.yBuf[row*w:row*w+w], fcb[:w], fcr[:w])
			for x := w; x < fw; x++ {
				fcb[x], fcr[x] = fcb[w-1], fcr[w-1]
			}
		}
		o := oy * cw
		downsampleH2V2RowGo(e.cbBuf[o:o+cw], e.fullCbBuf[:fw], e.fullCbBuf[fw:])
		downsampleH2V2RowGo(e.crBuf[o:o+cw], e.fullCrBuf[:fw], e.fullCrBuf[fw:])
	}
	return yPlane, EncPlane{Pix: e.cbBuf, Stride: cw, W: cw, H: ch}, EncPlane{Pix: e.crBuf, Stride: cw, W: cw, H: ch}
}
