//go:build !(amd64 || arm64) || purego

package codec

// fastMCU is turbo's fast path; nil blks skips.
func fastMCU(r *bitReader, dc, ac *[4]derivedTable, lastDC *[4]int32, blks *[maxMCUBlocks]*Block, ks *[maxMCUBlocks]int, n int, hasAC *[maxMCUBlocks]bool) bool {
	return fastMCUGo(r, dc, ac, lastDC, blks, ks, n, hasAC)
}
