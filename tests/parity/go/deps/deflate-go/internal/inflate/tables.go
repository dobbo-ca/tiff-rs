package inflate

import "math/bits"

// Fast tables, shared by the Go loop and the assembly loops.
//
// Both tables are indexed by the low bits of the bit buffer: llBits
// bits for litlen, distBits bits for distance. Codes longer than
// that go through a link entry to a subtable, indexed by the next
// bits above the primary bits. Each entry is a uint32:
//
//	bits  0-4   bits to consume: code length + extra bits
//	bit   5     linkFlag: value is a subtable start, bits 8-11 its index width
//	bit   6     stopFlag: the careful path decodes this symbol
//	bit   7     litFlag: value is a literal byte (litlen only)
//	bits  8-11  code length n
//	bits 12-15  litlen only: pullN = max(minLL, n), the bits stdlib reads
//	bits 16-31  length base, distance base, or subtable start
//	bits 16-23  literal byte
//	bits 24-27  literal only: pullN - n, so slack is e >> 24
//
// For a length or distance, the extra bits value is
// (bitbuf & (1<<consume - 1)) >> n. Subtable entries hold the full
// code length, so one shift consumes code and extra bits together.
//
// Stop entries: EOB (256), litlen 286/287, distance 30/31, and
// every code missing from an incomplete tree (degenerate or empty).
// stdlib's init rejects all other incomplete trees, so subtables
// only exist for complete trees. A stop entry has bits 0-7 =
// stopFlag; for a symbol, bits 8-11 hold n and bits 16-31 the
// symbol, for the careful path. Missing codes have n = 0.
//
// Pair table (pair loops only): the primary entries, except
// where literal codes n1 at i and n2 at i>>n1 fit in llBits bits:
//
//	bits  0-4   n1 + n2
//	bit   6     pairFlag, only with litFlag
//	bit   7     litFlag
//	bits  8-15  second literal's bits 8-15: slack is pullN - n
//	bits 16-23  first literal
//	bits 24-31  second literal
//
// So bits 16-31 of any literal entry are its output halfword (a
// single's second byte is junk), bit 6 adds the second byte, and
// pullN - n from bits 8-15 is the last literal's slack.
const (
	llBits   = 11
	distBits = 8
	llMask   = 1<<llBits - 1
	distMask = 1<<distBits - 1

	linkFlag = 1 << 5
	stopFlag = 1 << 6
	litFlag  = 1 << 7
	pairFlag = 1 << 6

	// Subtables hold >= 2 codes each.
	llTableSize   = 1<<llBits + 288/2<<(15-llBits)
	distTableSize = 1<<distBits + 32/2<<(15-distBits)
)

type litlenTable [llTableSize]uint32
type distTable [distTableSize]uint32
type pairTable [1 << llBits]uint32

// Pair table modes.
const (
	PairsOff  = iota // never
	PairsOn          // whenever a pair fits
	PairsAuto        // when the code lengths say pairs pay
)

// Pairs is the pair table mode; benchmarks set it.
var Pairs = PairsAuto

// Fixed tables, RFC 1951 3.2.6.
var fixedLL litlenTable
var fixedDT distTable

// Entries before code length is added.
var llSyms, distSyms = symEntries()

func symEntries() (ll [288]uint32, dist [32]uint32) {
	for v := range ll {
		var base, extra int
		switch {
		case v < 256:
			ll[v] = uint32(v)<<16 | litFlag
			continue
		case v == 256 || v >= maxNumLit:
			ll[v] = uint32(v)<<16 | stopFlag
			continue
		case v < 265:
			base, extra = v-(257-3), 0
		case v < 285:
			extra = (v - 261) / 4
			base = (v-261)%4<<extra + 1<<(extra+2) + 3
		default:
			base, extra = 258, 0
		}
		ll[v] = uint32(base)<<16 | uint32(extra)
	}
	for d := range dist {
		switch {
		case d < 4:
			dist[d] = uint32(d+1) << 16
		case d < maxNumDist:
			nb := uint(d-2) >> 1
			dist[d] = uint32(1<<(nb+1)+1+(d&1)<<nb)<<16 | uint32(nb)
		default:
			dist[d] = uint32(d)<<16 | stopFlag
		}
	}
	return
}

