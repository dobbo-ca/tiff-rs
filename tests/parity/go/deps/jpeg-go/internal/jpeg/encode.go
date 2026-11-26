package jpeg

// Uniform tile fast path (T14): a whole-tile-one-colour image skips colour
// conversion, downsampling, FDCT and quantisation, and the Encoder caches
// its last uniform output for a repeat hit.

import (
	"bytes"
	"context"
	"image"

	"github.com/dobbo-ca/jpeg-go/internal/codec"
)

type uniformKind uint8

const (
	uniformKindRGB uniformKind = iota
	uniformKindGray
	uniformKindYCbCr
)

// uniformKey caches a uniform-tile encode (pixel, width, height, quality,
// sampling), per T14 spec. kind keeps identical numeric samples from
// different colour spaces from colliding.
type uniformKey struct {
	kind          uniformKind
	c0, c1, c2    byte
	width, height int
	quality       int
	chroma444     bool
}

// uniformKeyFor reports whether m is a single solid colour, per input
// type, by comparing each row against a row built from the first pixel
// (T14 spec). chroma444 is the caller's requested sampling, overridden by
// a *image.YCbCr's own ratio like the general path does.
func (e *Encoder) uniformKeyFor(m image.Image, quality int, chroma444 bool) (uniformKey, bool) {
	b := m.Bounds()
	w, h := b.Dx(), b.Dy()
	if w == 0 || h == 0 {
		return uniformKey{}, false
	}

	switch img := m.(type) {
	case *image.RGBA:
		r, g, bl, ok := e.uniformRGBRow(img.Pix[img.PixOffset(b.Min.X, b.Min.Y):], img.Stride, w, h)
		if !ok {
			return uniformKey{}, false
		}
		return uniformKey{kind: uniformKindRGB, c0: r, c1: g, c2: bl, width: w, height: h, quality: quality, chroma444: chroma444}, true

	case *image.NRGBA:
		r, g, bl, ok := e.uniformRGBRow(img.Pix[img.PixOffset(b.Min.X, b.Min.Y):], img.Stride, w, h)
		if !ok {
			return uniformKey{}, false
		}
		return uniformKey{kind: uniformKindRGB, c0: r, c1: g, c2: bl, width: w, height: h, quality: quality, chroma444: chroma444}, true

	case *image.Gray:
		y, ok := e.uniformByteRow(img.Pix[img.PixOffset(b.Min.X, b.Min.Y):], img.Stride, w, h)
		if !ok {
			return uniformKey{}, false
		}
		return uniformKey{kind: uniformKindGray, c0: y, width: w, height: h, quality: quality}, true

	case *image.YCbCr:
		if !directYCbCr(img) {
			return uniformKey{}, false
		}
		is444 := img.SubsampleRatio == image.YCbCrSubsampleRatio444
		y, ok := e.uniformByteRow(img.Y[img.YOffset(b.Min.X, b.Min.Y):], img.YStride, w, h)
		if !ok {
			return uniformKey{}, false
		}
		cw, ch := w, h
		if !is444 {
			cw, ch = ceilDiv(w, 2), ceilDiv(h, 2)
		}
		cOff := img.COffset(b.Min.X, b.Min.Y)
		cb, ok := e.uniformByteRow(img.Cb[cOff:], img.CStride, cw, ch)
		if !ok {
			return uniformKey{}, false
		}
		cr, ok := e.uniformByteRow(img.Cr[cOff:], img.CStride, cw, ch)
		if !ok {
			return uniformKey{}, false
		}
		return uniformKey{kind: uniformKindYCbCr, c0: y, c1: cb, c2: cr, width: w, height: h, quality: quality, chroma444: is444}, true
	}
	return uniformKey{}, false
}

