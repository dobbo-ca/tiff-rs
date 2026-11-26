package codec

// HuffTable is a canonical Huffman table decoded from a DHT segment: Bits
// are the counts of codes of each length 1..16, and Values[:N] lists the
// symbols in code order (turbo jdmarker.c get_dht). Code and Length give
// each value's canonical code, parallel to Values; later tasks build the
// decode and encode lookup tables from these.
type HuffTable struct {
	Bits   [16]byte
	N      int
	Values [256]byte
	Code   [256]uint16
	Length [256]byte
}

// buildHuffTable assigns canonical Huffman codes to values in bit-length
// order, shortest and lowest-valued code first (JPEG Annex C).
func buildHuffTable(bits [16]byte, values []byte) HuffTable {
	var t HuffTable
	t.Bits = bits
	t.N = len(values)
	copy(t.Values[:], values)

	code, k := uint16(0), 0
	for length := 1; length <= 16; length++ {
		for i := byte(0); i < bits[length-1]; i++ {
			t.Code[k] = code
			t.Length[k] = byte(length)
			code++
			k++
		}
		code <<= 1
	}
	return t
}

// Standard tables, JPEG Annex K.3 (turbo jstdhuff.c): index 0
// luminance, 1 chrominance.
var stdDC, stdAC = [2]HuffTable{
	buildHuffTable([16]byte{0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0},
		[]byte{0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11}),
	buildHuffTable([16]byte{0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0},
		[]byte{0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11}),
}, [2]HuffTable{
	buildHuffTable([16]byte{0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d}, []byte{
		0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12,
		0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07,
		0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08,
		0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0,
		0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16,
		0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28,
		0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39,
		0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
		0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59,
		0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
		0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79,
		0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
		0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98,
		0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7,
		0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6,
		0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5,
		0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4,
		0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2,
		0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea,
		0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
		0xf9, 0xfa,
	}),
	buildHuffTable([16]byte{0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77}, []byte{
		0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21,
		0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71,
		0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91,
		0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0,
		0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34,
		0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26,
		0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38,
		0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48,
		0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58,
		0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
		0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78,
		0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
		0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96,
		0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5,
		0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4,
		0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3,
		0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2,
		0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda,
		0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9,
		0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
		0xf9, 0xfa,
	}),
}

// installStdHuffTables fills undefined tables 0 and 1 with the standard
// ones, as turbo jinit_huff_decoder does after the first SOS (for
// Motion-JPEG streams that omit DHT).
func (f *Frame) installStdHuffTables() {
	for i := 0; i < 2; i++ {
		if !f.HasDC[i] {
			f.DC[i], f.HasDC[i] = stdDC[i], true
		}
		if !f.HasAC[i] {
			f.AC[i], f.HasAC[i] = stdAC[i], true
		}
	}
}

// lookAheadBits is the bit-reader's lookahead width, matching turbo's
// HUFF_LOOKAHEAD (jdhuff.h:25): the fast path's refill point, and so where
// it meets FF FF, depends on this exactly, not just on speed.
const lookAheadBits = 8

// derivedTable is jpeg_make_d_derived_tbl's decode-side lookup: a
// lookAheadBits lookup for short codes plus maxcode/valoffset for the slow
// path (turbo jdhuff.c jpeg_make_d_derived_tbl, jdhuff.h HUFF_DECODE).
type derivedTable struct {
	// lookup[b] packs (length<<lookAheadBits)|symbol for a code found
	// within lookAheadBits bits; unfilled entries stay 0 (no such length),
	// which the slow path below treats as "longer than lookAheadBits".
	lookup [1 << lookAheadBits]uint16
	// combined[b] packs a code-plus-value hit, 0 a miss.
	combined  [1 << combBits]uint32
	maxcode   [18]int32
	valoffset [18]int32
	values    []byte
}

// BandTableExtra is band-table growth past 8 bits.
const BandTableExtra = 8 * 4 * (1<<combBits - 1<<lookAheadBits)

