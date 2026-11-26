package inflate

import (
	"bufio"
	"bytes"
	"compress/flate"
	"io"
	"strings"
)

const (
	winSize   = 128 << 10 // 32 KiB history plus decode area
	raBufSize = 16 << 10
)

// Reader is a streaming flate.Resetter decoder.
type Reader struct {
	f    Decoder
	rpos int // out[:rpos] has been returned
	rBuf *bufio.Reader
	win  []byte
}

// NewReader is flate.NewReaderDict.
func NewReader(r io.Reader, dict []byte) *Reader {
	z := new(Reader)
	_ = z.Reset(r, dict)
	return z
}

// Reset is flate.Resetter.Reset.
func (z *Reader) Reset(r io.Reader, dict []byte) error {
	f := &z.f
	f.reset()
	f.src = nil
	f.oneShot = false

	// makeReader, as stdlib.
	if rr, ok := r.(flate.Reader); ok {
		z.rBuf = nil
		f.r = rr
	} else {
		if z.rBuf != nil {
			z.rBuf.Reset(r)
		} else {
			z.rBuf = bufio.NewReader(r)
		}
		f.r = z.rBuf
	}
	f.br, f.ra = nil, nil
	switch rr := f.r.(type) {
	case *bufio.Reader:
		f.br = rr
	case *bytes.Reader, *strings.Reader: // exact: wrappers may override ReadByte
		f.ra = rr.(readAtSeeker)
		if f.raBuf == nil {
			f.raBuf = make([]byte, raBufSize)
		}
	}

	if z.win == nil {
		z.win = make([]byte, winSize)
	}
	if len(dict) > maxMatchOffset {
		dict = dict[len(dict)-maxMatchOffset:]
	}
	f.out = z.win
	f.wpos = copy(z.win, dict)
	f.histBase = 0
	z.rpos = f.wpos
	return nil
}

func (z *Reader) Read(b []byte) (int, error) {
	f := &z.f
	for {
		if z.rpos < f.wpos {
			// Stdlib's chunks end at each 32 KiB of history.
			v := int((f.shifted + int64(z.rpos)) & (maxMatchOffset - 1))
			n := copy(b, f.out[z.rpos:min(f.wpos, z.rpos+maxMatchOffset-v)])
			z.rpos += n
			// Stdlib flushes a full 32 KiB before it sees the error.
			if z.rpos == f.wpos && (f.shifted+int64(z.rpos))&(maxMatchOffset-1) != 0 {
				return n, f.err
			}
			return n, nil
		}
		if f.err != nil {
			return 0, f.err
		}
		if len(f.out)-f.wpos < maxMatchOffset {
			// Whole 32 KiB steps keep pauses on stdlib's flushes.
			k := (f.wpos - maxMatchOffset) &^ (maxMatchOffset - 1)
			copy(f.out, f.out[k:f.wpos])
			f.wpos -= k
			z.rpos = f.wpos
			f.shifted += int64(k)
		}
		f.raLen = 0 // source may change between Reads
		f.paused = false
		for f.err == nil && !f.paused {
			f.step(f)
		}
	}
}

// Close is stdlib's Close.
func (z *Reader) Close() error {
	if z.f.err == io.EOF {
		return nil
	}
	return z.f.err
}

// fastInput returns unconsumed input at roffset.
func (f *Decoder) fastInput() []byte {
	switch {
	case f.r == nil:
		return f.src[f.roffset:]
	case f.br != nil:
		n := f.br.Buffered()
		if n <= fastInMargin {
			return nil
		}
		// Peek clears lastByte; re-read it later.
		f.brBack = f.br.UnreadByte() == nil
		if f.brBack {
			b, _ := f.br.Peek(n + 1)
			return b[1:]
		}
		b, _ := f.br.Peek(n)
		return b
	case f.ra != nil:
		cur, err := f.ra.Seek(0, io.SeekCurrent)
		if err != nil {
			return nil
		}
		if cur < f.raOff || cur+fastInMargin >= f.raOff+int64(f.raLen) {
			n, _ := f.ra.ReadAt(f.raBuf, cur)
			f.raOff, f.raLen = cur, n
		}
		f.raCur = cur
		return f.raBuf[cur-f.raOff : f.raLen]
	}
	return nil
}

// fastConsume consumes n pulled bytes.
func (f *Decoder) fastConsume(n int) {
	switch {
	case f.r == nil:
	case f.br != nil:
		if f.brBack {
			n++
		}
		if n > 0 {
			_, _ = f.br.Discard(n - 1)
			_, _ = f.br.ReadByte()
		}
	case f.ra != nil:
		_, _ = f.ra.Seek(f.raCur+int64(n), io.SeekStart)
	}
}
