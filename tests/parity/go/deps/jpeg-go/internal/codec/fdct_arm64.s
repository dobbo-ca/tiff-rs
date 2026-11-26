//go:build !purego

#include "textflag.h"

// fdctQuantAsm is fdctQuantRowsGo: jfdctint.c's islow FDCT then
// jcdctmgr.c's reciprocal quantize, in 16-bit lanes with 32-bit
// products. Byte input bounds every 16-bit sum: pass-1 outputs lie in
// [-4096, 4080], so pass-2 sums of eight stay in [-32768, 32640].
// Constants regrouped as in fdct_amd64.s.

// Go's assembler has no RSHRN: DESCALE(x, shift) narrowed to .H4/.H8.
#define RSHRN_H(shift, n, d) WORD $(0x0F008C00 | ((32-(shift))<<16) | ((n)<<5) | (d))
#define RSHRN2_H(shift, n, d) WORD $(0x4F008C00 | ((32-(shift))<<16) | ((n)<<5) | (d))
// Nor SABA or SQDMULH (.8H forms).
#define SABA_H(m, n, d) WORD $(0x4E607C00 | ((m)<<16) | ((n)<<5) | (d))
#define SQDMULH_H(m, n, d) WORD $(0x4E60B400 | ((m)<<16) | ((n)<<5) | (d))
// Nor UADDL/USUBL{2} (.8B to .8H).
#define UADDL_B(m, n, d) WORD $(0x2E200000 | ((m)<<16) | ((n)<<5) | (d))
#define UADDL2_B(m, n, d) WORD $(0x6E200000 | ((m)<<16) | ((n)<<5) | (d))
#define USUBL_B(m, n, d) WORD $(0x2E202000 | ((m)<<16) | ((n)<<5) | (d))
#define USUBL2_B(m, n, d) WORD $(0x6E202000 | ((m)<<16) | ((n)<<5) | (d))
#define CMLT0_H(n, d) WORD $(0x4E60A800 | ((n)<<5) | (d))

// TRANSPOSE8 turns rows a0-a7 into columns b0-b7; a0-a7 are clobbered.
#define TRANSPOSE8(a0, a1, a2, a3, a4, a5, a6, a7, b0, b1, b2, b3, b4, b5, b6, b7) \
	VTRN1 a1.H8, a0.H8, b0.H8; \
	VTRN2 a1.H8, a0.H8, b1.H8; \
	VTRN1 a3.H8, a2.H8, b2.H8; \
	VTRN2 a3.H8, a2.H8, b3.H8; \
	VTRN1 a5.H8, a4.H8, b4.H8; \
	VTRN2 a5.H8, a4.H8, b5.H8; \
	VTRN1 a7.H8, a6.H8, b6.H8; \
	VTRN2 a7.H8, a6.H8, b7.H8; \
	VTRN1 b2.S4, b0.S4, a0.S4; \
	VTRN2 b2.S4, b0.S4, a2.S4; \
	VTRN1 b3.S4, b1.S4, a1.S4; \
	VTRN2 b3.S4, b1.S4, a3.S4; \
	VTRN1 b6.S4, b4.S4, a4.S4; \
	VTRN2 b6.S4, b4.S4, a6.S4; \
	VTRN1 b7.S4, b5.S4, a5.S4; \
	VTRN2 b7.S4, b5.S4, a7.S4; \
	VTRN1 a4.D2, a0.D2, b0.D2; \
	VTRN2 a4.D2, a0.D2, b4.D2; \
	VTRN1 a5.D2, a1.D2, b1.D2; \
	VTRN2 a5.D2, a1.D2, b5.D2; \
	VTRN1 a6.D2, a2.D2, b2.D2; \
	VTRN2 a6.D2, a2.D2, b6.D2; \
	VTRN1 a7.D2, a3.D2, b3.D2; \
	VTRN2 a7.D2, a3.D2, b7.D2

