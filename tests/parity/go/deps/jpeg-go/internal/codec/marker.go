// Package codec holds the pure-Go JPEG decode internals shared by the
// root jpeg package.
package codec

import (
	"errors"

	"github.com/dobbo-ca/jpeg-go/internal/ledger"
)

// Sentinel errors, re-exported by the root package.
var (
	ErrUnsupported = errors.New("jpeg: unsupported stream or option")
	ErrTruncated   = errors.New("jpeg: truncated entropy data")
	ErrAfterScan   = errors.New("jpeg: fatal marker after the last scan")
)

// afterScan is a fatal marker past a single-scan frame's last row: turbo
// ERREXITs in jpeg_finish_decompress with every row already out.
type afterScan struct{ err error }

func (e afterScan) Error() string   { return ErrAfterScan.Error() + ": " + e.err.Error() }
func (e afterScan) Unwrap() []error { return []error{ErrAfterScan, e.err} }

const (
	markerSOI   = 0xd8
	markerEOI   = 0xd9
	markerSOS   = 0xda
	markerDQT   = 0xdb
	markerDHT   = 0xc4
	markerDRI   = 0xdd
	markerAPP0  = 0xe0
	markerAPP14 = 0xee
)

// zigzagToNatural maps a DQT/coefficient zigzag index to its natural
// (raster) position, per turbo jdmarker.c:537-545.
var zigzagToNatural = [64]int{
	0, 1, 8, 16, 9, 2, 3, 10,
	17, 24, 32, 25, 18, 11, 4, 5,
	12, 19, 26, 33, 40, 48, 41, 34,
	27, 20, 13, 6, 7, 14, 21, 28,
	35, 42, 49, 56, 57, 50, 43, 36,
	29, 22, 15, 23, 30, 37, 44, 51,
	58, 59, 52, 45, 38, 31, 39, 46,
	53, 60, 61, 54, 47, 55, 62, 63,
}

// Component is one SOF component descriptor.
type Component struct {
	ID   byte
	H, V byte
	Tq   byte
}

// Frame holds marker-segment state up to SOS (or EOI for a tables-only
// stream). The zero value is ready; Parse allocates nothing.
type Frame struct {
	Width, Height   int
	NComp           int
	Comp            [3]Component
	RestartInterval int
	RGB             bool
	HasSOF          bool
	// Ledger, when set, is charged before every input-sized buffer the
	// decode makes (Plane storage, restart starts); nil is unbounded.
	Ledger *ledger.Ledger

	jfif           bool
	adobe          bool
	adobeTransform byte

	Quant    [4][64]uint16
	HasQuant [4]bool
	// These fill padding, keeping baseline offsets.
	Progressive                    bool
	Overran                        bool // turbo's fake EOI warns
	ScanSs, ScanSe, ScanAh, ScanAl byte

	// Scan is the current SOS: components in scan order (indices into
	// Comp) and each one's Huffman table selectors.
	ScanN      int
	ScanComp   [4]int
	ScanDC     [4]byte
	ScanAC     [4]byte
	ScanOffset int // src offset of the first entropy byte after SOS

	DC           [4]HuffTable
	AC           [4]HuffTable
	HasDC, HasAC [4]bool
}

// Preallocated so failures allocate nothing.
var (
	errNoSOI         = errors.New("jpeg: missing SOI")
	errDupSOI        = errors.New("jpeg: two SOI markers")
	errLength        = errors.New("jpeg: bad marker length")
	errUnknownMarker = errors.New("jpeg: unknown marker")
	errNoSOF         = errors.New("jpeg: no SOF marker")
	errNoSOS         = errors.New("jpeg: missing SOS marker")
	errSOSNoSOF      = errors.New("jpeg: SOS before SOF")
	errDupSOF        = errors.New("jpeg: multiple SOF markers")
	errEmpty         = errors.New("jpeg: empty image")
	errTooBig        = errors.New("jpeg: image dimension over 65500")
	errComponentID   = errors.New("jpeg: bad component id in SOS")
	errDQTIndex      = errors.New("jpeg: bad DQT index")
	errDHTIndex      = errors.New("jpeg: bad DHT index")
	errHuffTable     = errors.New("jpeg: bad DHT table")
	errTables        = errors.New("jpeg: tables stream has SOF or SOS")
	errDACIndex      = errors.New("jpeg: bad DAC value")
	errScanSelector  = errors.New("jpeg: bad Huffman table selector in SOS")
)

