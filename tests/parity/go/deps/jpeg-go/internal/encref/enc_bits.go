package encref

import (
	"bytes"
	"encoding/binary"
	"math/bits"
)

// bitWriter packs Huffman-coded bits MSB first, stuffs 0xFF data bytes
// with 0x00, and pads the final byte with 1-bits (jchuff.c:344-497).
// acc holds 64-free pending bits in its low end (jchuff.c put_buffer).
type bitWriter struct {
	buf  *bytes.Buffer
	out  []byte // pending output, flushed into buf
	acc  uint64
	free int        // 64 when empty
	vals [64]uint16 // writeBlock scratch, not zeroed per block
	nb   [64]uint8
}

// reserve makes room for n more output bytes, so spill rarely grows.
func (w *bitWriter) reserve(n int) {
	if cap(w.out)-len(w.out) >= n {
		return
	}
	w.buf.Write(w.out)
	w.buf.Grow(n)
	w.out = w.buf.AvailableBuffer()
}

// spill is PUT_BITS's full-buffer path (jchuff.c); free is already
// reduced by size and negative.
func (w *bitWriter) spill(acc uint64, free int, code uint64, size int) (uint64, int) {
	v := acc<<((size+free)&63) | code>>(-free&63)
	if y := ^v; (y-0x0101010101010101)&^y&0x8080808080808080 == 0 {
		w.out = binary.BigEndian.AppendUint64(w.out, v)
	} else {
		w.emitStuffed(v)
	}
	return code, free + 64
}

// emitStuffed is spill's 0xFF path, rare in real data.
func (w *bitWriter) emitStuffed(v uint64) {
	for sh := 56; sh >= 0; sh -= 8 {
		w.writeByte(byte(v >> sh))
	}
}

func (w *bitWriter) writeByte(b byte) {
	w.out = append(w.out, b)
	if b == 0xff {
		w.out = append(w.out, 0)
	}
}

// flush writes the pending bytes and pads the last with 1-bits
// (jchuff.c:494-497).
func (w *bitWriter) flush() {
	n := uint(64 - w.free)
	for n >= 8 {
		n -= 8
		w.writeByte(byte(w.acc >> n))
	}
	if n > 0 {
		pad := 8 - n
		w.writeByte(byte(w.acc<<pad | (1<<pad - 1)))
	}
	w.acc, w.free = 0, 64
	w.buf.Write(w.out)
	w.out = nil
}

// buildCodeArray indexes a EncHuffTable's canonical codes by symbol value for
// zero-alloc lookup during entropy coding.
func buildCodeArray(t EncHuffTable) [256]HuffCode {
	var out [256]HuffCode
	for sym, code := range t.Codes() {
		out[sym] = code
	}
	return out
}

// StdDCLumaCodes, StdACLumaCodes, StdDCChromaCodes, StdACChromaCodes are
// the standard tables' canonical codes, indexed by symbol value.
var (
	StdDCLumaCodes   = buildCodeArray(StdDCLuma)
	StdACLumaCodes   = buildCodeArray(StdACLuma)
	StdDCChromaCodes = buildCodeArray(StdDCChroma)
	StdACChromaCodes = buildCodeArray(StdACChroma)
)

// lowMask[n] is the low n bits; baseline value sizes are at most 16.
var lowMask = [17]uint64{0, 1, 3, 7, 0xf, 0x1f, 0x3f, 0x7f, 0xff, 0x1ff, 0x3ff, 0x7ff, 0xfff, 0x1fff, 0x3fff, 0x7fff, 0xffff}

// writeBlockGo entropy-codes one quantized block, natural order
// (jchuff.c:546-643): DC diff against lastDC, then AC in zigzag order with
// ZRL/EOB, walking the nonzero mask (jchuff-neon.c).
func writeBlockGo(bw *bitWriter, blk *[64]int16, lastDC *int32, dc, ac *[256]HuffCode) {
	vals, nb := &bw.vals, &bw.nb
	mask := huffPrepGo(blk, vals, nb) &^ 1
	acc, free := bw.acc, bw.free

	diff := int32(blk[0]) - *lastDC
	*lastDC = int32(blk[0])
	a := diff >> 31
	s := bits.Len32(uint32((diff ^ a) - a))
	c := dc[s]
	code, size := uint64(c.Code)<<(s&31)|uint64(uint32(diff+a))&lowMask[s], int(c.Size)+s
	if free -= size; free >= 0 {
		acc = acc<<(size&63) | code
	} else {
		acc, free = bw.spill(acc, free, code, size)
	}

	last := 0
	for mask != 0 {
		k := bits.TrailingZeros64(mask)
		mask &= mask - 1
		run := k - last - 1
		last = k
		for run >= 16 {
			c := ac[0xf0]
			code, size := uint64(c.Code), int(c.Size)
			if free -= size; free >= 0 {
				acc = acc<<(size&63) | code
			} else {
				acc, free = bw.spill(acc, free, code, size)
			}
			run -= 16
		}
		s := int(nb[k&63])
		c := ac[byte(run<<4)|byte(s)]
		code, size := uint64(c.Code)<<(s&31)|uint64(vals[k&63])&lowMask[s], int(c.Size)+s
		if free -= size; free >= 0 {
			acc = acc<<(size&63) | code
		} else {
			acc, free = bw.spill(acc, free, code, size)
		}
	}
	if last != 63 {
		c := ac[0]
		code, size := uint64(c.Code), int(c.Size)
		if free -= size; free >= 0 {
			acc = acc<<(size&63) | code
		} else {
			acc, free = bw.spill(acc, free, code, size)
		}
	}
	bw.acc, bw.free = acc, free
}
