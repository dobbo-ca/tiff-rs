//go:build !purego

package codec

// writeBlockSIMD: writeBlockAsm inlines huffPrepAsm.
var writeBlockSIMD = huffPrepSIMD

// writeBlockAsm is writeBlockGo; needs maxBlockBytes room.
//
//go:noescape
func writeBlockAsm(bw *bitWriter, blk *[64]int16, lastDC *int32, dc, ac *[256]HuffCode)

func writeBlock(bw *bitWriter, blk *[64]int16, lastDC *int32, dc, ac *[256]HuffCode) {
	if writeBlockSIMD && cap(bw.out)-len(bw.out) >= maxBlockBytes {
		writeBlockAsm(bw, blk, lastDC, dc, ac)
		return
	}
	writeBlockGo(bw, blk, lastDC, dc, ac)
}