// build fills a zeroed d in place.
func (d *derivedTable) build(t *HuffTable, isDC bool) error {
	d.values = t.Values[:t.N]

	code, p := int32(0), 0
	for length := 1; length <= 16; length++ {
		n := int32(t.Bits[length-1])
		if n > 0 {
			d.valoffset[length] = int32(p) - code
			code += n
			p += int(n)
			d.maxcode[length] = code - 1
		} else {
			d.maxcode[length] = -1
		}
		// A code must still fit in length bits: no code is all ones
		// (turbo jdhuff.c jpeg_make_d_derived_tbl's ERREXIT check).
		if code >= 1<<uint(length) {
			return errHuffTable
		}
		code <<= 1
	}
	d.maxcode[17] = 1 << 30 // sentinel so the slow loop always terminates
	if isDC {
		for _, sym := range d.values {
			if sym > 15 { // turbo: DC magnitudes fit 15 bits
				return errHuffTable
			}
		}
	}

	idx := 0
	for length := 1; length <= 16; length++ {
		n := int(t.Bits[length-1])
		for i := 0; i < n; i++ {
			sym := t.Values[idx+i]
			if length <= lookAheadBits {
				base := int(t.Code[idx+i]) << (lookAheadBits - length)
				entry := uint16(length)<<lookAheadBits | uint16(sym)
				for f := 0; f < 1<<(lookAheadBits-length); f++ {
					d.lookup[base+f] = entry
				}
			}
			run, size := 0, int(sym)
			if !isDC {
				run, size = int(sym>>4), int(sym&15)
			}
			if noCombined || length+size > combBits || size == 0 && !combZero {
				continue
			}
			base := int(t.Code[idx+i]) << (combBits - length)
			if size == 0 {
				// Bit 6: no value, no refill.
				ce := uint32(length) | 64 | uint32(length)<<8 | uint32(run)<<12
				for p := 0; p < 1<<(combBits-length); p++ {
					d.combined[base+p] = ce
				}
				continue
			}
			pad := combBits - length - size
			for v := 0; v < 1<<size; v++ {
				val := huffExtend(int32(v), int32(size))
				// Low 6 bits shift as is.
				ce := uint32(length+size) | uint32(length)<<8 | uint32(run)<<12 | uint32(uint16(val))<<16
				for p := 0; p < 1<<pad; p++ {
					d.combined[base+v<<pad+p] = ce
				}
			}
		}
		idx += n
	}
	return nil
}

// huffExtend converts an s-bit magnitude-and-sign value to a signed int
// (JPEG Annex F.2.2.1, turbo jdhuff.c HUFF_EXTEND). s is always >= 1.
// Branch-free: the sign would mispredict.
func huffExtend(v, s int32) int32 {
	neg := (v - 1<<((s-1)&31)) >> 31
	return v + neg&(-1<<(s&31)+1)
}

// minGetBits is turbo's MIN_GET_BITS for a 64-bit get_buffer
// (jdhuff.c:282, BIT_BUF_SIZE-7).
const minGetBits = 57

// bitReader is the Huffman entropy bit reader: a 64-bit buffer with marker
// detection and byte unstuffing (turbo jdhuff.c jpeg_fill_bit_buffer).
type bitReader struct {
	data []byte
	pos  int

	buf      uint64
	bitsLeft int

	// marker mirrors cinfo->unread_marker != 0: once set, no more real
	// bytes are read and further fills zero-pad (jdhuff.c:297-380).
	marker       bool
	markerByte   byte
	insufficient bool // cinfo->entropy->insufficient_data
	lost         bool // insufficient, a resync or a bad code ever seen; sticky
	eof          bool // ran off src; turbo fakes EOI and warns

	decoded, scanned int // bytes read by fill and nextMarker
}