// Parse reads an optional abbreviated tables stream, then src, stopping at
// SOS. src must contain a SOF; tables (when non-nil) supplies only DQT and
// DHT segments.
func (f *Frame) Parse(src, tables []byte) error {
	if tables != nil {
		if err := f.ParseTables(tables); err != nil {
			return err
		}
	}
	return f.ParseFrame(src)
}

// ParseTables reads an abbreviated tables stream (DQT/DHT only) into f, for
// callers that cache the result across calls sharing the same tables (a
// Decoder skips this when the bytes match the last call's).
func (f *Frame) ParseTables(tables []byte) error {
	sos, _, err := f.parseSegments(tables, true)
	if err != nil {
		return err
	}
	if sos || f.HasSOF {
		return errTables
	}
	return nil
}

// ParseFrame reads src, stopping at SOS. src must contain a SOF; any DQT/DHT
// already in f (from ParseTables) apply unless src carries its own.
func (f *Frame) ParseFrame(src []byte) error {
	sos, offset, err := f.parseSegments(src, false)
	if err != nil {
		return err
	}
	f.ScanOffset = offset
	if !sos {
		if f.HasSOF {
			return errNoSOS
		}
		return errNoSOF
	}
	f.resolveColor()
	return nil
}

// parseSegments reads b's markers until SOS (sos true, offset is the first
// entropy byte after it) or EOI.
func (f *Frame) parseSegments(b []byte, tables bool) (sos bool, offset int, err error) {
	if len(b) < 2 || b[0] != 0xff || b[1] != markerSOI {
		return false, 0, errNoSOI
	}
	// turbo get_soi resets these per SOI; HasSOF too, so a Decoder reusing
	// one Frame across calls can see a fresh SOF each time.
	f.RestartInterval, f.jfif, f.adobe, f.adobeTransform, f.HasSOF = 0, false, false, 0, false
	f.Overran = false
	return f.consumeFrom(b, 2, tables)
}

// findMarker returns the next marker code in b at or after i and the index
// right after its 2-byte code, skipping stuffed 0xff padding like turbo
// next_marker; TEM and RSTn (no length) are skipped too. ok is false at EOF.
func findMarker(b []byte, i int) (marker byte, next int, ok bool) {
	for {
		m := byte(0)
		for m == 0 {
			for i < len(b) && b[i] != 0xff {
				i++
			}
			for i < len(b) && b[i] == 0xff {
				i++
			}
			if i >= len(b) {
				return 0, 0, false
			}
			m = b[i]
			i++
		}
		if m == 0x01 || (m >= 0xd0 && m <= 0xd7) {
			continue // TEM/RSTn carry no length
		}
		return m, i, true
	}
}

// consumeFrom finds and consumes markers from b starting at i until SOS
// (sos true, offset is the first entropy byte after it) or EOI.
func (f *Frame) consumeFrom(b []byte, i int, needSeg bool) (sos bool, offset int, err error) {
	marker, i, ok := findMarker(b, i)
	if !ok {
		return false, 0, nil // fake EOI
	}
	return f.consumeFromMarker(b, marker, i, needSeg)
}

