package encref

import "bytes"

// zigzagNatural[i] is the natural-order index of zigzag position i
// (jpeg_natural_order, jutils.c:59-70).
var zigzagNatural = [64]int{
	0, 1, 8, 16, 9, 2, 3, 10,
	17, 24, 32, 25, 18, 11, 4, 5,
	12, 19, 26, 33, 40, 48, 41, 34,
	27, 20, 13, 6, 7, 14, 21, 28,
	35, 42, 49, 56, 57, 50, 43, 36,
	29, 22, 15, 23, 30, 37, 44, 51,
	58, 59, 52, 45, 38, 31, 39, 46,
	53, 60, 61, 54, 47, 55, 62, 63,
}

// HeaderParams describes one baseline header cjpeg's compressor writes:
// JFIF 1.01, density 0/1/1, force_baseline, standard Huffman, no restarts.
type HeaderParams struct {
	Width, Height int
	Quality       int
	Gray          bool
	Chroma444     bool // ignored when Gray
}

// sofComponent is one SOF0/SOS component entry.
type sofComponent struct {
	id, hSamp, vSamp, quantTbl, dcTbl, acTbl byte
}

// components follows jcparam.c:379-381: 4:2:0 by default, or all 1x1 for
// 4:4:4 and gray.
func (p HeaderParams) components() []sofComponent {
	if p.Gray {
		return []sofComponent{{id: 1, hSamp: 1, vSamp: 1}}
	}
	yh, yv := byte(2), byte(2)
	if p.Chroma444 {
		yh, yv = 1, 1
	}
	return []sofComponent{
		{id: 1, hSamp: yh, vSamp: yv, quantTbl: 0, dcTbl: 0, acTbl: 0},
		{id: 2, hSamp: 1, vSamp: 1, quantTbl: 1, dcTbl: 1, acTbl: 1},
		{id: 3, hSamp: 1, vSamp: 1, quantTbl: 1, dcTbl: 1, acTbl: 1},
	}
}

func put16(buf *bytes.Buffer, v int) {
	buf.WriteByte(byte(v >> 8))
	buf.WriteByte(byte(v))
}

// writeDQT emits one DQT marker, zigzag order (emit_dqt, jcmarker.c).
func writeDQT(buf *bytes.Buffer, index int, table [64]byte) {
	buf.Write([]byte{0xff, 0xdb})
	put16(buf, 2+1+64)
	buf.WriteByte(byte(index))
	for _, n := range zigzagNatural {
		buf.WriteByte(table[n])
	}
}

// writeDHT emits one DHT marker (emit_dht, jcmarker.c).
func writeDHT(buf *bytes.Buffer, index int, isAC bool, t EncHuffTable) {
	buf.Write([]byte{0xff, 0xc4})
	length := 0
	for i := 1; i <= 16; i++ {
		length += int(t.Bits[i])
	}
	put16(buf, 2+1+16+length)
	id := byte(index)
	if isAC {
		id += 0x10
	}
	buf.WriteByte(id)
	for i := 1; i <= 16; i++ {
		buf.WriteByte(t.Bits[i])
	}
	buf.Write(t.Vals[:length])
}

// WriteHeader writes SOI, APP0 JFIF, DQT, SOF0, DHT and SOS in cjpeg's
// order. No restarts, no optimized tables.
func WriteHeader(buf *bytes.Buffer, p HeaderParams) {
	buf.Write([]byte{0xff, 0xd8}) // SOI

	buf.Write([]byte{0xff, 0xe0}) // APP0 JFIF 1.01, density 0/1/1, no thumbnail
	put16(buf, 16)
	buf.WriteString("JFIF\x00")
	buf.Write([]byte{0x01, 0x01, 0x00})
	put16(buf, 1)
	put16(buf, 1)
	buf.Write([]byte{0x00, 0x00})

	writeDQT(buf, 0, LumaQuantTable(p.Quality))
	if !p.Gray {
		writeDQT(buf, 1, ChromaQuantTable(p.Quality))
	}

	comps := p.components()
	buf.Write([]byte{0xff, 0xc0}) // SOF0
	put16(buf, 2+6+3*len(comps))
	buf.WriteByte(8) // data precision
	put16(buf, p.Height)
	put16(buf, p.Width)
	buf.WriteByte(byte(len(comps)))
	for _, c := range comps {
		buf.WriteByte(c.id)
		buf.WriteByte(c.hSamp<<4 | c.vSamp)
		buf.WriteByte(c.quantTbl)
	}

	writeDHT(buf, 0, false, StdDCLuma)
	writeDHT(buf, 0, true, StdACLuma)
	if !p.Gray {
		writeDHT(buf, 1, false, StdDCChroma)
		writeDHT(buf, 1, true, StdACChroma)
	}

	buf.Write([]byte{0xff, 0xda}) // SOS
	put16(buf, 2+1+2*len(comps)+3)
	buf.WriteByte(byte(len(comps)))
	for _, c := range comps {
		buf.WriteByte(c.id)
		buf.WriteByte(c.dcTbl<<4 | c.acTbl)
	}
	buf.Write([]byte{0x00, 0x3f, 0x00}) // Ss Se AhAl
}