// uniformRGBRow checks every pixel (including alpha) equals the first,
// using bytes.Equal against a row built once (T14 spec).
func (e *Encoder) uniformRGBRow(pix []byte, stride, w, h int) (r, g, bl byte, ok bool) {
	r, g, bl, a := pix[0], pix[1], pix[2], pix[3]
	e.uniformRow = codec.GrowBytes(e.uniformRow, w*4)
	for x := 0; x < w; x++ {
		e.uniformRow[x*4], e.uniformRow[x*4+1], e.uniformRow[x*4+2], e.uniformRow[x*4+3] = r, g, bl, a
	}
	for y := 0; y < h; y++ {
		if !bytes.Equal(pix[y*stride:y*stride+w*4], e.uniformRow) {
			return 0, 0, 0, false
		}
	}
	return r, g, bl, true
}

// uniformByteRow checks every sample in a single-byte-per-pixel plane
// (Gray, or one YCbCr plane) equals the first, via bytes.Equal.
func (e *Encoder) uniformByteRow(pix []byte, stride, w, h int) (v byte, ok bool) {
	v = pix[0]
	e.uniformRow = codec.GrowBytes(e.uniformRow, w)
	for x := range e.uniformRow {
		e.uniformRow[x] = v
	}
	for y := 0; y < h; y++ {
		if !bytes.Equal(pix[y*stride:y*stride+w], e.uniformRow) {
			return 0, false
		}
	}
	return v, true
}

// encodeUniform writes key's encode, from cache on a repeat key or
// otherwise through one flat-block colour/FDCT/quant per component plus
// the normal entropy coder (T14 spec).
func (e *Encoder) encodeUniform(ctx context.Context, key uniformKey) error {
	if e.uniformValid && key == e.uniformKey {
		if err := ctx.Err(); err != nil {
			return err
		}
		e.buf.Write(e.uniformOut)
		return nil
	}

	gray := key.kind == uniformKindGray
	var y, cb, cr byte
	switch key.kind {
	case uniformKindGray:
		y = key.c0
	case uniformKindYCbCr:
		y, cb, cr = key.c0, key.c1, key.c2
	case uniformKindRGB:
		y, cb, cr = codec.RGBToYCbCr(int32(key.c0), int32(key.c1), int32(key.c2))
	}

	e.lumaQ = codec.QuantNatural(codec.LumaQuantTable(key.quality))
	e.comps[0] = codec.EncComponent{HSamp: 1, VSamp: 1, Quant: &e.lumaQ, DC: &codec.StdDCLumaCodes, AC: &codec.StdACLumaCodes}
	var dc [3]int32
	dc[0] = codec.FlatBlockDC(y, &e.lumaQ)

	var comps []codec.EncComponent
	if gray {
		comps = e.comps[:1]
	} else {
		yh, yv := 2, 2
		if key.chroma444 {
			yh, yv = 1, 1
		}
		e.comps[0].HSamp, e.comps[0].VSamp = yh, yv
		e.chromaQ = codec.QuantNatural(codec.ChromaQuantTable(key.quality))
		e.comps[1] = codec.EncComponent{HSamp: 1, VSamp: 1, Quant: &e.chromaQ, DC: &codec.StdDCChromaCodes, AC: &codec.StdACChromaCodes}
		e.comps[2] = codec.EncComponent{HSamp: 1, VSamp: 1, Quant: &e.chromaQ, DC: &codec.StdDCChromaCodes, AC: &codec.StdACChromaCodes}
		dc[1] = codec.FlatBlockDC(cb, &e.chromaQ)
		dc[2] = codec.FlatBlockDC(cr, &e.chromaQ)
		comps = e.comps[:3]
	}

	codec.WriteHeader(&e.buf, codec.HeaderParams{
		Width: key.width, Height: key.height, Quality: key.quality, Gray: gray, Chroma444: key.chroma444,
	})
	if err := codec.WriteUniformScan(ctx, &e.buf, key.width, key.height, comps, dc[:len(comps)]); err != nil {
		return err
	}
	e.buf.Write([]byte{0xff, 0xd9}) // EOI

	e.uniformKey = key
	e.uniformValid = true
	e.uniformOut = append(e.uniformOut[:0], e.buf.Bytes()...)
	return nil
}