// consumeFromMarker processes marker (already identified, code position i)
// then keeps consuming further markers, same as consumeFrom. It also serves
// DecodeScan's multi-scan continuation: jdhuff.c hits the following marker
// while topping up its bit buffer, past the marker's own code bytes, for a
// non-interleaved sequential frame's next scan (docs/design.md section 1).
func (f *Frame) consumeFromMarker(b []byte, marker byte, i int, needSeg bool) (sos bool, offset int, err error) {
	for {
		var done bool
		sos, offset, done, err = f.handleMarker(b, marker, i, needSeg)
		if done || err != nil {
			return sos, offset, err
		}
		var ok bool
		marker, i, ok = findMarker(b, offset)
		if !ok {
			f.Overran = true
			return false, 0, nil // fake EOI
		}
	}
}

// handleMarker parses the segment following an already-identified marker.
// done is true once the frame's header work for this marker is finished:
// SOS or EOI end the scan (or a fatal error), anything else just moves i
// past its segment so the caller can look for the next marker.
func (f *Frame) handleMarker(b []byte, marker byte, i int, needSeg bool) (sos bool, offset int, done bool, err error) {
	switch {
	case marker == markerEOI:
		return false, 0, true, nil
	case marker == markerSOI:
		return false, 0, true, errDupSOI
	case marker == 0x01 || marker >= 0xd0 && marker <= 0xd7:
		return false, i, false, nil // turbo: parameterless, skipped
	}
	var lb [2]byte
	fakeEOI(lb[:], b, i)
	length := int(lb[0])<<8 | int(lb[1])
	if length < 2 && (marker >= 0xe0 && marker <= 0xef || marker == 0xfe || marker == 0xdc) {
		length = 2 // turbo skip_variable ignores short lengths
	}
	if length < 2 {
		return false, 0, true, errLength
	}
	if i+length > len(b) {
		return f.overrun(b, marker, i, length, needSeg)
	}
	seg := b[i+2 : i+length]
	i += length
	if marker == markerSOS {
		err := f.readSOS(seg)
		return true, i, true, err
	}
	if err := f.readSegment(marker, seg); err != nil {
		return false, 0, true, err
	}
	return false, i, false, nil
}

// fakeEOI fills dst from b[i:], then with turbo's fake EOI bytes: past
// the data, jdatasrc-tj.c returns FF D9 on every refill.
func fakeEOI(dst, b []byte, i int) {
	for k := range dst {
		switch j := i + k; {
		case j < len(b):
			dst[k] = b[j]
		case (j-len(b))%2 == 0:
			dst[k] = 0xff
		default:
			dst[k] = markerEOI
		}
	}
}

// overrun reads a segment that runs past the data. Only fake EOI
// bytes follow, so the stream ends at EOI unless the segment is fatal.
// needSeg is false in a frame header, where that EOI is fatal anyway.
func (f *Frame) overrun(b []byte, marker byte, i, length int, needSeg bool) (sos bool, offset int, done bool, err error) {
	if marker != markerSOS {
		f.Overran = true
	}
	switch {
	case marker == markerSOS:
		var buf [12]byte // turbo get_sos: length is 6 + 2*Ns
		if length-2 > len(buf) {
			return false, 0, true, errLength
		}
		seg := buf[:length-2]
		fakeEOI(seg, b, i+2)
		// i+length, not len(b): the entropy scan's bit reader picks up
		// the fake-EOI cycle where this header read left it (huff.go
		// virtualByte), which may be mid-cycle, not at real EOF.
		return true, i + length, true, f.readSOS(seg)
	case marker == markerDRI:
		if length != 4 {
			return false, 0, true, errLength
		}
	case marker == markerDHT || marker == markerDQT || marker == 0xcc:
		if needSeg {
			seg := make([]byte, length-2) // hostile streams only
			fakeEOI(seg, b, i+2)
			if err := f.readSegment(marker, seg); err != nil {
				return false, 0, true, err
			}
		}
	case marker >= 0xe0 && marker <= 0xef || marker == 0xfe || marker == 0xdc:
	default:
		return false, 0, true, f.readSegment(marker, b[min(i+2, len(b)):])
	}
	return false, 0, true, nil
}

