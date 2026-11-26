// Copyright 2009 The Go Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSE file.

// Package inflate is the DEFLATE decoder behind packages deflate
// and zlib.
//
// This file is a port of Go 1.27 compress/flate/inflate.go and
// dict_decoder.go: the careful path. It owns every error and every
// input read decision, so both equal compress/flate. The fast loop
// (fast_generic.go) decodes common symbols in between and hands
// back the exact stdlib state (design.md 4.1).
package inflate

import (
	"bufio"
	"compress/flate"
	"context"
	"errors"
	"io"
	"math"
	"math/bits"
	"slices"
	"sync"
	"unsafe"

	"github.com/dobbo-ca/deflate-go/internal/adler32"
)

const (
	maxCodeLen = 16 // max length of Huffman code
	// The next three numbers come from the RFC section 3.2.7, with the
	// additional proviso in section 3.2.5 which implies that distance codes
	// 30 and 31 should never occur in compressed data.
	maxNumLit  = 286
	maxNumDist = 30
	numCodes   = 19 // number of codes in Huffman meta-code

	endBlockMarker = 256
	maxMatchOffset = 1 << 15 // stdlib window size
)

// Build the fixed tables only once upon first use.
var fixedOnce sync.Once

// The data structure for decoding Huffman tables is based on that of
// zlib. There is a lookup table of a fixed bit width (huffmanChunkBits),
// For codes smaller than the table width, there are multiple entries
// (each combination of trailing bits has the same value). For codes
// larger than the table width, the table contains a link to an overflow
// table. The width of each entry in the link table is the maximum code
// size minus the chunk width.
//
// Note that you can do a lookup in the table even without all bits
// filled. Since the extra bits are zero, and the DEFLATE Huffman codes
// have the property that shorter codes come before longer ones, the
// bit length estimate in the result is a lower bound on the actual
// number of bits.
//
// See the following:
//	https://github.com/madler/zlib/raw/master/doc/algorithm.txt

// chunk & 15 is number of bits
// chunk >> 4 is value, including table link

const (
	huffmanChunkBits  = 9
	huffmanNumChunks  = 1 << huffmanChunkBits
	huffmanCountMask  = 15
	huffmanValueShift = 4
)

type huffmanDecoder struct {
	min      int                      // the minimum code length
	chunks   [huffmanNumChunks]uint32 // chunks as described above
	links    []uint32                 // overflow links, 1<<linkBits per link table
	linkBits uint
	linkMask uint32 // mask the width of the link table
}

// Initialize Huffman decoding tables from array of code lengths.
// Following this function, h is guaranteed to be initialized into a complete
// tree (i.e., neither over-subscribed nor under-subscribed). The exception is a
// degenerate case where the tree has only a single symbol with length 1. Empty
// trees are permitted.
func (h *huffmanDecoder) init(lengths []int) bool {
	if h.min != 0 {
		*h = huffmanDecoder{links: h.links} // keep link storage: no allocs
	}

	// Count number of codes of each length,
	// compute min and max length.
	var count [maxCodeLen]int
	var min, max int
	for _, n := range lengths {
		if n == 0 {
			continue
		}
		if min == 0 || n < min {
			min = n
		}
		if n > max {
			max = n
		}
		count[n]++
	}

	// Empty tree. The decompressor.huffSym function will fail later if the tree
	// is used. Technically, an empty tree is only valid for the HDIST tree and
	// not the HCLEN and HLIT tree. However, a stream with an empty HCLEN tree
	// is guaranteed to fail since it will attempt to use the tree to decode the
	// codes for the HLIT and HDIST trees. Similarly, an empty HLIT tree is
	// guaranteed to fail later since the compressed data section must be
	// composed of at least one symbol (the end-of-block marker).
	if max == 0 {
		return true
	}

	code := 0
	var nextcode [maxCodeLen]int
	for i := min; i <= max; i++ {
		code <<= 1
		nextcode[i] = code
		code += count[i]
	}

	// Check that the coding is complete (i.e., that we've
	// assigned all 2-to-the-max possible bit sequences).
	// Exception: To be compatible with zlib, we also need to
	// accept degenerate single-code codings. See also
	// TestDegenerateHuffmanCoding.
	if code != 1<<uint(max) && (code != 1 || max != 1) {
		return false
	}

	h.min = min
	if max > huffmanChunkBits {
		numLinks := 1 << (uint(max) - huffmanChunkBits)
		h.linkMask = uint32(numLinks - 1)
		h.linkBits = uint(max) - huffmanChunkBits

		// create link tables
		link := nextcode[huffmanChunkBits+1] >> 1
		n := (huffmanNumChunks - link) * numLinks
		if cap(h.links) < n {
			h.links = make([]uint32, n)
		}
		h.links = h.links[:n]
		clear(h.links)
		for j := uint(link); j < huffmanNumChunks; j++ {
			reverse := int(bits.Reverse16(uint16(j)))
			reverse >>= uint(16 - huffmanChunkBits)
			off := j - uint(link)
			h.chunks[reverse] = uint32(off<<huffmanValueShift | (huffmanChunkBits + 1))
		}
	}

	for i, n := range lengths {
		if n == 0 {
			continue
		}
		code := nextcode[n]
		nextcode[n]++
		chunk := uint32(i<<huffmanValueShift | n)
		reverse := int(bits.Reverse16(uint16(code)))
		reverse >>= uint(16 - n)
		if n <= huffmanChunkBits {
			for off := reverse; off < len(h.chunks); off += 1 << uint(n) {
				h.chunks[off] = chunk
			}
		} else {
			j := reverse & (huffmanNumChunks - 1)
			value := h.chunks[j] >> huffmanValueShift
			linktab := h.links[value<<h.linkBits : (value+1)<<h.linkBits]
			reverse >>= huffmanChunkBits
			for off := reverse; off < len(linktab); off += 1 << uint(n-huffmanChunkBits) {
				linktab[off] = chunk
			}
		}
	}

	return true
}

