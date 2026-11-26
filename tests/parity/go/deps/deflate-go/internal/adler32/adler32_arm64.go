//go:build !purego

package adler32

// updateNEON needs len(p) a nonzero multiple of 64.
//
//go:noescape
func updateNEON(adler uint32, p []byte) uint32

var useNEON = true

func update(adler uint32, p []byte) uint32 {
	for len(p) >= 64 && useNEON {
		n := min(len(p), 1<<20) &^ 63 // asm is not preemptible
		adler = updateNEON(adler, p[:n])
		p = p[n:]
	}
	return updateGo(adler, p)
}
