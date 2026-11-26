//go:build !purego

package codec

// writeBlockSIMD: writeBlockAsm needs huffPrepAsm and BMI1/2.
var writeBlockSIMD = huffPrepSIMD && hasBMI2() && hasBMI1()

// writeBlockAVX512 selects writeBlockAsm's inline AVX-512 huffPrep.
var writeBlockAVX512 = writeBlockSIMD && hasAVX512()

// hasAVX512: F, BW, CD, VBMI, XCR0.
func hasAVX512() bool {
	if maxID, _, _, _ := cpuid(0, 0); maxID < 7 {
		return false
	}
	if xcr0, _ := xgetbv(); xcr0&0xe6 != 0xe6 {
		return false
	}
	_, ebx7, ecx7, _ := cpuid(7, 0)
	const f, cd, bw, vbmi = 1 << 16, 1 << 28, 1 << 30, 1 << 1
	return ebx7&(f|cd|bw) == f|cd|bw && ecx7&vbmi != 0
}

func hasBMI1() bool {
	if maxID, _, _, _ := cpuid(0, 0); maxID < 7 {
		return false
	}
	_, ebx7, _, _ := cpuid(7, 0)
	return ebx7&(1<<3) != 0
}

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