// acceptLengths is init's accept decision without the tables.
// min is init's h.min: 0 for an empty tree.
func acceptLengths(lengths []int) (min int, ok bool) {
	var count [maxCodeLen]int
	var max int
	for _, n := range lengths {
		if n == 0 {
			continue
		}
		if min == 0 || n < min {
			min = n
		}
		if n > max {
			max = n
		}
		count[n]++
	}
	if max == 0 {
		return 0, true
	}
	code := 0
	for i := min; i <= max; i++ {
		code = code<<1 + count[i]
	}
	return min, code == 1<<uint(max) || code == 1 && max == 1
}

type readAtSeeker interface {
	io.ReaderAt
	io.Seeker
}

// Decoder is stdlib's decompressor plus the output buffer, the
// input modes and the fast tables. The zero value is ready.
type Decoder struct {
	// Input source.
	src     []byte // one-shot input, indexed by roffset
	r       flate.Reader
	br      *bufio.Reader // r, fast input via Peek/Discard
	brBack  bool          // fast input follows an unread byte
	ra      readAtSeeker  // r, fast input via ReadAt/Seek
	raBuf   []byte
	raOff   int64 // stream position of raBuf[0]
	raLen   int
	raCur   int64 // fast window start
	roffset int64

	// Input bits, in top of b.
	b  uint32
	nb uint

	// Huffman decoder for code lengths.
	h1 huffmanDecoder

	// Length arrays used to define Huffman codes.
	bits     [maxNumLit + maxNumDist]int
	codebits [numCodes]int

	// Output; out[histBase:wpos] is history.
	out      []byte
	wpos     int
	histBase int
	shifted  int64 // bytes slid off the front (Reader)
	oneShot  bool  // out grows instead of pausing
	limit    int   // one-shot: wpos stops here
	paused   bool  // Reader: return output now

	// One-shot fused adler32: Adler covers out[histBase:sumPos].
	FuseAdler bool   // set before Decode: keep Adler
	Adler     uint32 // valid after Decode returns io.EOF
	sumPos    int

	// Temporary buffer (avoids repeated allocation).
	buf [4]byte

	// Next step in the decompression,
	// and decompression state.
	step      func(*Decoder)
	stepState int
	final     bool
	err       error
	ctx       context.Context // DecodeLimitContext only; checked at each block
	copyLen   int
	copyDist  int

	// Block's tables, for both paths; llMin is stdlib's h1.min.
	ll    *litlenTable
	dt    *distTable
	llMin uint
	llBuf litlenTable
	dtBuf distTable
	pr    *pairTable // nil: primary entries
	prBuf pairTable
	fs    fastState // in Decoder: &fs must not escape
}

