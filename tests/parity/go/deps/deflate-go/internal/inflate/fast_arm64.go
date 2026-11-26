//go:build !purego

package inflate

// fastLoopAsm is fast_arm64.s.
//
//go:noescape
func fastLoopAsm(s *fastState)

// fastLoopPairAsm is fastLoopAsm on s.pair.
//
//go:noescape
func fastLoopPairAsm(s *fastState)

// fastLoopPick takes the pair loop if s has a pair table.
func fastLoopPick(s *fastState) {
	if s.pair != nil {
		fastLoopPairAsm(s)
		return
	}
	fastLoopAsm(s)
}

func init() { fastLoop, pairLoop = fastLoopPick, true }
