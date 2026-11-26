//go:build !purego

package inflate

// fastLoopV1 runs on any amd64 (GOAMD64=v1).
//
//go:noescape
func fastLoopV1(s *fastState)

// fastLoopBMI2 needs BMI2 and SSSE3.
//
//go:noescape
func fastLoopBMI2(s *fastState)

// fastLoopBMI2Pair is fastLoopBMI2 on s.pair.
//
//go:noescape
func fastLoopBMI2Pair(s *fastState)

// fastLoopBMI2Pick takes the pair loop if s has a pair table.
func fastLoopBMI2Pick(s *fastState) {
	if s.pair != nil {
		fastLoopBMI2Pair(s)
		return
	}
	fastLoopBMI2(s)
}

func hasBMI2() bool

func init() {
	fastLoop = fastLoopV1
	if hasBMI2() {
		fastLoop, pairLoop = fastLoopBMI2Pick, true
	}
}