func (f *Decoder) nextBlock() {
	if f.ctx != nil {
		if f.err = f.ctx.Err(); f.err != nil {
			return
		}
	}
	for f.nb < 1+2 {
		if f.err = f.moreBits(); f.err != nil {
			return
		}
	}
	f.final = f.b&1 == 1
	f.b >>= 1
	typ := f.b & 3
	f.b >>= 2
	f.nb -= 1 + 2
	switch typ {
	case 0:
		f.dataBlock()
	case 1:
		// compressed, fixed Huffman tables
		f.ll, f.dt, f.llMin = &fixedLL, &fixedDT, 7
		f.pr = nil // 8-bit literals never pair
		f.huffmanBlock()
	case 2:
		// compressed, dynamic Huffman tables
		if f.err = f.readHuffman(); f.err != nil {
			break
		}
		f.ll, f.dt = &f.llBuf, &f.dtBuf
		f.huffmanBlock()
	default:
		// 3 is reserved.
		f.err = flate.CorruptInputError(f.roffset)
	}
}

// RFC 1951 section 3.2.7.
// Compression with dynamic Huffman codes

var codeOrder = [...]int{16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15}

func (f *Decoder) readHuffman() error {
	// HLIT[5], HDIST[5], HCLEN[4].
	for f.nb < 5+5+4 {
		if err := f.moreBits(); err != nil {
			return err
		}
	}
	nlit := int(f.b&0x1F) + 257
	if nlit > maxNumLit {
		return flate.CorruptInputError(f.roffset)
	}
	f.b >>= 5
	ndist := int(f.b&0x1F) + 1
	if ndist > maxNumDist {
		return flate.CorruptInputError(f.roffset)
	}
	f.b >>= 5
	nclen := int(f.b&0xF) + 4
	// numCodes is 19, so nclen is always valid.
	f.b >>= 4
	f.nb -= 5 + 5 + 4

	// (HCLEN+4)*3 bits: code lengths in the magic codeOrder order.
	for i := 0; i < nclen; i++ {
		for f.nb < 3 {
			if err := f.moreBits(); err != nil {
				return err
			}
		}
		f.codebits[codeOrder[i]] = int(f.b & 0x7)
		f.b >>= 3
		f.nb -= 3
	}
	for i := nclen; i < len(codeOrder); i++ {
		f.codebits[codeOrder[i]] = 0
	}
	if !f.h1.init(f.codebits[0:]) {
		return flate.CorruptInputError(f.roffset)
	}

	// HLIT + 257 code lengths, HDIST + 1 code lengths,
	// using the code length Huffman code.
	for i, n := 0, nlit+ndist; i < n; {
		x, err := f.huffSym(&f.h1)
		if err != nil {
			return err
		}
		if x < 16 {
			// Actual length.
			f.bits[i] = x
			i++
			continue
		}
		// Repeat previous length or zero.
		var rep int
		var nb uint
		var b int
		switch x {
		default:
			return flate.InternalError("unexpected length code")
		case 16:
			rep = 3
			nb = 2
			if i == 0 {
				return flate.CorruptInputError(f.roffset)
			}
			b = f.bits[i-1]
		case 17:
			rep = 3
			nb = 3
			b = 0
		case 18:
			rep = 11
			nb = 7
			b = 0
		}
		for f.nb < nb {
			if err := f.moreBits(); err != nil {
				return err
			}
		}
		rep += int(f.b & uint32(1<<nb-1))
		f.b >>= nb
		f.nb -= nb
		if i+rep > n {
			return flate.CorruptInputError(f.roffset)
		}
		for j := 0; j < rep; j++ {
			f.bits[i] = b
			i++
		}
	}

	minLL, okLL := acceptLengths(f.bits[0:nlit])
	_, okDist := acceptLengths(f.bits[nlit : nlit+ndist])
	if !okLL || !okDist {
		return flate.CorruptInputError(f.roffset)
	}

	// As an optimization, we can initialize the min bits to read at a time
	// for the HLIT tree to the length of the EOB marker since we know that
	// every block must terminate with one. This preserves the property that
	// we never read any extra bytes after the end of the DEFLATE stream.
	if minLL < f.bits[endBlockMarker] {
		minLL = f.bits[endBlockMarker]
	}

	f.llMin = uint(minLL)
	buildTable(f.llBuf[:], f.bits[:nlit], llSyms[:], llBits, f.llMin)
	buildTable(f.dtBuf[:], f.bits[nlit:nlit+ndist], distSyms[:], distBits, 0)
	f.pr = nil
	if pairLoop && buildPairs(&f.prBuf, &f.llBuf, f.bits[:nlit]) {
		f.pr = &f.prBuf
	}
	return nil
}

