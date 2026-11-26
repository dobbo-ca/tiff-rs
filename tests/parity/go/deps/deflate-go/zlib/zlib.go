// Copyright 2009 The Go Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSE file.

// Package zlib decodes zlib (RFC 1950) streams. Output and errors
// equal compress/zlib; the reader is a port of its reader.go.
package zlib

import (
	"bufio"
	"compress/flate"
	"compress/zlib"
	"encoding/binary"
	"io"
	"runtime"
	"sync"

	"github.com/dobbo-ca/deflate-go/internal/adler32"
	"github.com/dobbo-ca/deflate-go/internal/inflate"
)

// fuseArch: fusing gains 3-6% on c7i, loses 1-3% on c7g.
const fuseArch = runtime.GOARCH == "amd64"

const (
	zlibDeflate   = 8
	zlibMaxWindow = 7
)

// Decoder holds tables and scratch; the zero value is ready.
// Reuse one per goroutine for zero allocations per stream.
type Decoder struct{ d inflate.Decoder }

// Decode appends the inflated zlib stream src to dst. Output and
// error equal io.ReadAll of zlib.NewReader(bytes.NewReader(src)),
// with no output when NewReader fails.
// Decode may write anywhere in dst[len(dst):cap(dst)]; use a full
// slice expression to share a buffer.
func (d *Decoder) Decode(dst, src []byte) ([]byte, error) {
	return d.decode(dst, src, -1)
}

// DecodeLimit appends at most n bytes of the inflated zlib stream
// src to dst. Output and error equal io.ReadAll of
// io.LimitReader(zlib.NewReader(bytes.NewReader(src)), n), with no
// output when NewReader fails. A stop at n skips the adler32.
// It never writes dst past len(dst)+n.
func (d *Decoder) DecodeLimit(dst, src []byte, n int) ([]byte, error) {
	return d.decode(dst, src, max(n, 0))
}

// decode is Decode for n < 0, else DecodeLimit.
func (d *Decoder) decode(dst, src []byte, n int) ([]byte, error) {
	if len(src) < 2 {
		return dst, io.ErrUnexpectedEOF
	}
	h := binary.BigEndian.Uint16(src)
	if (src[0]&0x0f != zlibDeflate) || (src[0]>>4 > zlibMaxWindow) || (h%31 != 0) {
		return dst, zlib.ErrHeader
	}
	off := 2
	if src[1]&0x20 != 0 {
		if len(src) < 6 {
			return dst, io.ErrUnexpectedEOF
		}
		// Decode has no dict: adler32(nil).
		if binary.BigEndian.Uint32(src[2:]) != adler32.Update(1, nil) {
			return dst, zlib.ErrDictionary
		}
		off = 6
	}
	if n == 0 {
		return dst, nil
	}
	var out []byte
	var c int
	var err error
	// A stop at n skips the trailer, so a fused sum is wasted there. n
	// % 32768 == 0 is stdlib's window flush: tiles that long normally
	// stop at n, so keep the post-pass for a stream ending before n.
	fused := fuseArch && (n < 0 || n%32768 != 0)
	d.d.FuseAdler = fused
	if n < 0 {
		out, c, err = d.d.Decode(dst, src[off:])
	} else {
		out, c, err = d.d.DecodeLimit(dst, src[off:], n)
		if err == nil {
			return out, nil // stopped at n: no adler32
		}
		if err == io.EOF {
			err = nil
		}
	}
	if err != nil {
		return out, err
	}
	rest := src[off+c:]
	if len(rest) < 4 {
		return out, io.ErrUnexpectedEOF
	}
	sum := d.d.Adler
	if !fused {
		sum = adler32.Update(1, out[len(dst):])
	}
	if binary.BigEndian.Uint32(rest) != sum {
		return out, zlib.ErrChecksum
	}
	return out, nil
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

type reader struct {
	r            flate.Reader
	decompressor *inflate.Reader
	digest       uint32
	err          error
	scratch      [4]byte
}

// NewReader is zlib.NewReader. The result also implements
// zlib.Resetter.
func NewReader(r io.Reader) (io.ReadCloser, error) {
	return NewReaderDict(r, nil)
}

// NewReaderDict is zlib.NewReaderDict. The result also implements
// zlib.Resetter.
func NewReaderDict(r io.Reader, dict []byte) (io.ReadCloser, error) {
	z := new(reader)
	err := z.Reset(r, dict)
	if err != nil {
		return nil, err
	}
	return z, nil
}

func (z *reader) Read(p []byte) (int, error) {
	if z.err != nil {
		return 0, z.err
	}

	var n int
	n, z.err = z.decompressor.Read(p)
	z.digest = adler32.Update(z.digest, p[0:n])
	if z.err != io.EOF {
		// In the normal case we return here.
		return n, z.err
	}

	// Finished file; check checksum.
	if _, err := io.ReadFull(z.r, z.scratch[0:4]); err != nil {
		if err == io.EOF {
			err = io.ErrUnexpectedEOF
		}
		z.err = err
		return n, z.err
	}
	// ZLIB (RFC 1950) is big-endian, unlike GZIP (RFC 1952).
	checksum := binary.BigEndian.Uint32(z.scratch[:4])
	if checksum != z.digest {
		z.err = zlib.ErrChecksum
		return n, z.err
	}
	return n, io.EOF
}

// Calling Close does not close the wrapped [io.Reader] originally passed to [NewReader].
// In order for the ZLIB checksum to be verified, the reader must be
// fully consumed until the [io.EOF].
func (z *reader) Close() error {
	if z.err != nil && z.err != io.EOF {
		return z.err
	}
	z.err = z.decompressor.Close()
	return z.err
}

func (z *reader) Reset(r io.Reader, dict []byte) error {
	*z = reader{decompressor: z.decompressor, digest: z.digest}
	if fr, ok := r.(flate.Reader); ok {
		z.r = fr
	} else {
		z.r = bufio.NewReader(r)
	}

	// Read the header (RFC 1950 section 2.2.).
	_, z.err = io.ReadFull(z.r, z.scratch[0:2])
	if z.err != nil {
		if z.err == io.EOF {
			z.err = io.ErrUnexpectedEOF
		}
		return z.err
	}
	h := binary.BigEndian.Uint16(z.scratch[:2])
	if (z.scratch[0]&0x0f != zlibDeflate) || (z.scratch[0]>>4 > zlibMaxWindow) || (h%31 != 0) {
		z.err = zlib.ErrHeader
		return z.err
	}
	haveDict := z.scratch[1]&0x20 != 0
	if haveDict {
		_, z.err = io.ReadFull(z.r, z.scratch[0:4])
		if z.err != nil {
			if z.err == io.EOF {
				z.err = io.ErrUnexpectedEOF
			}
			return z.err
		}
		checksum := binary.BigEndian.Uint32(z.scratch[:4])
		if checksum != adler32.Update(1, dict) {
			z.err = zlib.ErrDictionary
			return z.err
		}
	}

	if z.decompressor == nil {
		if haveDict {
			z.decompressor = inflate.NewReader(z.r, dict)
		} else {
			z.decompressor = inflate.NewReader(z.r, nil)
		}
	} else {
		if err := z.decompressor.Reset(z.r, dict); err != nil {
			z.err = err
			return err
		}
	}
	z.digest = 1
	return nil
}
