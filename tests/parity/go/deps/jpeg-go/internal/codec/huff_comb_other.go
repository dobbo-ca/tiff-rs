//go:build !arm64 || purego

package codec

// 8-bit table, no flagged entries.
const (
	combBits = 8
	combZero = false
)
