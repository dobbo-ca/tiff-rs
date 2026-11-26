//go:build !(amd64 || arm64) || purego

package codec

func huffPrep(blk *[64]int16, vals *[64]uint16, nb *[64]uint8) uint64 {
	return huffPrepGo(blk, vals, nb)
}
