//go:build purego || !(amd64 || arm64)

package adler32

func update(adler uint32, p []byte) uint32 { return updateGo(adler, p) }
