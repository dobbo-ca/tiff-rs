//go:build !(amd64 || arm64) || purego

package codec

func writeBlock(bw *bitWriter, blk *[64]int16, lastDC *int32, dc, ac *[256]HuffCode) {
	writeBlockGo(bw, blk, lastDC, dc, ac)
}