// buildTable fills t from lengths stdlib accepted.
// minLL is stdlib's h1.min.
func buildTable(t []uint32, lengths []int, syms []uint32, tableBits, minLL uint) {
	var count [maxCodeLen]int
	for _, n := range lengths {
		count[n]++
	}
	ncodes := len(lengths) - count[0]
	count[0] = 0
	var next [maxCodeLen]int
	code := 0
	for i := 1; i < maxCodeLen; i++ {
		code = (code + count[i-1]) << 1
		next[i] = code
	}

	size := 1 << tableBits
	mask := size - 1
	if ncodes < 2 {
		// Incomplete: fill missing codes.
		for i := range t[:size] {
			t[i] = stopFlag
		}
	}

	// Reversed codes, and each subtable's widest code.
	var rev [288]int
	var subMax [1 << llBits]uint8
	var subs [288]uint16 // subtable prefixes, in first-use order
	nsub := 0
	for sym, n := range lengths {
		if n == 0 {
			continue
		}
		r := int(bits.Reverse16(uint16(next[n]))) >> (16 - n)
		next[n]++
		rev[sym] = r
		if uint(n) > tableBits {
			if subMax[r&mask] == 0 {
				subs[nsub] = uint16(r & mask)
				nsub++
			}
			subMax[r&mask] = max(subMax[r&mask], uint8(n))
		}
	}

	// Links first: t may hold stale links.
	free := size
	for _, p := range subs[:nsub] {
		w := uint(subMax[p]) - tableBits
		t[p] = uint32(free)<<16 | linkFlag | uint32(w)<<8
		free += 1 << w
	}
	for sym, n := range lengths {
		if n == 0 {
			continue
		}
		e := syms[sym] | uint32(n)<<8
		if e&stopFlag == 0 {
			pull := max(uint(n), minLL)
			e += uint32(n) | uint32(pull)<<12
			if e&litFlag != 0 {
				e |= uint32(pull-uint(n)) << 24
			}
		}
		r := rev[sym]
		if uint(n) <= tableBits {
			for i := r; i < size; i += 1 << n {
				t[i] = e
			}
			continue
		}
		p := r & mask
		start, w := int(t[p]>>16), uint(t[p]>>8&15)
		for i := r >> tableBits; i < 1<<w; i += 1 << (uint(n) - tableBits) {
			t[start+i] = e
		}
	}
}

// buildPairs fills p from t and the litlen lengths that built it.
// It reports false, leaving p unset, if no pair is wanted.
func buildPairs(p *pairTable, t *litlenTable, lengths []int) bool {
	if Pairs == PairsOff {
		return false
	}
	// Literals per length; lanes break store forwarding chains.
	var lanes [4][maxCodeLen]int
	lits := lengths[:256]
	for i := 0; i < len(lits); i += 4 {
		lanes[0][lits[i]]++
		lanes[1][lits[i+1]]++
		lanes[2][lits[i+2]]++
		lanes[3][lits[i+3]]++
	}
	var cnt [maxCodeLen]int
	for n := range cnt {
		cnt[n] = lanes[0][n] + lanes[1][n] + lanes[2][n] + lanes[3][n]
	}
	minLit := 1
	for minLit < maxCodeLen && cnt[minLit] == 0 {
		minLit++
	}
	maxN := llBits - minLit // longest first literal
	if maxN < minLit {
		return false
	}
	if Pairs == PairsAuto {
		// Shares of 1<<22: literal symbols, and pairs if
		// symbols were independent with p = 2^-n.
		var lit, pair int
		for n := 1; n < maxCodeLen; n++ {
			lit += cnt[n] << (22 - n)
			for n2 := minLit; n+n2 <= llBits; n2++ {
				pair += cnt[n] * cnt[n2] << (22 - n - n2)
			}
		}
		// ponytail: fixed model, a pair saves ~16x what a single
		// loses in the pair loop, build ~3/8 cycle per symbol at
		// typical block sizes; refit from bench numbers.
		if 16*pair < lit+3<<19 {
			return false
		}
	}

	// Canonical codes: rank among literals of length n,
	// plus base[n]. Sort literals by length into rs.
	all := cnt
	for _, n := range lengths[256:] {
		all[n]++
	}
	all[0] = 0
	var base, off [maxCodeLen]int
	var lim [llBits + 1]int // rs[:lim[n]]: lengths <= n
	code, k := 0, 0
	for n := 1; n < maxCodeLen; n++ {
		code = (code + all[n-1]) << 1
		off[n] = k
		base[n] = code - k
		k += cnt[n]
		if n <= llBits {
			lim[n] = k
		}
	}
	off[0] = k
	var rs [256]uint16
	for _, n := range lits {
		i := off[n]
		off[n] = i + 1
		rs[i] = bits.Reverse16(uint16(base[n]+i)) >> (16 - n)
	}

	copy(p[:], t[:])
	for _, r1 := range rs[:lim[maxN]] {
		e1 := t[r1]
		n1 := e1 & 31
		for _, r2 := range rs[:lim[llBits-n1]] {
			e2 := t[r2]
			e := e1&31 + e2&31 | litFlag | pairFlag | e2&0xff00 | e1&0xff0000 | e2<<8&0xff000000
			for i := uint(r1) | uint(r2)<<n1; i < 1<<llBits; i += 1 << (n1 + e2&31) {
				p[i] = e
			}
		}
	}
	return true
}