// QUANT row x; needs V0 zero.
// xn, cn, bn: WORD register numbers.
#define QUANT(x, xn, c, cn, bn, s) \
	SABA_H(0, xn, cn); \
	SQDMULH_H(bn, cn, 10); \
	VADD    c.H8, V10.H8, V10.H8; \
	VUSHL   s.H8, V10.H8, V10.H8; \
	CMLT0_H(xn, 11); \
	VEOR    V11.B16, V10.B16, V10.B16; \
	VSUB    V11.H8, V10.H8, V10.H8; \
	VST1.P  [V10.H8], 16(R3)

// V19: 1024 (8*128 level shift); V20-V31: jfdctint.c FIX sums (fdctK).
DATA fdctConsts<>+0(SB)/8, $0x0400040004000400
DATA fdctConsts<>+8(SB)/8, $0x0400040004000400
DATA fdctConsts<>+16(SB)/8, $0x29cf29cf29cf29cf // F0541+F0765
DATA fdctConsts<>+24(SB)/8, $0x29cf29cf29cf29cf
DATA fdctConsts<>+32(SB)/8, $0x1151115111511151 // F0541
DATA fdctConsts<>+40(SB)/8, $0x1151115111511151
DATA fdctConsts<>+48(SB)/8, $0xd630d630d630d630 // F0541-F1847
DATA fdctConsts<>+56(SB)/8, $0xd630d630d630d630
DATA fdctConsts<>+64(SB)/8, $0xe6dce6dce6dce6dc // F1175-F1961
DATA fdctConsts<>+72(SB)/8, $0xe6dce6dce6dce6dc
DATA fdctConsts<>+80(SB)/8, $0x25a125a125a125a1 // F1175
DATA fdctConsts<>+88(SB)/8, $0x25a125a125a125a1
DATA fdctConsts<>+96(SB)/8, $0x1925192519251925 // F1175-F0390
DATA fdctConsts<>+104(SB)/8, $0x1925192519251925
DATA fdctConsts<>+112(SB)/8, $0xecc1ecc1ecc1ecc1 // F0298-F0899
DATA fdctConsts<>+120(SB)/8, $0xecc1ecc1ecc1ecc1
DATA fdctConsts<>+128(SB)/8, $0xe333e333e333e333 // -F0899
DATA fdctConsts<>+136(SB)/8, $0xe333e333e333e333
DATA fdctConsts<>+144(SB)/8, $0x133e133e133e133e // F1501-F0899
DATA fdctConsts<>+152(SB)/8, $0x133e133e133e133e
DATA fdctConsts<>+160(SB)/8, $0xefb0efb0efb0efb0 // F2053-F2562
DATA fdctConsts<>+168(SB)/8, $0xefb0efb0efb0efb0
DATA fdctConsts<>+176(SB)/8, $0xadfdadfdadfdadfd // -F2562
DATA fdctConsts<>+184(SB)/8, $0xadfdadfdadfdadfd
DATA fdctConsts<>+192(SB)/8, $0x1051105110511051 // F3072-F2562
DATA fdctConsts<>+200(SB)/8, $0x1051105110511051
GLOBL fdctConsts<>(SB), RODATA|NOPTR, $208

