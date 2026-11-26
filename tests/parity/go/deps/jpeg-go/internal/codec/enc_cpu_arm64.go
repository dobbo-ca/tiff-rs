//go:build !purego

package codec

// encHasSIMD: NEON is baseline on arm64.
func encHasSIMD() bool { return true }