// Decode a single Huffman block from f, using the tables ll and dt.
func (f *Decoder) huffmanBlock() {
	const (
		stateInit = iota // Zero value must be stateInit
		stateDict
	)

	switch f.stepState {
	case stateInit:
		goto readLiteral
	case stateDict:
		goto copyHistory
	}

readLiteral:
	if f.wpos <= len(f.out)-fastOutMargin {
		f.fastRun()
	}
	// Read literal and/or (length, distance) according to RFC section 3.2.3.
	{
		e, err := f.tableSym(f.ll[:], llBits, f.llMin)
		if err != nil {
			f.err = err
			return
		}
		var n uint // number of bits extra
		var length int
		switch {
		case e&litFlag != 0:
			if !f.writeByte(byte(e >> 16)) {
				return
			}
			if f.availWrite() == 0 {
				f.pause(stateInit)
				return
			}
			goto readLiteral
		// otherwise, reference to older data
		case e&stopFlag == 0:
			length = int(e >> 16)
			n = uint(e&31 - e>>8&15)
		case e>>16 == endBlockMarker:
			f.finishBlock()
			return
		default: // 286, 287
			f.err = flate.CorruptInputError(f.roffset)
			return
		}
		if n > 0 {
			for f.nb < n {
				if err = f.moreBits(); err != nil {
					f.err = err
					return
				}
			}
			length += int(f.b & uint32(1<<n-1))
			f.b >>= n
			f.nb -= n
		}

		// Distance minimum 0: same reads (design.md 4.1).
		d, err := f.tableSym(f.dt[:], distBits, 0)
		if err != nil {
			f.err = err
			return
		}
		if d&stopFlag != 0 { // 30, 31
			f.err = flate.CorruptInputError(f.roffset)
			return
		}
		dist := int(d >> 16)
		if nb := uint(d&31 - d>>8&15); nb > 0 {
			for f.nb < nb {
				if err = f.moreBits(); err != nil {
					f.err = err
					return
				}
			}
			dist += int(f.b & uint32(1<<nb-1))
			f.b >>= nb
			f.nb -= nb
		}

		// No check on length; encoding can be prescient.
		if dist > f.histSize() {
			f.err = flate.CorruptInputError(f.roffset)
			return
		}

		f.copyLen, f.copyDist = length, dist
		goto copyHistory
	}

copyHistory:
	// Perform a backwards copy according to RFC section 3.2.3.
	{
		f.writeCopy()
		if f.availWrite() == 0 || f.copyLen > 0 {
			f.pause(stateDict)
			return
		}
		goto readLiteral
	}
}

// pause stops a Reader with full output.
func (f *Decoder) pause(state int) {
	f.step = (*Decoder).huffmanBlock
	f.stepState = state
	f.paused = true
}

// Copy a single uncompressed data block from input to output.
func (f *Decoder) dataBlock() {
	// Uncompressed.
	// Discard current half-byte.
	f.nb = 0
	f.b = 0

	// Length then ones-complement of length.
	nr, err := f.readFull(f.buf[0:4])
	f.roffset += int64(nr)
	if err != nil {
		f.err = noEOF(err)
		return
	}
	n := int(f.buf[0]) | int(f.buf[1])<<8
	nn := int(f.buf[2]) | int(f.buf[3])<<8
	if uint16(nn) != uint16(^n) {
		f.err = flate.CorruptInputError(f.roffset)
		return
	}

	if n == 0 {
		if f.oneShot && f.wpos == f.limit && !f.final {
			f.err = errLimit // stdlib flushes here
			return
		}
		f.finishBlock()
		// Sync flush: stdlib's Read stops here too.
		f.paused = !f.oneShot
		return
	}

	f.copyLen = n
	f.copyData()
}