// func fdctQuantAsm(pix *byte, stride int, div *fdctDivisors, coef *[64]int16)
TEXT ·fdctQuantAsm(SB), NOSPLIT, $0-32
	MOVD pix+0(FP), R0
	MOVD stride+8(FP), R1
	MOVD div+16(FP), R2
	MOVD coef+24(FP), R3
	MOVD $fdctConsts<>(SB), R4
	VLD1.P 64(R4), [V19.H8, V20.H8, V21.H8, V22.H8]
	VLD1.P 64(R4), [V23.H8, V24.H8, V25.H8, V26.H8]
	VLD1.P 64(R4), [V27.H8, V28.H8, V29.H8, V30.H8]
	VLD1   (R4), [V31.H8]

	// Rows r, r+4 paired: byte transpose.
	ADD    R1<<2, R0, R5
	VLD1.P (R0)(R1), [V0.B8]
	VLD1.P (R5)(R1), V0.D[1]
	VLD1.P (R0)(R1), [V1.B8]
	VLD1.P (R5)(R1), V1.D[1]
	VLD1.P (R0)(R1), [V2.B8]
	VLD1.P (R5)(R1), V2.D[1]
	VLD1   (R0), [V3.B8]
	VLD1   (R5), V3.D[1]
	VTRN1  V1.B16, V0.B16, V4.B16
	VTRN2  V1.B16, V0.B16, V5.B16
	VTRN1  V3.B16, V2.B16, V6.B16
	VTRN2  V3.B16, V2.B16, V7.B16
	VTRN1  V6.H8, V4.H8, V0.H8
	VTRN2  V6.H8, V4.H8, V1.H8
	VTRN1  V7.H8, V5.H8, V2.H8
	VTRN2  V7.H8, V5.H8, V3.H8
	VUZP1  V2.S4, V0.S4, V4.S4 // columns 0|1
	VUZP2  V0.S4, V2.S4, V5.S4 // columns 5|4
	VUZP1  V3.S4, V1.S4, V6.S4 // columns 2|3
	VUZP2  V1.S4, V3.S4, V7.S4 // columns 7|6

	// Pass 1: lanes are rows. Pixels are not yet level-shifted;
	// only the DC sum sees the shift. Widening sums skip USHLL.
	UADDL_B(7, 4, 0)   // tmp0
	USUBL_B(7, 4, 15)  // tmp7
	UADDL2_B(7, 4, 1)  // tmp1
	USUBL2_B(7, 4, 14) // tmp6
	UADDL_B(5, 6, 2)   // tmp2
	USUBL_B(5, 6, 13)  // tmp5
	UADDL2_B(5, 6, 3)  // tmp3
	USUBL2_B(5, 6, 12) // tmp4
	VADD V3.H8, V0.H8, V4.H8   // tmp10
	VSUB V3.H8, V0.H8, V5.H8   // tmp13
	VADD V2.H8, V1.H8, V6.H8   // tmp11
	VSUB V2.H8, V1.H8, V7.H8   // tmp12

	VADD V6.H8, V4.H8, V8.H8
	VSUB V19.H8, V8.H8, V8.H8
	VSHL $2, V8.H8, V8.H8      // out0
	VSUB V6.H8, V4.H8, V9.H8
	VSHL $2, V9.H8, V9.H8      // out4

	VSMULL  V20.H4, V5.H4, V0.S4
	VSMULL2 V20.H8, V5.H8, V1.S4
	VSMLAL  V21.H4, V7.H4, V0.S4
	VSMLAL2 V21.H8, V7.H8, V1.S4
	RSHRN_H(11, 0, 10)
	RSHRN2_H(11, 1, 10)        // out2
	VSMULL  V21.H4, V5.H4, V2.S4
	VSMULL2 V21.H8, V5.H8, V3.S4
	VSMLAL  V22.H4, V7.H4, V2.S4
	VSMLAL2 V22.H8, V7.H8, V3.S4
	RSHRN_H(11, 2, 11)
	RSHRN2_H(11, 3, 11)        // out6

	VADD    V14.H8, V12.H8, V0.H8 // z3
	VADD    V15.H8, V13.H8, V1.H8 // z4
	VSMULL  V23.H4, V0.H4, V2.S4
	VSMULL2 V23.H8, V0.H8, V3.S4
	VSMLAL  V24.H4, V1.H4, V2.S4
	VSMLAL2 V24.H8, V1.H8, V3.S4  // Q37
	VSMULL  V24.H4, V0.H4, V4.S4
	VSMULL2 V24.H8, V0.H8, V5.S4
	VSMLAL  V25.H4, V1.H4, V4.S4
	VSMLAL2 V25.H8, V1.H8, V5.S4  // Q14

	VSMULL  V26.H4, V12.H4, V6.S4
	VSMULL2 V26.H8, V12.H8, V7.S4
	VSMLAL  V27.H4, V15.H4, V6.S4
	VSMLAL2 V27.H8, V15.H8, V7.S4
	VADD    V2.S4, V6.S4, V6.S4
	VADD    V3.S4, V7.S4, V7.S4
	VSMULL  V28.H4, V15.H4, V16.S4
	VSMULL2 V28.H8, V15.H8, V17.S4
	VSMLAL  V27.H4, V12.H4, V16.S4
	VSMLAL2 V27.H8, V12.H8, V17.S4
	VADD    V4.S4, V16.S4, V16.S4
	VADD    V5.S4, V17.S4, V17.S4
	RSHRN_H(11, 6, 12)
	RSHRN2_H(11, 7, 12)           // out7
	RSHRN_H(11, 16, 15)
	RSHRN2_H(11, 17, 15)          // out1

	VSMLAL  V31.H4, V14.H4, V2.S4
	VSMLAL2 V31.H8, V14.H8, V3.S4
	VSMLAL  V30.H4, V13.H4, V2.S4
	VSMLAL2 V30.H8, V13.H8, V3.S4
	VSMLAL  V29.H4, V13.H4, V4.S4
	VSMLAL2 V29.H8, V13.H8, V5.S4
	VSMLAL  V30.H4, V14.H4, V4.S4
	VSMLAL2 V30.H8, V14.H8, V5.S4
	RSHRN_H(11, 2, 14)
	RSHRN2_H(11, 3, 14)           // out3
	RSHRN_H(11, 4, 13)
	RSHRN2_H(11, 5, 13)           // out5

	// out0-7 are in V8, V15, V10, V14, V9, V13, V11, V12.
	TRANSPOSE8(V8, V15, V10, V14, V9, V13, V11, V12, V0, V1, V2, V3, V4, V5, V6, V7)

	// Pass 2: V0-V7 are rows 0-7, lanes are columns.
	VADD V7.H8, V0.H8, V8.H8   // tmp0
	VSUB V7.H8, V0.H8, V7.H8   // tmp7
	VADD V6.H8, V1.H8, V9.H8   // tmp1
	VSUB V6.H8, V1.H8, V6.H8   // tmp6
	VADD V5.H8, V2.H8, V10.H8  // tmp2
	VSUB V5.H8, V2.H8, V5.H8   // tmp5
	VADD V4.H8, V3.H8, V11.H8  // tmp3
	VSUB V4.H8, V3.H8, V4.H8   // tmp4
	VADD V11.H8, V8.H8, V0.H8  // tmp10
	VSUB V11.H8, V8.H8, V3.H8  // tmp13
	VADD V10.H8, V9.H8, V1.H8  // tmp11
	VSUB V10.H8, V9.H8, V2.H8  // tmp12

	VADD   V1.H8, V0.H8, V8.H8
	VSRSHR $2, V8.H8, V8.H8    // out0
	VSUB   V1.H8, V0.H8, V9.H8
	VSRSHR $2, V9.H8, V9.H8    // out4

	VSMULL  V20.H4, V3.H4, V10.S4
	VSMULL2 V20.H8, V3.H8, V11.S4
	VSMLAL  V21.H4, V2.H4, V10.S4
	VSMLAL2 V21.H8, V2.H8, V11.S4
	RSHRN_H(15, 10, 1)
	RSHRN2_H(15, 11, 1)        // out2
	VSMULL  V21.H4, V3.H4, V12.S4
	VSMULL2 V21.H8, V3.H8, V13.S4
	VSMLAL  V22.H4, V2.H4, V12.S4
	VSMLAL2 V22.H8, V2.H8, V13.S4
	RSHRN_H(15, 12, 2)
	RSHRN2_H(15, 13, 2)        // out6

	VADD    V6.H8, V4.H8, V0.H8   // z3
	VADD    V7.H8, V5.H8, V3.H8   // z4
	VSMULL  V23.H4, V0.H4, V10.S4
	VSMULL2 V23.H8, V0.H8, V11.S4
	VSMLAL  V24.H4, V3.H4, V10.S4
	VSMLAL2 V24.H8, V3.H8, V11.S4 // Q37
	VSMULL  V24.H4, V0.H4, V12.S4
	VSMULL2 V24.H8, V0.H8, V13.S4
	VSMLAL  V25.H4, V3.H4, V12.S4
	VSMLAL2 V25.H8, V3.H8, V13.S4 // Q14

	VSMULL  V26.H4, V4.H4, V14.S4
	VSMULL2 V26.H8, V4.H8, V15.S4
	VSMLAL  V27.H4, V7.H4, V14.S4
	VSMLAL2 V27.H8, V7.H8, V15.S4
	VADD    V10.S4, V14.S4, V14.S4
	VADD    V11.S4, V15.S4, V15.S4
	VSMULL  V28.H4, V7.H4, V16.S4
	VSMULL2 V28.H8, V7.H8, V17.S4
	VSMLAL  V27.H4, V4.H4, V16.S4
	VSMLAL2 V27.H8, V4.H8, V17.S4
	VADD    V12.S4, V16.S4, V16.S4
	VADD    V13.S4, V17.S4, V17.S4
	RSHRN_H(15, 14, 4)
	RSHRN2_H(15, 15, 4)           // out7
	RSHRN_H(15, 16, 7)
	RSHRN2_H(15, 17, 7)           // out1

	VSMLAL  V31.H4, V6.H4, V10.S4
	VSMLAL2 V31.H8, V6.H8, V11.S4
	VSMLAL  V30.H4, V5.H4, V10.S4
	VSMLAL2 V30.H8, V5.H8, V11.S4
	VSMLAL  V29.H4, V5.H4, V12.S4
	VSMLAL2 V29.H8, V5.H8, V13.S4
	VSMLAL  V30.H4, V6.H4, V12.S4
	VSMLAL2 V30.H8, V6.H8, V13.S4
	RSHRN_H(15, 10, 6)
	RSHRN2_H(15, 11, 6)           // out3
	RSHRN_H(15, 12, 5)
	RSHRN2_H(15, 13, 5)           // out5

	// Rows 0-7 are in V8, V7, V1, V6, V9, V5, V2, V4.
	// Divisor rows split: registers run out.
	ADD    $128, R2, R5
	ADD    $384, R2, R6
	ADD    $512, R2, R7
	VEOR   V0.B16, V0.B16, V0.B16
	VLD1.P 64(R5), [V16.H8, V17.H8, V18.H8, V19.H8]
	VLD1.P 64(R6), [V20.H8, V21.H8, V22.H8, V23.H8]
	VLD1.P 64(R7), [V24.H8, V25.H8, V26.H8, V27.H8]
	QUANT(V8, 8, V16, 16, 20, V24)
	QUANT(V7, 7, V17, 17, 21, V25)
	QUANT(V1, 1, V18, 18, 22, V26)
	QUANT(V6, 6, V19, 19, 23, V27)
	VLD1   (R5), [V16.H8, V17.H8, V18.H8, V19.H8]
	VLD1   (R6), [V20.H8, V21.H8, V22.H8, V23.H8]
	VLD1   (R7), [V24.H8, V25.H8, V26.H8, V27.H8]
	QUANT(V9, 9, V16, 16, 20, V24)
	QUANT(V5, 5, V17, 17, 21, V25)
	QUANT(V2, 2, V18, 18, 22, V26)
	QUANT(V4, 4, V19, 19, 23, V27)
	RET
