package inflate

// The fast loop decodes the common symbols of a Huffman block
// between careful-path symbols. This is the Go version; assembly
// versions must implement the same contract, and plug in through
// the fastLoop variable.
//
// fastState layout (offsets are for 64-bit targets and fixed):
//
//	off  field     dir     meaning
//	  0  in        in      input window base
//	  8  inPos     in/out  next input byte to load into bitbuf
//	 16  inLimit   in      see Margins
//	 24  out       in      output buffer base
//	 32  outPos    in/out  next output byte
//	 40  outLimit  in      start a symbol only if outPos <= outLimit
//	 48  histBase  in      matches may not reach below this index
//	 56  bitbuf    in/out  bit 0 is the next unread input bit
//	 64  bitsleft  in/out  valid bits in bitbuf, < 64; bits above are zero or input
//	 72  slack     in/out  max(0, s + max(minLL, n) - pos) for the last litlen symbol
//	 80  litlen    in      *litlenTable (tables.go)
//	 88  dist      in      *distTable
//	 96  pair      in      *pairTable or nil; only pair loops read it
//
// Bit positions count from in[0]: pos = inPos*8 - bitsleft, negative
// at entry when bitbuf holds bits from before in[0]. s is the last
// litlen symbol's start and n its code length; slack is what stdlib
// read past pos for it (tables.go). The caller sets slack = 0 and
// converts back to stdlib's state with pulled = ceil8(pos + slack)
// (design.md 4.1).
//
// Stop rules. Return before a symbol, with no side effects for it,
// when outPos > outLimit at its start, or inPos > inLimit at the last
// check, or its litlen entry has stopFlag, or its distance entry has
// stopFlag, or its distance is more than outPos - histBase.
//
// Margins. inLimit = len(in) - fastInMargin; after a passing check
// of inPos <= inLimit the loop refills (8-byte load, advance <= 7) at
// most three times before checking again. outLimit = len(out) -
// fastOutMargin, so one match may write 258 bytes plus over-copy.
// Each refill leaves >= 56 bits: three literals (3*15), or one
// match (15+5+15+13), never needs more.
//
// Preemption. Assembly is not async-preemptible, so asm loops bound
// each call (1 MiB of output) and may stop earlier than the Go loop.
// The careful path then decodes one symbol and calls again.

import (
	"encoding/binary"
	"unsafe"
)

const (
	fastInMargin  = 24
	fastOutMargin = 320
)

type fastState struct {
	in       unsafe.Pointer
	inPos    int
	inLimit  int
	out      unsafe.Pointer
	outPos   int
	outLimit int
	histBase int
	bitbuf   uint64
	bitsleft uint
	slack    uint
	litlen   *litlenTable
	dist     *distTable
	pair     *pairTable
}

// fastLoop is where assembly loops plug in.
var fastLoop = fastLoopGo

// pairLoop: fastLoop runs a pair loop when s.pair != nil.
var pairLoop bool

func load64(p unsafe.Pointer, i int) uint64 {
	return binary.LittleEndian.Uint64((*[8]byte)(unsafe.Add(p, i))[:])
}

func store64(p unsafe.Pointer, i int, v uint64) {
	binary.LittleEndian.PutUint64((*[8]byte)(unsafe.Add(p, i))[:], v)
}

// copy8 copies 8 bytes, any byte order.
func copy8(dst, src unsafe.Pointer, i uint) {
	*(*[8]byte)(unsafe.Add(dst, i)) = *(*[8]byte)(unsafe.Add(src, i))
}

