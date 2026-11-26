package main

import (
	"encoding/binary"
	"fmt"
)

// ifd0 locates the first IFD of a TIFF: the byte order, the offset of each entry, and the offset
// of the field that holds the next IFD's offset.
func ifd0(b []byte) (bo binary.ByteOrder, big bool, entries []int, next int, ok bool) {
	if len(b) < 8 {
		return
	}
	switch string(b[:2]) {
	case "II":
		bo = binary.LittleEndian
	case "MM":
		bo = binary.BigEndian
	default:
		return
	}
	switch bo.Uint16(b[2:]) {
	case 42:
		off := int(bo.Uint32(b[4:]))
		if off < 0 || off+2 > len(b) {
			return
		}
		n := int(bo.Uint16(b[off:]))
		for i := 0; i < n && off+2+12*(i+1) <= len(b); i++ {
			entries = append(entries, off+2+12*i)
		}
		next = off + 2 + 12*n
	case 43:
		if len(b) < 16 {
			return
		}
		off64 := bo.Uint64(b[8:])
		if off64 > uint64(len(b)) || int(off64)+8 > len(b) {
			return
		}
		off := int(off64)
		n64 := bo.Uint64(b[off:])
		if n64 > 4096 {
			return
		}
		n := int(n64)
		for i := 0; i < n && off+8+20*(i+1) <= len(b); i++ {
			entries = append(entries, off+8+20*i)
		}
		next = off + 8 + 20*n
		big = true
	default:
		return
	}
	return bo, big, entries, next, next+4 <= len(b)
}

func put(b []byte, bo binary.ByteOrder, off, width int, v uint64) {
	if off+width > len(b) {
		return
	}
	switch width {
	case 2:
		bo.PutUint16(b[off:], uint16(v))
	case 4:
		bo.PutUint32(b[off:], uint32(v))
	case 8:
		bo.PutUint64(b[off:], v)
	}
}

func get(b []byte, bo binary.ByteOrder, off, width int) uint64 {
	if off+width > len(b) {
		return 0
	}
	switch width {
	case 2:
		return uint64(bo.Uint16(b[off:]))
	case 4:
		return uint64(bo.Uint32(b[off:]))
	}
	return bo.Uint64(b[off:])
}

// mutate writes the mutations of one base input: truncations (every cut of a file under 300
// bytes, else 24), bit flips (24 anywhere, 8 in the first 64 bytes), byte sets (12), edits of the
// type, count and value field of the first IFD's entries (a deterministic sample of 60) and of
// its next-IFD offset.
func (g *gen) mutate(name string, b []byte) {
	r := seedOf(name)
	if len(b) <= 300 {
		for c := 0; c < len(b); c++ {
			g.emit(fmt.Sprintf("%s~t%d", name, c), b[:c])
		}
	} else {
		for i := 0; i < 24; i++ {
			c := r.intn(len(b))
			g.emit(fmt.Sprintf("%s~t%d", name, c), b[:c])
		}
	}
	for i := 0; i < 32; i++ {
		span := len(b)
		if i >= 24 {
			span = min(len(b), 64)
		}
		bit := r.intn(span * 8)
		c := append([]byte(nil), b...)
		c[bit/8] ^= 1 << (bit % 8)
		g.emit(fmt.Sprintf("%s~f%d", name, bit), c)
	}
	for i := 0; i < 12; i++ {
		pos := r.intn(len(b))
		v := []byte{0, 0xff, byte(r.next())}[i%3]
		c := append([]byte(nil), b...)
		c[pos] = v
		g.emit(fmt.Sprintf("%s~s%dx%d", name, pos, v), c)
	}
	bo, big, es, next, ok := ifd0(b)
	if !ok {
		return
	}
	cw, vw, vo := 4, 4, 8
	if big {
		cw, vw, vo = 8, 8, 12
	}
	type edit struct {
		off, width int
		v          uint64
	}
	var all []edit
	for _, e := range es {
		all = append(all, edit{e + 2, 2, 0}, edit{e + 2, 2, 3}, edit{e + 2, 2, 4}, edit{e + 2, 2, 16},
			edit{e + 4, cw, 0}, edit{e + 4, cw, 2}, edit{e + 4, cw, 0xffffffff},
			edit{e + vo, vw, 0}, edit{e + vo, vw, 1}, edit{e + vo, vw, 0xffffffff}, edit{e + vo, vw, uint64(len(b))},
			edit{e + vo, vw, get(b, bo, e+vo, vw) + 1})
		if big {
			all = append(all, edit{e + 4, 8, 1 << 63}, edit{e + vo, 8, 1<<64 - 1})
		}
	}
	for i := 0; i < 60 && len(all) > 0; i++ {
		k := r.intn(len(all))
		e := all[k]
		all = append(all[:k], all[k+1:]...)
		c := append([]byte(nil), b...)
		put(c, bo, e.off, e.width, e.v)
		g.emit(fmt.Sprintf("%s~e%d_%d_%x", name, e.off, e.width, e.v), c)
	}
	nw := 4
	if big {
		nw = 8
	}
	first, self := get(b, bo, 4, 4), uint64(next-2-12*len(es))
	if big {
		first, self = get(b, bo, 8, 8), uint64(next-8-20*len(es))
	}
	for i, v := range []uint64{0, first, self, uint64(len(b)), 0xffffffff} {
		c := append([]byte(nil), b...)
		put(c, bo, next, nw, v)
		g.emit(fmt.Sprintf("%s~n%d", name, i), c)
	}
}
