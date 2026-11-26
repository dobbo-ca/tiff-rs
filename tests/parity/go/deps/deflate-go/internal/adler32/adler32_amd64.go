//go:build !purego

package adler32

// Bulk loops: len(p) is a nonzero multiple of 64.
//
//go:noescape
func updateSSSE3(adler uint32, p []byte) uint32

//go:noescape
func updateAVX2(adler uint32, p []byte) uint32

//go:noescape
func updateVNNI512(adler uint32, p []byte) uint32

// modMul is MOD_MUL, for tests.
func modMul(r uint64) uint64

func cpuFeatures() (ssse3, avx2, vnni bool)

// Bulk loops, slowest first.
const (
	loopGo = iota
	loopSSSE3
	loopAVX2
	loopVNNI512
)

var loopNames = [...]string{"go", "ssse3", "avx2", "vnni512"}

// maxLoop is the fastest loop this CPU runs.
var maxLoop = func() int {
	ssse3, avx2, vnni := cpuFeatures()
	switch {
	case vnni:
		return loopVNNI512
	case avx2:
		return loopAVX2
	case ssse3:
		return loopSSSE3
	}
	return loopGo
}()

var bulk = maxLoop

// Use makes Update use the named loop, for benchmarks. It
// reports whether this CPU runs it.
func Use(name string) bool {
	for i, n := range loopNames[:maxLoop+1] {
		if n == name {
			bulk = i
			return true
		}
	}
	return false
}

func update(adler uint32, p []byte) uint32 {
	for len(p) >= 64 && bulk != loopGo {
		n := min(len(p), 1<<20) &^ 63 // asm is not preemptible
		switch bulk {
		case loopSSSE3:
			adler = updateSSSE3(adler, p[:n])
		case loopAVX2:
			adler = updateAVX2(adler, p[:n])
		default:
			adler = updateVNNI512(adler, p[:n])
		}
		p = p[n:]
	}
	return updateGo(adler, p)
}