func fastLoopGo(s *fastState) {
	// Pointers, not indexes: fewer live registers.
	in := unsafe.Add(s.in, s.inPos)
	out := unsafe.Add(s.out, s.outPos)
	inLimit := uintptr(s.in) + uintptr(s.inLimit)
	outLimit := uintptr(s.out) + uintptr(s.outLimit)
	hist := uintptr(s.out) + uintptr(s.histBase)
	bitbuf, bitsleft := s.bitbuf, s.bitsleft // only bitsleft&63 matters
	slack := s.slack
	ll, dt := s.litlen, s.dist

	for uintptr(in) <= inLimit && uintptr(out) <= outLimit {
		bitbuf |= load64(in, 0) << (bitsleft & 63)
		in = unsafe.Add(in, (^bitsleft&63)>>3)
		bitsleft |= 56
		e := ll[bitbuf&llMask]
		if e&linkFlag != 0 {
			e = ll[e>>16+uint32(bitbuf>>llBits)&(1<<(e>>8&15)-1)]
		}

	decode: // e: next entry, bits refilled
		if e&litFlag != 0 {
			slack = uint(e >> 24)
			bitbuf >>= e & 63
			bitsleft -= uint(e)
			*(*byte)(out) = byte(e >> 16)
			out = unsafe.Add(out, 1)
			if uintptr(out) > outLimit {
				break
			}
			e = ll[bitbuf&llMask]
			if e&linkFlag != 0 {
				e = ll[e>>16+uint32(bitbuf>>llBits)&(1<<(e>>8&15)-1)]
			}
			if e&litFlag != 0 {
				slack = uint(e >> 24)
				bitbuf >>= e & 63
				bitsleft -= uint(e)
				*(*byte)(out) = byte(e >> 16)
				out = unsafe.Add(out, 1)
				if uintptr(out) > outLimit {
					break
				}
				e = ll[bitbuf&llMask]
				if e&linkFlag != 0 {
					e = ll[e>>16+uint32(bitbuf>>llBits)&(1<<(e>>8&15)-1)]
				}
				if e&litFlag != 0 {
					slack = uint(e >> 24)
					bitbuf >>= e & 63
					bitsleft -= uint(e)
					*(*byte)(out) = byte(e >> 16)
					out = unsafe.Add(out, 1)
					continue
				}
			}
			// Refill for the match; e stays valid.
			bitbuf |= load64(in, 0) << (bitsleft & 63)
			in = unsafe.Add(in, (^bitsleft&63)>>3)
			bitsleft |= 56
		}
		if e&stopFlag != 0 {
			break
		}

		// Commit only once distance is valid.
		length := uint(e>>16) + uint(bitbuf&(uint64(1)<<(e&63)-1)>>(e>>8&15))
		bb := bitbuf >> (e & 63)
		d := dt[bb&distMask]
		if d&linkFlag != 0 {
			d = dt[d>>16+uint32(bb>>distBits)&(1<<(d>>8&15)-1)]
		}
		if d&stopFlag != 0 {
			break
		}
		dist := uint(d>>16) + uint(bb&(uint64(1)<<(d&63)-1)>>(d>>8&15))
		if uintptr(dist) > uintptr(out)-hist {
			break
		}
		bitbuf = bb >> (d & 63)
		used := uint(e&31 + d&31)
		bitsleft -= used
		slack = uint(max(0, int(e>>12&15)-int(used)))

		// Preload: overlap the lookup with copying.
		bitbuf |= load64(in, 0) << (bitsleft & 63)
		in = unsafe.Add(in, (^bitsleft&63)>>3)
		bitsleft |= 56
		e = ll[bitbuf&llMask]
		if e&linkFlag != 0 {
			e = ll[e>>16+uint32(bitbuf>>llBits)&(1<<(e>>8&15)-1)]
		}

		src := unsafe.Add(out, -int(dist))
		switch {
		case dist >= 8:
			// Most matches fit: skip loop branch.
			copy8(out, src, 0)
			copy8(out, src, 8)
			copy8(out, src, 16)
			copy8(out, src, 24)
			for i := uint(32); i < length; i += 8 {
				copy8(out, src, i)
			}
		case dist == 1:
			v := uint64(*(*byte)(src)) * 0x0101010101010101
			store64(out, 0, v)
			store64(out, 8, v)
			store64(out, 16, v)
			store64(out, 24, v)
			for i := uint(32); i < length; i += 8 {
				store64(out, int(i), v)
			}
		default:
			// Each copy fixes dist more bytes.
			for i := uint(0); i < length; i += dist {
				copy8(out, src, i)
			}
		}
		out = unsafe.Add(out, length)
		if uintptr(in) <= inLimit && uintptr(out) <= outLimit {
			goto decode
		}
		break
	}

	s.inPos = int(uintptr(in) - uintptr(s.in))
	s.outPos = int(uintptr(out) - uintptr(s.out))
	s.bitbuf, s.bitsleft = bitbuf, bitsleft&63
	s.slack = slack
}
