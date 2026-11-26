// Copyright 2009 The Go Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSE file.

// Package adler32 is hash/adler32 with SIMD loops on amd64 and
// arm64. updateGo is a port of hash/adler32's update.
package adler32

const (
	mod  = 65521
	nmax = 5552
)

// Update returns the Adler-32 checksum of the input that gave
// adler, followed by p. The checksum of no input is 1. Results
// equal hash/adler32 for every adler and p.
func Update(adler uint32, p []byte) uint32 { return update(adler, p) }

func updateGo(adler uint32, p []byte) uint32 {
	s1, s2 := adler&0xffff, adler>>16
	for len(p) > 0 {
		var q []byte
		if len(p) > nmax {
			p, q = p[:nmax], p[nmax:]
		}
		for len(p) >= 4 {
			s1 += uint32(p[0])
			s2 += s1
			s1 += uint32(p[1])
			s2 += s1
			s1 += uint32(p[2])
			s2 += s1
			s1 += uint32(p[3])
			s2 += s1
			p = p[4:]
		}
		for _, x := range p {
			s1 += uint32(x)
			s2 += s1
		}
		s1 %= mod
		s2 %= mod
		p = q
	}
	return s2<<16 | s1
}