// copyData copies f.copyLen bytes from the input.
// Readers use stdlib's window chunks: same Read calls.
func (f *Decoder) copyData() {
	for f.copyLen > 0 {
		n := f.copyLen
		if f.oneShot {
			if f.wpos == len(f.out) {
				if f.roffset == int64(len(f.src)) {
					f.err = io.ErrUnexpectedEOF // no input: don't grow
					return
				}
				if !f.grow() {
					return
				}
			}
			n = min(n, len(f.out)-f.wpos)
		} else {
			vpos := int((f.shifted + int64(f.wpos)) & (maxMatchOffset - 1))
			n = min(n, maxMatchOffset-vpos)
			if len(f.out)-f.wpos < n {
				f.step = (*Decoder).copyData
				f.paused = true
				return
			}
		}
		cnt, err := f.readFull(f.out[f.wpos : f.wpos+n])
		f.roffset += int64(cnt)
		f.copyLen -= cnt
		f.wpos += cnt
		if err != nil {
			f.err = noEOF(err)
			return
		}
	}
	// Full window: stdlib flushes before the next block.
	if !f.oneShot && f.wpos == len(f.out) {
		f.step = (*Decoder).copyData
		f.paused = true
		return
	}
	f.finishBlock()
}

func (f *Decoder) finishBlock() {
	if f.final {
		f.err = io.EOF
	}
	f.step = (*Decoder).nextBlock
}

// noEOF returns err, unless err == io.EOF, in which case it returns io.ErrUnexpectedEOF.
func noEOF(e error) error {
	if e == io.EOF {
		return io.ErrUnexpectedEOF
	}
	return e
}

// readByte is f.r.ReadByte, or src[roffset] for one-shot.
func (f *Decoder) readByte() (byte, error) {
	if f.r == nil {
		if f.roffset < int64(len(f.src)) {
			return f.src[f.roffset], nil
		}
		return 0, io.EOF
	}
	return f.r.ReadByte()
}

// readFull is io.ReadFull(f.r, p), or the bytes.Reader result.
func (f *Decoder) readFull(p []byte) (int, error) {
	if f.r == nil {
		n := copy(p, f.src[f.roffset:])
		switch {
		case n == len(p):
			return n, nil
		case n == 0:
			return 0, io.EOF
		}
		return n, io.ErrUnexpectedEOF
	}
	return io.ReadFull(f.r, p)
}

func (f *Decoder) moreBits() error {
	c, err := f.readByte()
	if err != nil {
		return noEOF(err)
	}
	f.roffset++
	f.b |= uint32(c) << f.nb
	f.nb += 8
	return nil
}

// Read the next Huffman-encoded symbol from f according to h.
func (f *Decoder) huffSym(h *huffmanDecoder) (int, error) {
	// Since a huffmanDecoder can be empty or be composed of a degenerate tree
	// with single element, huffSym must error on these two edge cases. In both
	// cases, the chunks slice will be 0 for the invalid sequence, leading it
	// satisfy the n == 0 check below.
	n := uint(h.min)
	// Optimization. Compiler isn't smart enough to keep f.b,f.nb in registers,
	// but is smart enough to keep local variables in registers, so use nb and b,
	// inline call to moreBits and reassign b,nb back to f on return.
	nb, b := f.nb, f.b
	for {
		for nb < n {
			c, err := f.readByte()
			if err != nil {
				f.b = b
				f.nb = nb
				return 0, noEOF(err)
			}
			f.roffset++
			b |= uint32(c) << (nb & 31)
			nb += 8
		}
		chunk := h.chunks[b&(huffmanNumChunks-1)]
		n = uint(chunk & huffmanCountMask)
		if n > huffmanChunkBits {
			chunk = h.links[chunk>>huffmanValueShift<<h.linkBits+(b>>huffmanChunkBits)&h.linkMask]
			n = uint(chunk & huffmanCountMask)
		}
		if n <= nb {
			if n == 0 {
				f.b = b
				f.nb = nb
				f.err = flate.CorruptInputError(f.roffset)
				return 0, f.err
			}
			f.b = b >> (n & 31)
			f.nb = nb - n
			return int(chunk >> huffmanValueShift), nil
		}
	}
}

