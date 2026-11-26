package deflate

import (
	"context"
	"io"
	"sync"

	"github.com/dobbo-ca/deflate-go/internal/inflate"
)

// Decoder holds tables and scratch; the zero value is ready.
// Reuse one per goroutine for zero allocations per stream.
type Decoder struct{ d inflate.Decoder }

// Decode appends the inflated raw DEFLATE stream src to dst. Output
// and error equal io.ReadAll(flate.NewReader(bytes.NewReader(src))).
// Decode may write anywhere in dst[len(dst):cap(dst)]; use a full
// slice expression to share a buffer.
func (d *Decoder) Decode(dst, src []byte) ([]byte, error) {
	out, _, err := d.d.Decode(dst, src)
	return out, err
}

// DecodeLimit appends at most n bytes of the inflated stream src
// to dst. Output and error equal
// io.ReadAll(io.LimitReader(flate.NewReader(bytes.NewReader(src)), n)).
// It never writes dst past len(dst)+n.
func (d *Decoder) DecodeLimit(dst, src []byte, n int) ([]byte, error) {
	out, _, err := d.d.DecodeLimit(dst, src, n)
	if err == io.EOF {
		err = nil
	}
	return out, err
}

// DecodeContext is DecodeLimit that checks ctx between DEFLATE blocks
// and returns ctx.Err() with the output so far once ctx is done. A
// block in progress is not interrupted. Otherwise it equals DecodeLimit.
func (d *Decoder) DecodeContext(ctx context.Context, dst, src []byte, n int) ([]byte, error) {
	out, _, err := d.d.DecodeLimitContext(ctx, dst, src, n)
	if err == io.EOF {
		err = nil
	}
	return out, err
}

var pool = sync.Pool{New: func() any { return new(Decoder) }}

// Decode is Decoder.Decode with a pooled Decoder.
func Decode(dst, src []byte) ([]byte, error) {
	d := pool.Get().(*Decoder)
	out, err := d.Decode(dst, src)
	pool.Put(d)
	return out, err
}

// DecodeLimit is Decoder.DecodeLimit with a pooled Decoder.
func DecodeLimit(dst, src []byte, n int) ([]byte, error) {
	d := pool.Get().(*Decoder)
	out, err := d.DecodeLimit(dst, src, n)
	pool.Put(d)
	return out, err
}

// DecodeContext is Decoder.DecodeContext with a pooled Decoder.
func DecodeContext(ctx context.Context, dst, src []byte, n int) ([]byte, error) {
	d := pool.Get().(*Decoder)
	out, err := d.DecodeContext(ctx, dst, src, n)
	pool.Put(d)
	return out, err
}

// NewReader is flate.NewReader. The result also implements
// flate.Resetter.
func NewReader(r io.Reader) io.ReadCloser {
	return inflate.NewReader(r, nil)
}

// NewReaderDict is flate.NewReaderDict. The result also implements
// flate.Resetter.
func NewReaderDict(r io.Reader, dict []byte) io.ReadCloser {
	return inflate.NewReader(r, dict)
}