func (f *Frame) readSegment(marker byte, seg []byte) error {
	switch marker {
	case markerAPP0:
		if len(seg) >= 14 && string(seg[:5]) == "JFIF\x00" {
			f.jfif = true
		}
	case markerAPP14:
		if len(seg) >= 12 && string(seg[:5]) == "Adobe" {
			f.adobe = true
			f.adobeTransform = seg[11]
		}
	case markerDQT:
		return f.readDQT(seg)
	case markerDHT:
		return f.readDHT(seg)
	case markerDRI:
		if len(seg) != 2 {
			return errLength
		}
		f.RestartInterval = int(seg[0])<<8 | int(seg[1])
	case 0xc0, 0xc1:
		return f.readSOF(seg, false)
	case 0xc2:
		return f.readSOF(seg, true)
	case 0xc3, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xcb, 0xcd, 0xce, 0xcf:
		// Out-of-scope SOFn and JPG.
		return ErrUnsupported
	case 0xcc:
		return readDAC(seg)
	case 0xdc, 0xfe:
		// DNL, COM: skip.
	default:
		if marker < 0xe1 || marker > 0xef {
			return errUnknownMarker // turbo rejects DHP, EXP, JPGn, RESn
		}
	}
	return nil
}

// readDAC only validates, as turbo get_dac does.
func readDAC(seg []byte) error {
	if len(seg)%2 != 0 {
		return errLength
	}
	for ; len(seg) > 0; seg = seg[2:] {
		index, val := seg[0], seg[1]
		switch {
		case index >= 32:
			return errDACIndex
		case index < 16 && val&0x0f > val>>4:
			return errDACIndex
		}
	}
	return nil
}

func (f *Frame) readDQT(seg []byte) error {
	for len(seg) > 0 {
		pq := seg[0] >> 4
		tq := seg[0] & 0xf
		if tq > 3 {
			return errDQTIndex
		}
		seg = seg[1:]
		if pq == 0 {
			if len(seg) < 64 {
				return errLength
			}
			for i := 0; i < 64; i++ {
				f.Quant[tq][zigzagToNatural[i]] = uint16(seg[i])
			}
			seg = seg[64:]
		} else { // turbo: any nonzero Pq is 16-bit
			if len(seg) < 128 {
				return errLength
			}
			for i := 0; i < 64; i++ {
				f.Quant[tq][zigzagToNatural[i]] = uint16(seg[i*2])<<8 | uint16(seg[i*2+1])
			}
			seg = seg[128:]
		}
		f.HasQuant[tq] = true
	}
	return nil
}

func (f *Frame) readDHT(seg []byte) error {
	for len(seg) > 0 {
		if len(seg) < 17 {
			return errLength
		}
		index := seg[0]
		if index&0xef > 3 { // turbo jdmarker.c:488-492
			return errDHTIndex
		}
		var bits [16]byte
		copy(bits[:], seg[1:17])
		n := 0
		for _, c := range bits {
			n += int(c)
		}
		seg = seg[17:]
		if n > 256 || len(seg) < n { // turbo jdmarker.c:475
			return errHuffTable
		}
		tbl := buildHuffTable(bits, seg[:n])
		seg = seg[n:]
		if index&0x10 == 0 {
			f.DC[index], f.HasDC[index] = tbl, true
		} else {
			f.AC[index&3], f.HasAC[index&3] = tbl, true
		}
	}
	return nil
}