// tableSym is huffSym on fast table t, with h.min = min: the same
// reads, state and errors (design.md 4.1). It returns the entry.
func (f *Decoder) tableSym(t []uint32, tableBits, min uint) (uint32, error) {
	n := min
	nb, b := f.nb, f.b
	for {
		for nb < n {
			c, err := f.readByte()
			if err != nil {
				f.b = b
				f.nb = nb
				return 0, noEOF(err)
			}
			f.roffset++
			b |= uint32(c) << (nb & 31)
			nb += 8
		}
		e := t[b&(1<<tableBits-1)]
		if e&linkFlag != 0 {
			e = t[e>>16+b>>tableBits&(1<<(e>>8&15)-1)]
		}
		n = uint(e >> 8 & 15)
		if n <= nb {
			if n == 0 { // missing from an incomplete tree
				f.b = b
				f.nb = nb
				f.err = flate.CorruptInputError(f.roffset)
				return 0, f.err
			}
			f.b = b >> (n & 31)
			f.nb = nb - n
			return e, nil
		}
	}
}

func fixedTablesInit() {
	fixedOnce.Do(func() {
		// These come from the RFC section 3.2.6.
		var bits [288]int
		for i := 0; i < 144; i++ {
			bits[i] = 8
		}
		for i := 144; i < 256; i++ {
			bits[i] = 9
		}
		for i := 256; i < 280; i++ {
			bits[i] = 7
		}
		for i := 280; i < 288; i++ {
			bits[i] = 8
		}
		var dbits [32]int
		for i := range dbits {
			dbits[i] = 5
		}
		buildTable(fixedLL[:], bits[:], llSyms[:], llBits, 7)
		buildTable(fixedDT[:], dbits[:], distSyms[:], distBits, 0)
	})
}

// Output, in place of stdlib's dictDecoder.

// histSize reports the total amount of historical data in the dictionary.
func (f *Decoder) histSize() int {
	return min(f.wpos-f.histBase, maxMatchOffset)
}

// availWrite reports free space; one-shot grows.
func (f *Decoder) availWrite() int {
	if f.oneShot {
		return 1
	}
	return len(f.out) - f.wpos
}

// writeByte writes c, growing one-shot output if full.
// It reports false at the limit.
func (f *Decoder) writeByte(c byte) bool {
	if f.wpos == len(f.out) && !f.grow() {
		return false
	}
	f.out[f.wpos] = c
	f.wpos++
	return true
}

// writeCopy copies the pending match as space allows.
// One-shot output grows only when full.
func (f *Decoder) writeCopy() {
	for f.copyLen > 0 {
		if f.wpos == len(f.out) {
			if !f.oneShot || !f.grow() {
				return
			}
		}
		dstPos := f.wpos
		srcPos := dstPos - f.copyDist
		endPos := min(dstPos+f.copyLen, len(f.out))
		for dstPos < endPos {
			dstPos += copy(f.out[dstPos:endPos], f.out[srcPos:dstPos])
		}
		f.copyLen -= endPos - f.wpos
		f.wpos = endPos
	}
}

// errLimit stops one-shot output at limit.
var errLimit = errors.New("inflate: limit")

// grow enlarges full one-shot output. At the limit it
// sets errLimit and reports false.
func (f *Decoder) grow() bool {
	if f.wpos == f.limit {
		f.err = errLimit
		return false
	}
	n := min(max(f.wpos-f.histBase, 2*len(f.src), 4096), f.limit-f.wpos)
	f.out = slices.Grow(f.out[:f.wpos], n)
	f.out = f.out[:min(cap(f.out), f.limit)]
	return true
}

// reset is stdlib's Reset minus input and output.
func (f *Decoder) reset() {
	fixedTablesInit()
	f.roffset, f.b, f.nb = 0, 0, 0
	f.step = (*Decoder).nextBlock
	f.stepState = 0
	f.final = false
	f.err = nil
	f.copyLen, f.copyDist = 0, 0
	f.ll, f.dt = nil, nil
	f.paused = false
	f.shifted = 0
	f.raLen = 0
}

// sumChunk caps output per fast loop call when fusing adler32, so
// the sum runs while the bytes are still in cache. Tests lower it.
var sumChunk = 32 << 10

// sumTo adds out[sumPos:wpos] to Adler.
func (f *Decoder) sumTo() {
	f.Adler = adler32.Update(f.Adler, f.out[f.sumPos:f.wpos])
	f.sumPos = f.wpos
}

