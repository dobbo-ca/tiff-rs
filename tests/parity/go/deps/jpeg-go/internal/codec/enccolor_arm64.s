//go:build !purego

#include "textflag.h"

// Go's assembler lacks these; operands in Go order (m, n, d).
// USUBL: Vd.8H = Vn.8B - Vm.8B, widened; USUBL2 takes the high halves.
#define USUBL(m, n, d) WORD $(0x2E202000 | ((m)<<16) | ((n)<<5) | (d))
#define USUBL2(m, n, d) WORD $(0x6E202000 | ((m)<<16) | ((n)<<5) | (d))
// RSHRN #16: Vd.4H (RSHRN2: Vd.8H high half) = (Vn.4S + 1<<15) >> 16.
#define RSHRN16(n, d) WORD $(0x0F108C00 | ((n)<<5) | (d))
#define RSHRN216(n, d) WORD $(0x4F108C00 | ((n)<<5) | (d))
// SQDMULH Vd.8H = sat(2*Vn*Vm >> 16); SHSUB Vd.8H = (Vn - Vm) >> 1.
#define SQDMULH_H(m, n, d) WORD $(0x4E60B400 | ((m)<<16) | ((n)<<5) | (d))
#define SHSUB_H(m, n, d) WORD $(0x4E602400 | ((m)<<16) | ((n)<<5) | (d))

// func rgbaToYCbCrAsm(src, y, cb, cr *byte, n int)
//
// With dr = r-g and db = b-g, RGBToYCbCr's weights give exactly
// y = g + (19595dr + 7471db + 32768) >> 16 (the Y weights sum to 65536),
// cb = 128 + (db - (11059dr >> 15)) >> 1 and cr = 128 + (dr - (5329db >> 15)) >> 1
// (11059+21709 = 27439+5329 = 32768, and x>>15 then >>1 floors like >>16
// after the +32767). |dr|, |db| <= 255, so SQDMULH never saturates.
TEXT ·rgbaToYCbCrAsm(SB), NOSPLIT, $0-40
	MOVD src+0(FP), R0
	MOVD y+8(FP), R1
	MOVD cb+16(FP), R2
	MOVD cr+24(FP), R3
	MOVD n+32(FP), R4
	MOVD $19595, R5
	VDUP R5, V16.H8
	MOVD $7471, R5
	VDUP R5, V17.H8
	MOVD $11059, R5
	VDUP R5, V18.H8
	MOVD $5329, R5
	VDUP R5, V19.H8
	MOVD $0x80, R5
	VDUP R5, V20.B16

loop:
	VLD4.P 64(R0), [V0.B16, V1.B16, V2.B16, V3.B16]
	USUBL(1, 0, 4)  // dr
	USUBL2(1, 0, 5)
	USUBL(1, 2, 6)  // db
	USUBL2(1, 2, 7)

	VSMULL  V16.H4, V4.H4, V8.S4
	VSMULL2 V16.H8, V4.H8, V9.S4
	VSMULL  V16.H4, V5.H4, V10.S4
	VSMULL2 V16.H8, V5.H8, V11.S4
	VSMLAL  V17.H4, V6.H4, V8.S4
	VSMLAL2 V17.H8, V6.H8, V9.S4
	VSMLAL  V17.H4, V7.H4, V10.S4
	VSMLAL2 V17.H8, V7.H8, V11.S4
	RSHRN16(8, 12)
	RSHRN216(9, 12)
	RSHRN16(10, 13)
	RSHRN216(11, 13)
	VUZP1  V13.B16, V12.B16, V14.B16
	VADD   V1.B16, V14.B16, V14.B16 // + g, mod 256
	VST1.P [V14.B16], 16(R1)

	SQDMULH_H(18, 4, 8)
	SQDMULH_H(18, 5, 9)
	SHSUB_H(8, 6, 8)
	SHSUB_H(9, 7, 9)
	VUZP1  V9.B16, V8.B16, V14.B16
	VEOR   V20.B16, V14.B16, V14.B16 // + 128, mod 256
	VST1.P [V14.B16], 16(R2)

	SQDMULH_H(19, 6, 8)
	SQDMULH_H(19, 7, 9)
	SHSUB_H(8, 4, 8)
	SHSUB_H(9, 5, 9)
	VUZP1  V9.B16, V8.B16, V14.B16
	VEOR   V20.B16, V14.B16, V14.B16
	VST1.P [V14.B16], 16(R3)

	SUBS $16, R4, R4
	BNE  loop
	RET

// func downsampleH2V2Asm(dst, r0, r1 *byte, n int)
TEXT ·downsampleH2V2Asm(SB), NOSPLIT, $0-32
	MOVD dst+0(FP), R0
	MOVD r0+8(FP), R1
	MOVD r1+16(FP), R2
	MOVD n+24(FP), R3
	MOVD $0x00020001, R4 // bias 1,2 per column pair
	VDUP R4, V7.S4

dloop:
	VLD2.P  32(R1), [V0.B16, V1.B16]
	VLD2.P  32(R2), [V2.B16, V3.B16]
	VUXTL   V0.B8, V4.H8
	VUXTL2  V0.B16, V5.H8
	VUADDW  V1.B8, V4.H8, V4.H8
	VUADDW2 V1.B16, V5.H8, V5.H8
	VUADDW  V2.B8, V4.H8, V4.H8
	VUADDW2 V2.B16, V5.H8, V5.H8
	VUADDW  V3.B8, V4.H8, V4.H8
	VUADDW2 V3.B16, V5.H8, V5.H8
	VADD    V7.H8, V4.H8, V4.H8
	VADD    V7.H8, V5.H8, V5.H8
	VSHRN   $2, V4.H8, V6.B8
	VSHRN2  $2, V5.H8, V6.B16
	VST1.P  [V6.B16], 16(R0)
	SUBS    $16, R3, R3
	BNE     dloop
	RET