// readSOF follows turbo get_sof, plus initial_setup's size and sampling
// checks.
func (f *Frame) readSOF(seg []byte, progressive bool) error {
	if f.HasSOF {
		return errDupSOF
	}
	if len(seg) < 6 {
		return errLength
	}
	height := int(seg[1])<<8 | int(seg[2])
	width := int(seg[3])<<8 | int(seg[4])
	nc := int(seg[5])
	if height == 0 || width == 0 || nc == 0 {
		return errEmpty
	}
	if len(seg) != 6+nc*3 {
		return errLength
	}
	if height > 65500 || width > 65500 {
		return errTooBig
	}
	if seg[0] != 8 || (nc != 1 && nc != 3) {
		return ErrUnsupported // only 8-bit gray or 3 components
	}
	var comp [3]Component
	for i := 0; i < nc; i++ {
		off := 6 + i*3
		comp[i] = Component{ID: seg[off], H: seg[off+1] >> 4, V: seg[off+1] & 0xf, Tq: seg[off+2]}
	}
	if err := validateSampling(nc, comp); err != nil {
		return err
	}
	f.Width, f.Height, f.NComp, f.Comp = width, height, nc, comp
	f.HasSOF, f.Progressive = true, progressive
	return nil
}

// validateSampling accepts gray at any legal factor (1..4) and, for 3
// components, 4:4:4, 4:2:2, 4:2:0, 4:4:0, 4:1:1 and 4:1:0 (Y 1x2, 4x1, 4x2)
// with Cb/Cr unsubsampled.
func validateSampling(nc int, c [3]Component) error {
	if nc == 1 {
		if c[0].H < 1 || c[0].H > 4 || c[0].V < 1 || c[0].V > 4 {
			return ErrUnsupported
		}
		return nil
	}
	if c[1].H != 1 || c[1].V != 1 || c[2].H != 1 || c[2].V != 1 {
		return ErrUnsupported
	}
	switch {
	case c[0].H == 1 && c[0].V == 1, c[0].H == 2 && c[0].V == 1, c[0].H == 2 && c[0].V == 2,
		c[0].H == 1 && c[0].V == 2, c[0].H == 4 && c[0].V == 1, c[0].H == 4 && c[0].V == 2:
		return nil
	default:
		return ErrUnsupported
	}
}

// readSOS checks the scan header as turbo get_sos does.
func (f *Frame) readSOS(seg []byte) error {
	if !f.HasSOF {
		return errSOSNoSOF
	}
	if len(seg) < 1 {
		return errLength
	}
	n := int(seg[0])
	if n < 1 || n > 4 || len(seg) != 4+2*n {
		return errLength
	}
	var picked [4]int
	for k := 0; k < n; k++ {
		cc := seg[1+2*k]
		// turbo quirk: ci < k never matches.
		ci := 0
		for ci < f.NComp && (f.Comp[ci].ID != cc || ci < k) {
			ci++
		}
		if ci == f.NComp {
			return errComponentID
		}
		for p := 0; p < k; p++ {
			if picked[p] == ci {
				return errComponentID
			}
		}
		picked[k] = ci

		td := seg[2+2*k] >> 4
		ta := seg[2+2*k] & 0xf
		// jdphuff.c rejects bad selectors late.
		if !f.Progressive && (td > 3 || ta > 3) {
			return errScanSelector
		}
		f.ScanComp[k], f.ScanDC[k], f.ScanAC[k] = ci, td, ta
	}
	f.ScanN = n
	f.ScanSs, f.ScanSe, f.ScanAh, f.ScanAl = seg[1+2*n], seg[2+2*n], seg[3+2*n]>>4, seg[3+2*n]&15
	return nil
}

// resolveColor follows turbo jdapimin.c:137-215: JFIF wins over Adobe,
// Adobe transform 0 means RGB, and with neither marker ids R,G,B mean RGB.
func (f *Frame) resolveColor() {
	switch {
	case f.NComp == 1:
		f.RGB = false
	case f.jfif:
		f.RGB = false
	case f.adobe:
		f.RGB = f.adobeTransform == 0
	case f.NComp == 3 && f.Comp[0].ID == 'R' && f.Comp[1].ID == 'G' && f.Comp[2].ID == 'B':
		f.RGB = true
	default:
		f.RGB = false
	}
}