// Decode appends the inflated raw DEFLATE stream src to dst. It
// also returns the number of bytes of src consumed, as stdlib's
// reader would have read them.
func (f *Decoder) Decode(dst, src []byte) ([]byte, int, error) {
	out, n, err := f.decode(dst, src, math.MaxInt)
	if err == io.EOF {
		err = nil
	}
	return out, n, err
}

// DecodeLimit is Decode stopped where io.LimitReader(n) over
// stdlib's reader stops: it never writes dst past len(dst)+n.
// err is io.EOF if the stream ended, nil if the limit stopped it.
func (f *Decoder) DecodeLimit(dst, src []byte, n int) ([]byte, int, error) {
	if n <= 0 {
		return dst, 0, nil
	}
	limit := len(dst) + min(n, math.MaxInt-len(dst))
	out, c, err := f.decode(dst[:len(dst):min(cap(dst), limit)], src, limit)
	if err == errLimit || len(out) == limit && n%maxMatchOffset == 0 {
		// Limit hit, or stdlib's window flush at n ended it.
		err = nil
	}
	if unsafe.SliceData(out) == unsafe.SliceData(dst) {
		out = dst[:len(out)] // keep the caller's cap for appends
	}
	return out, c, err
}

// DecodeLimitContext is DecodeLimit that stops with ctx.Err() at
// the first block boundary after ctx is done.
func (f *Decoder) DecodeLimitContext(ctx context.Context, dst, src []byte, n int) ([]byte, int, error) {
	f.ctx = ctx
	out, c, err := f.DecodeLimit(dst, src, n)
	f.ctx = nil
	return out, c, err
}

// decode runs one-shot until an error, io.EOF or errLimit.
func (f *Decoder) decode(dst, src []byte, limit int) ([]byte, int, error) {
	if overlapsSpare(dst, src) {
		dst = dst[:len(dst):len(dst)] // output must not clobber src
	}
	f.reset()
	f.src, f.r, f.br, f.ra = src, nil, nil, nil
	f.oneShot = true
	f.limit = limit
	f.out = dst[:cap(dst)]
	f.wpos, f.histBase = len(dst), len(dst)
	f.Adler, f.sumPos = 1, f.wpos
	for f.err == nil {
		f.step(f)
	}
	if f.FuseAdler && f.err == io.EOF {
		f.sumTo()
	}
	out, n, err := f.out[:f.wpos], int(f.roffset), f.err
	f.src, f.out = nil, nil
	f.fs.in, f.fs.out = nil, nil
	return out, n, err
}

// overlapsSpare reports whether src shares dst[len(dst):cap(dst)].
func overlapsSpare(dst, src []byte) bool {
	if len(src) == 0 || len(dst) == cap(dst) {
		return false
	}
	lo := uintptr(unsafe.Pointer(unsafe.SliceData(dst))) + uintptr(len(dst))
	hi := lo + uintptr(cap(dst)-len(dst))
	s := uintptr(unsafe.Pointer(unsafe.SliceData(src)))
	return s < hi && lo < s+uintptr(len(src))
}

// fastRun runs the fast loop at a symbol boundary
// and restores stdlib's (b, nb, roffset).
func (f *Decoder) fastRun() {
	in := f.fastInput()
	if len(in) <= fastInMargin {
		return
	}
	s := &f.fs
	*s = fastState{
		in:       unsafe.Pointer(&in[0]),
		inLimit:  len(in) - fastInMargin,
		out:      unsafe.Pointer(&f.out[0]),
		outPos:   f.wpos,
		outLimit: len(f.out) - fastOutMargin,
		histBase: f.histBase,
		bitbuf:   uint64(f.b),
		bitsleft: f.nb,
		litlen:   f.ll,
		dist:     f.dt,
		pair:     f.pr,
	}
	if f.FuseAdler {
		s.outLimit = min(s.outLimit, f.wpos+sumChunk)
	}
	fastLoop(s)
	// Handoff, design.md 4.1; 0 is pulledAtEntry.
	pos := s.inPos*8 - int(s.bitsleft)
	pulled := max(0, (pos+int(s.slack)+7)>>3)
	nb := uint(pulled*8 - pos)
	f.b = uint32(s.bitbuf & (1<<nb - 1))
	f.nb = nb
	f.roffset += int64(pulled)
	f.wpos = s.outPos
	f.fastConsume(pulled)
	if f.FuseAdler {
		f.sumTo() // only bytes below wpos are final
	}
}