// virtualByte returns the byte at pos: real if pos < len(r.data), else
// turbo's fake EOI repeating past it (jdatasrc.c fill_mem_input_buffer,
// same formula as marker.go's fakeEOI). A scan whose SOS header ran off
// the end starts mid-cycle: a leftover fake byte from the header read
// (odd pos-len(r.data)) is real entropy-stream content to this reader,
// not yet the marker.
func (r *bitReader) virtualByte(pos int) byte {
	if pos < len(r.data) {
		return r.data[pos]
	}
	if (pos-len(r.data))%2 == 0 {
		return 0xff
	}
	return markerEOI
}

// nextLogicalByte returns the next entropy-stream byte, unstuffing FF 00
// and stopping (marker set) on a real marker or fake EOI (turbo
// jdatasrc.c fill_mem_input_buffer fakes an EOI at end of buffer).
func (r *bitReader) nextLogicalByte() (byte, bool) {
	if r.pos >= len(r.data) {
		r.eof = true
	}
	c := r.virtualByte(r.pos)
	r.pos++
	if c != 0xff {
		return c, true
	}
	for {
		if r.pos >= len(r.data) {
			r.eof = true
		}
		c = r.virtualByte(r.pos)
		r.pos++
		if c != 0xff {
			break
		}
	}
	if c == 0 {
		return 0xff, true
	}
	r.marker, r.markerByte = true, c
	return 0, false
}

// fill loads up to minGetBits bits, or pads with zero bits once a marker
// is hit and nbits still exceeds what is buffered (turbo
// jpeg_fill_bit_buffer, including the insufficient_data warning path).
func (r *bitReader) fill(nbits int) {
	if !r.marker {
		p := r.pos
		for r.bitsLeft < minGetBits {
			c, ok := r.nextLogicalByte()
			if !ok {
				break
			}
			r.buf = r.buf<<8 | uint64(c)
			r.bitsLeft += 8
		}
		r.decoded += r.pos - p
	}
	if nbits > r.bitsLeft {
		r.buf <<= uint(minGetBits - r.bitsLeft)
		r.bitsLeft = minGetBits
		r.insufficient, r.lost = true, true
	}
}

func (r *bitReader) ensure(nbits int) {
	if r.bitsLeft < nbits {
		r.fill(nbits)
	}
}

func (r *bitReader) getBits(n int) int32 {
	r.bitsLeft -= n
	return int32(r.buf>>uint(r.bitsLeft)) & (1<<uint(n) - 1)
}

func (r *bitReader) peekBits(n int) int32 {
	return int32(r.buf>>uint(r.bitsLeft-n)) & (1<<uint(n) - 1)
}

// decodeHuff returns the next Huffman symbol (turbo HUFF_DECODE macro).
func (r *bitReader) decodeHuff(d *derivedTable) byte {
	if r.bitsLeft < lookAheadBits {
		r.fill(0)
		if r.bitsLeft < lookAheadBits {
			return r.slowHuff(d, 1)
		}
	}
	entry := d.lookup[r.peekBits(lookAheadBits)]
	nb := int(entry >> lookAheadBits)
	if nb >= 1 && nb <= lookAheadBits {
		r.bitsLeft -= nb
		return byte(entry & (1<<lookAheadBits - 1))
	}
	return r.slowHuff(d, lookAheadBits+1)
}

// slowHuff is turbo's jpeg_huff_decode: fetch minBits, then extend one bit
// at a time until the code matches a known length.
func (r *bitReader) slowHuff(d *derivedTable, minBits int) byte {
	r.ensure(minBits)
	l := minBits
	code := r.getBits(l)
	for code > d.maxcode[l] {
		r.ensure(1)
		code = code<<1 | r.getBits(1)
		l++
	}
	if l > 16 {
		r.lost = true // JWRN_HUFF_BAD_CODE; turbo's fast path skips it
		return 0      // turbo: fake a zero as the safest result
	}
	return d.values[code+d.valoffset[l]]
}
