//go:build !purego

#include "textflag.h"
// Go asm lacks by-element forms: WORDs, index in H:L(:M).
#define SMULLE(n, m, i, d) WORD $(0x0F40A000 | ((((i)>>2)&1)<<11) | ((((i)>>1)&1)<<21) | (((i)&1)<<20) | ((m)<<16) | ((n)<<5) | (d))
#define SMULL2E(n, m, i, d) WORD $(0x4F40A000 | ((((i)>>2)&1)<<11) | ((((i)>>1)&1)<<21) | (((i)&1)<<20) | ((m)<<16) | ((n)<<5) | (d))
#define SMLALE(n, m, i, d) WORD $(0x0F402000 | ((((i)>>2)&1)<<11) | ((((i)>>1)&1)<<21) | (((i)&1)<<20) | ((m)<<16) | ((n)<<5) | (d))
#define SMLAL2E(n, m, i, d) WORD $(0x4F402000 | ((((i)>>2)&1)<<11) | ((((i)>>1)&1)<<21) | (((i)&1)<<20) | ((m)<<16) | ((n)<<5) | (d))
#define SMLSLE(n, m, i, d) WORD $(0x0F406000 | ((((i)>>2)&1)<<11) | ((((i)>>1)&1)<<21) | (((i)&1)<<20) | ((m)<<16) | ((n)<<5) | (d))
#define SMLSL2E(n, m, i, d) WORD $(0x4F406000 | ((((i)>>2)&1)<<11) | ((((i)>>1)&1)<<21) | (((i)&1)<<20) | ((m)<<16) | ((n)<<5) | (d))
#define MULSE(n, m, i, d) WORD $(0x4F808000 | ((((i)>>1)&1)<<11) | (((i)&1)<<21) | ((m)<<16) | ((n)<<5) | (d))
#define MLASE(n, m, i, d) WORD $(0x6F800000 | ((((i)>>1)&1)<<11) | (((i)&1)<<21) | ((m)<<16) | ((n)<<5) | (d))
#define SQSHRN_B(shift, n, d) WORD $(0x0F009400 | ((16-(shift))<<16) | ((n)<<5) | (d))
#define SQSHRN2_B(shift, n, d) WORD $(0x4F009400 | ((16-(shift))<<16) | ((n)<<5) | (d))

// Pass-2 constants x16, VTBL indices.
// V16.S[i] = word i, V17 = 4+i, V18 = 8+i.
GLOBL idct8P2<>(SB), RODATA, $80
DATA idct8P2<>+0(SB)/4, $70928
DATA idct8P2<>+4(SB)/4, $-171264
DATA idct8P2<>+8(SB)/4, $171248
DATA idct8P2<>+12(SB)/4, $154128
DATA idct8P2<>+16(SB)/4, $39136
DATA idct8P2<>+20(SB)/4, $269104
DATA idct8P2<>+24(SB)/4, $402752
DATA idct8P2<>+28(SB)/4, $196784
DATA idct8P2<>+32(SB)/4, $-117968
DATA idct8P2<>+36(SB)/4, $-335920
DATA idct8P2<>+40(SB)/4, $-102976
DATA idct8P2<>+44(SB)/4, $102992
DATA idct8P2<>+48(SB)/8, $0x1C1814100C080400
DATA idct8P2<>+56(SB)/8, $0x1D1915110D090501
DATA idct8P2<>+64(SB)/8, $0x1E1A16120E0A0602
DATA idct8P2<>+72(SB)/8, $0x1F1B17130F0B0703

// Pass-1 constants, int16, loaded into V12-V13.
// V12.H[0-7]: c1 c9 c6 c11 c7 c8 c3 c0.
// V13.H[0-3]: c4 c5 c2 c10.
GLOBL idct8P1<>(SB), RODATA, $32
DATA idct8P1<>+0(SB)/2, $4433
DATA idct8P1<>+2(SB)/2, $-10704
DATA idct8P1<>+4(SB)/2, $10703
DATA idct8P1<>+6(SB)/2, $-6436
DATA idct8P1<>+8(SB)/2, $9633
DATA idct8P1<>+10(SB)/2, $6437
DATA idct8P1<>+12(SB)/2, $-4927
DATA idct8P1<>+14(SB)/2, $7373
DATA idct8P1<>+16(SB)/2, $4926
DATA idct8P1<>+18(SB)/2, $-4176
DATA idct8P1<>+20(SB)/2, $20995
DATA idct8P1<>+22(SB)/2, $4177
DATA idct8P1<>+24(SB)/8, $0

// func idct8NEONAsm(coef *[64]int16, quant *[64]uint16, dst *byte, stride int)
//
// Ports turbo's jsimd_idct_islow_neon (simd/arm/jidctint-neon.c). Of
// its sparse special cases it keeps one: when columns 4-7 are all
// zero, the right half's pass 1 is skipped and pass 2 drops w4-w7. Zero
// terms add and multiply to zero, so the output is unchanged.
//
// Pass 1 (column IDCT) runs in NEON, 4 columns (a "half") at a time
// for the 32-bit products, dequantizing all 8 columns at once with a
// 16-bit truncating multiply the way turbo's NEON body does (docs/design.md section 4: the SIMD kernel only needs to
// match idct8Go, which dequantizes at full width, on in-range blocks).
// Pass 2 (row IDCT) runs in NEON too, 4 rows at a time, in 32-bit lanes
// (exact for any int32 ws, unlike turbo's 16-bit pass 2), and VTBL
// transposes the output bytes back into rows. The left/right halves of
// pass 1 are the same code, the right one using the "2" (upper-half)
// forms.
//
// ws never leaves registers. Register map:
//   pass 1: V0-V7 rows r0-r7 (.H8), V8-V11 r0, r4, z3o, z4o.
//   V12-V13 hold the constants (VLD1 from idct8P1), the rest temps.
//   Left half's rows 0-7 land in V16-V23, the right half's in V24-V31
//   (peak 23 live; pass 2 uses all 32).
//   pass 2: TRN1/TRN2 turn each half's rows 0-3 (then 4-7) into
//   w0-w7 in V0-V7, lane i of wj = row i, column j, as VLD4 of the
//   old stack ws gave. V8-V15 temps, V16-V18 constants (VLD1 from
//   idct8P2), V24 rounding, V25 0x80,
//   V26-V27 VTBL indices; rows 4-7 wait in V20-V23 and V28-V31.
//
// Constants are jidctint-neon.c's combined FIX_* values (comments below
// give each one's constituent dct.go names). Go arm64 asm's two-source
// forms read "OP src2, src1, dst" (dst = src1 OP src2), so every
// subtraction below has its operands in that, not the natural, order.
TEXT ·idct8NEONAsm(SB), $0-32
	MOVD coef+0(FP), R4
	MOVD quant+8(FP), R5

	// Load all 8 coefficient and quant rows full width; each half
	// below reads columns 0-3 (.H4) or 4-7 (the "2" forms, .H8).
	VLD1.P 64(R4), [V0.H8, V1.H8, V2.H8, V3.H8]
	VLD1 (R4), [V4.H8, V5.H8, V6.H8, V7.H8]
	VLD1.P 64(R5), [V8.H8, V9.H8, V10.H8, V11.H8]
	VLD1 (R5), [V12.H8, V13.H8, V14.H8, V15.H8]
	VMUL V8.H8, V0.H8, V8.H8 // r0, kept for both halves
	VMUL V9.H8, V1.H8, V1.H8
	VMUL V10.H8, V2.H8, V2.H8
	VMUL V11.H8, V3.H8, V3.H8
	VMUL V12.H8, V4.H8, V9.H8 // r4, likewise
	VMUL V13.H8, V5.H8, V5.H8
	VMUL V14.H8, V6.H8, V6.H8
	VMUL V15.H8, V7.H8, V7.H8

	// R10 = 0 when the products in columns 4-7 are all zero.
	VORR V1.B16, V8.B16, V12.B16
	VORR V3.B16, V2.B16, V13.B16
	VORR V5.B16, V9.B16, V14.B16
	VORR V7.B16, V6.B16, V15.B16
	VORR V13.B16, V12.B16, V12.B16
	VORR V15.B16, V14.B16, V14.B16
	VORR V14.B16, V12.B16, V12.B16
	VMOV V12.D[1], R10

	// Shared 16-bit sums can wrap.
	VADD V3.H8, V7.H8, V10.H8  // z3o = r7+r3
	VADD V1.H8, V5.H8, V11.H8  // z4o = r5+r1
	MOVD $idct8P1<>(SB), R9

	// ---- Left half: columns 0-3, rows into V16-V23. ----
	VLD1 (R9), [V12.H8, V13.H8]
	SMULLE(2, 12, 0, 15) // tmp2 = r2*c1
	SMLALE(6, 12, 1, 15)
	SMULLE(2, 12, 2, 0) // tmp3 = r2*c6
	SMLALE(6, 12, 0, 0)
	VSSHLL $13, V8.H4, V24.S4     // r0<<13
	VSSHLL $13, V9.H4, V25.S4     // r4<<13
	VSUB V25.S4, V24.S4, V26.S4   // tmp1
	VADD V25.S4, V24.S4, V24.S4   // tmp0
	VADD V0.S4, V24.S4, V4.S4     // tmp10
	VSUB V0.S4, V24.S4, V24.S4    // tmp13
	VADD V15.S4, V26.S4, V0.S4    // tmp11
	VSUB V15.S4, V26.S4, V25.S4   // tmp12

	SMULLE(10, 12, 3, 26) // z3 = z3o*c11
	SMLALE(11, 12, 4, 26)
	SMULLE(10, 12, 4, 27) // z4 = z3o*c7
	SMLALE(11, 12, 5, 27)
	SMULLE(7, 12, 6, 28) // tmp0o = r7*c3
	SMLSLE(1, 12, 7, 28)
	VADD V26.S4, V28.S4, V28.S4   //       + z3
	SMULLE(1, 13, 0, 29) // tmp3o = r1*c4
	SMLSLE(7, 12, 7, 29)
	VADD V27.S4, V29.S4, V29.S4   //       + z4
	SMULLE(5, 13, 1, 30) // tmp1o = r5*c5
	SMLSLE(3, 13, 2, 30)
	VADD V27.S4, V30.S4, V30.S4   //       + z4
	SMULLE(3, 13, 3, 31) // tmp2o = r3*c10
	SMLSLE(5, 13, 2, 31)
	VADD V26.S4, V31.S4, V31.S4   //       + z3

	VADD V29.S4, V4.S4, V16.S4
	VSRSHR $11, V16.S4, V16.S4 // row0 = tmp10+tmp3o
	VADD V31.S4, V0.S4, V17.S4
	VSRSHR $11, V17.S4, V17.S4 // row1 = tmp11+tmp2o
	VADD V30.S4, V25.S4, V18.S4
	VSRSHR $11, V18.S4, V18.S4 // row2 = tmp12+tmp1o
	VADD V28.S4, V24.S4, V19.S4
	VSRSHR $11, V19.S4, V19.S4 // row3 = tmp13+tmp0o
	VSUB V28.S4, V24.S4, V20.S4
	VSRSHR $11, V20.S4, V20.S4 // row4 = tmp13-tmp0o
	VSUB V30.S4, V25.S4, V21.S4
	VSRSHR $11, V21.S4, V21.S4 // row5 = tmp12-tmp1o
	VSUB V31.S4, V0.S4, V22.S4
	VSRSHR $11, V22.S4, V22.S4 // row6 = tmp11-tmp2o
	VSUB V29.S4, V4.S4, V23.S4
	VSRSHR $11, V23.S4, V23.S4 // row7 = tmp10-tmp3o

	// ---- Right half: columns 4-7, rows into V24-V31. ----
	CBZ R10, pass2 // all zero: pass 2 never reads it
	SMULL2E(2, 12, 0, 15) // tmp2 = r2*c1
	SMLAL2E(6, 12, 1, 15)
	SMULL2E(2, 12, 2, 0) // tmp3 = r2*c6
	SMLAL2E(6, 12, 0, 0)
	VSSHLL2 $13, V8.H8, V2.S4      // r0<<13
	VSSHLL2 $13, V9.H8, V6.S4      // r4<<13
	VSUB V6.S4, V2.S4, V8.S4       // tmp1
	VADD V6.S4, V2.S4, V2.S4       // tmp0
	VADD V0.S4, V2.S4, V4.S4       // tmp10
	VSUB V0.S4, V2.S4, V2.S4       // tmp13
	VADD V15.S4, V8.S4, V0.S4      // tmp11
	VSUB V15.S4, V8.S4, V6.S4      // tmp12

	SMULL2E(10, 12, 3, 8) // z3 = z3o*c11
	SMLAL2E(11, 12, 4, 8)
	SMULL2E(10, 12, 4, 9) // z4 = z3o*c7
	SMLAL2E(11, 12, 5, 9)
	SMULL2E(7, 12, 6, 10) // tmp0o = r7*c3
	SMLSL2E(1, 12, 7, 10)
	VADD V8.S4, V10.S4, V10.S4     //       + z3
	SMULL2E(1, 13, 0, 11) // tmp3o = r1*c4
	SMLSL2E(7, 12, 7, 11)
	VADD V9.S4, V11.S4, V11.S4     //       + z4
	SMULL2E(5, 13, 1, 15) // tmp1o = r5*c5
	SMLSL2E(3, 13, 2, 15)
	VADD V9.S4, V15.S4, V15.S4     //       + z4
	SMULL2E(3, 13, 3, 1) // tmp2o = r3*c10
	SMLSL2E(5, 13, 2, 1)
	VADD V8.S4, V1.S4, V1.S4       //       + z3

	VADD V11.S4, V4.S4, V24.S4
	VSRSHR $11, V24.S4, V24.S4 // row0 = tmp10+tmp3o
	VADD V1.S4, V0.S4, V25.S4
	VSRSHR $11, V25.S4, V25.S4 // row1 = tmp11+tmp2o
	VADD V15.S4, V6.S4, V26.S4
	VSRSHR $11, V26.S4, V26.S4 // row2 = tmp12+tmp1o
	VADD V10.S4, V2.S4, V27.S4
	VSRSHR $11, V27.S4, V27.S4 // row3 = tmp13+tmp0o
	VSUB V10.S4, V2.S4, V28.S4
	VSRSHR $11, V28.S4, V28.S4 // row4 = tmp13-tmp0o
	VSUB V15.S4, V6.S4, V29.S4
	VSRSHR $11, V29.S4, V29.S4 // row5 = tmp12-tmp1o
	VSUB V1.S4, V0.S4, V30.S4
	VSRSHR $11, V30.S4, V30.S4 // row6 = tmp11-tmp2o
	VSUB V11.S4, V4.S4, V31.S4
	VSRSHR $11, V31.S4, V31.S4 // row7 = tmp10-tmp3o

pass2:
	// ---- Pass 2: rows, 4 per iteration, in 32-bit lanes. ----
	// 32-bit math wraps like int64 in the low 32 bits, and those hold
	// turbo's x&1023 index. Rows 0-3 first: w0-w3 from V16-V19.
	VTRN1 V17.S4, V16.S4, V8.S4
	VTRN2 V17.S4, V16.S4, V9.S4
	VTRN1 V19.S4, V18.S4, V10.S4
	VTRN2 V19.S4, V18.S4, V11.S4
	VTRN1 V10.D2, V8.D2, V0.D2 // w0
	VTRN1 V11.D2, V9.D2, V1.D2 // w1
	VTRN2 V10.D2, V8.D2, V2.D2 // w2
	VTRN2 V11.D2, V9.D2, V3.D2 // w3
	CBZ R10, pass2setup
	VTRN1 V25.S4, V24.S4, V12.S4
	VTRN2 V25.S4, V24.S4, V13.S4
	VTRN1 V27.S4, V26.S4, V14.S4
	VTRN2 V27.S4, V26.S4, V15.S4
	VTRN1 V14.D2, V12.D2, V4.D2 // w4
	VTRN1 V15.D2, V13.D2, V5.D2 // w5
	VTRN2 V14.D2, V12.D2, V6.D2 // w6
	VTRN2 V15.D2, V13.D2, V7.D2 // w7

pass2setup:
	MOVD $16, R8
	VDUP R8, V24.S4 // (w0+16)<<17 adds DESCALE's 1<<17, times 16
	VMOVI $128, V25.B16
	MOVD $idct8P2<>(SB), R21
	VLD1 (R21), [V16.S4, V17.S4, V18.S4]
	ADD $48, R21, R8
	VLD1 (R8), [V26.B16, V27.B16]
	MOVD dst+16(FP), R22
	MOVD stride+24(FP), R23
	MOVD $2, R24

pass2loop:
	CBZ R10, pass2sparse

	// Even part, z1 folded in.
	VADD V24.S4, V0.S4, V0.S4
	MULSE(2, 16, 0, 9)
	MLASE(6, 16, 1, 9)
	MULSE(2, 16, 2, 10)
	MLASE(6, 16, 0, 10)
	VADD V4.S4, V0.S4, V11.S4
	VSUB V4.S4, V0.S4, V8.S4
	VSHL $17, V11.S4, V11.S4     // tmp0, times 16
	VSHL $17, V8.S4, V8.S4       // tmp1
	VADD V10.S4, V11.S4, V12.S4  // tmp10
	VSUB V10.S4, V11.S4, V13.S4  // tmp13
	VADD V9.S4, V8.S4, V14.S4    // tmp11
	VSUB V9.S4, V8.S4, V15.S4    // tmp12

	// Odd part, z5 folded into z3, z4.
	VADD V1.S4, V7.S4, V8.S4     // z1 = w7+w1
	VADD V3.S4, V5.S4, V9.S4     // z2 = w5+w3
	VADD V3.S4, V7.S4, V10.S4    // z3 = w7+w3
	VADD V1.S4, V5.S4, V11.S4    // z4 = w5+w1
	MULSE(8, 18, 0, 8)
	MULSE(9, 18, 1, 9)
	MULSE(10, 18, 2, 0)
	MLASE(11, 16, 3, 0)
	MULSE(11, 18, 3, 2)
	MLASE(10, 16, 3, 2)
	VADD V8.S4, V0.S4, V4.S4
	VADD V9.S4, V2.S4, V6.S4
	VADD V9.S4, V0.S4, V10.S4
	VADD V8.S4, V2.S4, V11.S4
	MLASE(7, 17, 0, 4)           // tmp0o
	MLASE(5, 17, 1, 6)           // tmp1o
	MLASE(3, 17, 2, 10)          // tmp2o
	MLASE(1, 17, 3, 11)          // tmp3o
	B pass2out

pass2sparse:
	// The same with w4-w7 zero: z1 = z4 = w1, z2 = z3 = w3.
	VADD V24.S4, V0.S4, V0.S4
	MULSE(2, 16, 0, 8)           // tmp2
	MULSE(2, 16, 2, 9)           // tmp3
	VSHL $17, V0.S4, V11.S4      // tmp0 = tmp1, times 16
	VADD V9.S4, V11.S4, V12.S4   // tmp10
	VSUB V9.S4, V11.S4, V13.S4   // tmp13
	VADD V8.S4, V11.S4, V14.S4   // tmp11
	VSUB V8.S4, V11.S4, V15.S4   // tmp12

	MULSE(1, 18, 0, 8)
	MULSE(3, 18, 1, 9)
	MULSE(3, 18, 2, 0)
	MLASE(1, 16, 3, 0)
	MULSE(1, 18, 3, 2)
	MLASE(3, 16, 3, 2)
	VADD V8.S4, V0.S4, V4.S4     // tmp0o
	VADD V9.S4, V2.S4, V6.S4     // tmp1o
	VADD V9.S4, V0.S4, V10.S4
	VADD V8.S4, V2.S4, V11.S4
	MLASE(3, 17, 2, 10)          // tmp2o
	MLASE(1, 17, 3, 11)          // tmp3o

pass2out:
	VADD V11.S4, V12.S4, V0.S4   // o0 = tmp10+tmp3o
	VSUB V11.S4, V12.S4, V2.S4   // o7
	VADD V10.S4, V14.S4, V1.S4   // o1 = tmp11+tmp2o
	VSUB V10.S4, V14.S4, V3.S4   // o6
	VADD V6.S4, V15.S4, V8.S4    // o2 = tmp12+tmp1o
	VSUB V6.S4, V15.S4, V9.S4    // o5
	VADD V4.S4, V13.S4, V5.S4    // o3 = tmp13+tmp0o
	VSUB V4.S4, V13.S4, V7.S4    // o4

	// High halfwords: sum>>12 (x16 constants).
	VUZP2 V1.H8, V0.H8, V12.H8   // o0|o1
	VUZP2 V5.H8, V8.H8, V13.H8   // o2|o3
	VUZP2 V9.H8, V7.H8, V14.H8   // o4|o5
	VUZP2 V2.H8, V3.H8, V15.H8   // o6|o7

	// SQSHRN #6 gives sum>>18, clamped.
	// Byte 4j+r is o_j, row r.
	SQSHRN_B(6, 12, 0)
	SQSHRN2_B(6, 13, 0)
	SQSHRN_B(6, 14, 1)
	SQSHRN2_B(6, 15, 1)
	VEOR   V25.B16, V0.B16, V0.B16
	VEOR   V25.B16, V1.B16, V1.B16
	VTBL V26.B16, [V0.B16, V1.B16], V2.B16 // rows 0, 1
	VTBL V27.B16, [V0.B16, V1.B16], V3.B16 // rows 2, 3
	VST1.P V2.D[0], (R22)(R23)
	VST1.P V2.D[1], (R22)(R23)
	VST1.P V3.D[0], (R22)(R23)
	VST1.P V3.D[1], (R22)(R23)

	SUBS $1, R24, R24
	BEQ pass2done

	// Rows 4-7 from V20-V23, V28-V31.
	VTRN1 V21.S4, V20.S4, V8.S4
	VTRN2 V21.S4, V20.S4, V9.S4
	VTRN1 V23.S4, V22.S4, V10.S4
	VTRN2 V23.S4, V22.S4, V11.S4
	VTRN1 V10.D2, V8.D2, V0.D2 // w0
	VTRN1 V11.D2, V9.D2, V1.D2 // w1
	VTRN2 V10.D2, V8.D2, V2.D2 // w2
	VTRN2 V11.D2, V9.D2, V3.D2 // w3
	CBZ R10, pass2loop
	VTRN1 V29.S4, V28.S4, V12.S4
	VTRN2 V29.S4, V28.S4, V13.S4
	VTRN1 V31.S4, V30.S4, V14.S4
	VTRN2 V31.S4, V30.S4, V15.S4
	VTRN1 V14.D2, V12.D2, V4.D2 // w4
	VTRN1 V15.D2, V13.D2, V5.D2 // w5
	VTRN2 V14.D2, V12.D2, V6.D2 // w6
	VTRN2 V15.D2, V13.D2, V7.D2 // w7
	B pass2loop

pass2done:
	RET

// Pass-2 constants for idct4NEONAsm (its c0-c9 times 8, as int32, so
// each sum comes out <<3; two padding slots), then the VTBL index
// gathering row r from byte 4j+r.
GLOBL idct4P2<>(SB), RODATA, $64
DATA idct4P2<>+0(SB)/4, $121096
DATA idct4P2<>+4(SB)/4, $-50160
DATA idct4P2<>+8(SB)/4, $-13840
DATA idct4P2<>+12(SB)/4, $95144
DATA idct4P2<>+16(SB)/4, $-142392
DATA idct4P2<>+20(SB)/4, $69576
DATA idct4P2<>+24(SB)/4, $-33408
DATA idct4P2<>+28(SB)/4, $-39408
DATA idct4P2<>+32(SB)/4, $58984
DATA idct4P2<>+36(SB)/4, $167960
DATA idct4P2<>+40(SB)/4, $0
DATA idct4P2<>+44(SB)/4, $0
DATA idct4P2<>+48(SB)/8, $0x0D0905010C080400
DATA idct4P2<>+56(SB)/8, $0x0F0B07030E0A0602

// func idct4NEONAsm(coef *[64]int16, quant *[64]uint16, dst *byte, stride int)
//
// Ports jsimd_idct_4x4_neon (simd/arm/jidctred-neon.c), T29. Row 4 (the
// input coefficient row idct4Go's column loop never reads) comes in
// with the full-width loads but is never used. Column 4's pass-1
// result would be dead too, so this computes
// all 8 lanes uniformly per half rather than special-casing it: pass 2
// below simply never reads ws slot 4 of each row (idct4Go's row loop
// skips ws[base+4] the same way). Pass 1 (columns) runs in NEON, both
// halves; its 4 rows stay in registers and TRN1/TRN2 transpose them
// as in idct8NEONAsm. Pass 2 (the 4 rows)
// runs in NEON as in idct8NEONAsm, with the same operand-order note.
TEXT ·idct4NEONAsm(SB), $0-32
	MOVD coef+0(FP), R4
	MOVD quant+8(FP), R5

	MOVD $15137, R8
	VDUP R8, V16.H8 // c0 =  fix1_847759065 (row2 -> tmp2)
	MOVD $-6270, R8
	VDUP R8, V17.H8 // c1 = -fix0_765366865 (row6 -> tmp2)
	MOVD $-1730, R8
	VDUP R8, V18.H8 // c2 = -fix0_211164243 (row7 -> oddA)
	MOVD $11893, R8
	VDUP R8, V19.H8 // c3 =  fix1_451774981 (row5 -> oddA)
	MOVD $-17799, R8
	VDUP R8, V20.H8 // c4 = -fix2_172734803 (row3 -> oddA)
	MOVD $8697, R8
	VDUP R8, V21.H8 // c5 =  fix1_061594337 (row1 -> oddA)
	MOVD $-4176, R8
	VDUP R8, V22.H8 // c6 = -fix0_509795579 (row7 -> oddB)
	MOVD $-4926, R8
	VDUP R8, V23.H8 // c7 = -fix0_601344887 (row5 -> oddB)
	MOVD $7373, R8
	VDUP R8, V24.H8 // c8 =  fix0_899976223 (row3 -> oddB)
	MOVD $20995, R8
	VDUP R8, V25.H8 // c9 =  fix2_562915447 (row1 -> oddB)

	// Load all rows full width; row 4 is loaded but never read.
	VLD1.P 64(R4), [V0.H8, V1.H8, V2.H8, V3.H8]
	VLD1 (R4), [V4.H8, V5.H8, V6.H8, V7.H8]
	VLD1.P 64(R5), [V8.H8, V9.H8, V10.H8, V11.H8]
	VLD1 (R5), [V12.H8, V13.H8, V14.H8, V15.H8]
	VMUL V8.H8, V0.H8, V0.H8
	VMUL V9.H8, V1.H8, V1.H8
	VMUL V10.H8, V2.H8, V2.H8
	VMUL V11.H8, V3.H8, V3.H8
	VMUL V13.H8, V5.H8, V5.H8
	VMUL V14.H8, V6.H8, V6.H8
	VMUL V15.H8, V7.H8, V7.H8
	// ---- Left half: columns 0-3, rows in V14 V15 V27 V28. ----
	VSMULL V16.H4, V2.H4, V8.S4   // tmp2 = r2*c0
	VSMLAL V17.H4, V6.H4, V8.S4   //      + r6*c1
	VSSHLL $14, V0.H4, V9.S4      // tmp0 = r0 << (CONST_BITS+1)
	VADD   V8.S4, V9.S4, V10.S4   // tmp10
	VSUB   V8.S4, V9.S4, V11.S4   // tmp12
	VSMULL V18.H4, V7.H4, V12.S4  // oddA = r7*c2
	VSMLAL V19.H4, V5.H4, V12.S4  //      + r5*c3
	VSMLAL V20.H4, V3.H4, V12.S4  //      + r3*c4
	VSMLAL V21.H4, V1.H4, V12.S4  //      + r1*c5
	VSMULL V22.H4, V7.H4, V13.S4  // oddB = r7*c6
	VSMLAL V23.H4, V5.H4, V13.S4  //      + r5*c7
	VSMLAL V24.H4, V3.H4, V13.S4  //      + r3*c8
	VSMLAL V25.H4, V1.H4, V13.S4  //      + r1*c9

	VADD  V13.S4, V10.S4, V14.S4
	VSRSHR $12, V14.S4, V14.S4 // row0 = tmp10+oddB
	VADD  V12.S4, V11.S4, V15.S4
	VSRSHR $12, V15.S4, V15.S4 // row1 = tmp12+oddA
	VSUB  V12.S4, V11.S4, V27.S4
	VSRSHR $12, V27.S4, V27.S4 // row2 = tmp12-oddA
	VSUB  V13.S4, V10.S4, V28.S4
	VSRSHR $12, V28.S4, V28.S4 // row3 = tmp10-oddB

	// ---- Right half: columns 4-7, rows in V8 V9 V11 V10. ----
	VSMULL2 V16.H8, V2.H8, V8.S4   // tmp2 = r2*c0
	VSMLAL2 V17.H8, V6.H8, V8.S4   //      + r6*c1
	VSSHLL2 $14, V0.H8, V9.S4      // tmp0 = r0 << (CONST_BITS+1)
	VADD   V8.S4, V9.S4, V10.S4   // tmp10
	VSUB   V8.S4, V9.S4, V11.S4   // tmp12
	VSMULL2 V18.H8, V7.H8, V12.S4  // oddA = r7*c2
	VSMLAL2 V19.H8, V5.H8, V12.S4  //      + r5*c3
	VSMLAL2 V20.H8, V3.H8, V12.S4  //      + r3*c4
	VSMLAL2 V21.H8, V1.H8, V12.S4  //      + r1*c5
	VSMULL2 V22.H8, V7.H8, V13.S4  // oddB = r7*c6
	VSMLAL2 V23.H8, V5.H8, V13.S4  //      + r5*c7
	VSMLAL2 V24.H8, V3.H8, V13.S4  //      + r3*c8
	VSMLAL2 V25.H8, V1.H8, V13.S4  //      + r1*c9

	VADD  V13.S4, V10.S4, V8.S4
	VSRSHR $12, V8.S4, V8.S4   // row0
	VADD  V12.S4, V11.S4, V9.S4
	VSRSHR $12, V9.S4, V9.S4   // row1
	VSUB  V12.S4, V11.S4, V11.S4
	VSRSHR $12, V11.S4, V11.S4 // row2
	VSUB  V13.S4, V10.S4, V10.S4
	VSRSHR $12, V10.S4, V10.S4 // row3

	// ---- Transpose 4x4 blocks in registers: Vk = column k, 4 rows. ----
	VTRN1 V15.S4, V14.S4, V12.S4
	VTRN2 V15.S4, V14.S4, V13.S4
	VTRN1 V28.S4, V27.S4, V29.S4
	VTRN2 V28.S4, V27.S4, V30.S4
	VTRN1 V29.D2, V12.D2, V0.D2 // w0
	VTRN2 V29.D2, V12.D2, V2.D2 // w2
	VTRN1 V30.D2, V13.D2, V1.D2 // w1
	VTRN2 V30.D2, V13.D2, V3.D2 // w3
	VTRN1 V9.S4, V8.S4, V12.S4
	VTRN2 V9.S4, V8.S4, V13.S4
	VTRN1 V10.S4, V11.S4, V29.S4
	VTRN2 V10.S4, V11.S4, V30.S4
	VTRN2 V29.D2, V12.D2, V6.D2 // w6
	VTRN1 V30.D2, V13.D2, V5.D2 // w5
	VTRN2 V30.D2, V13.D2, V7.D2 // w7

	// ---- Pass 2: the 4 rows at once, in 32-bit lanes. ----
	// Bits 19-28 of each sum are x&1023; VTBL turns columns into rows.
	MOVD $16, R8
	VDUP R8, V28.S4 // (w0+16)<<17 adds DESCALE's 1<<18, times 8
	VMOVI $128, V31.B16
	MOVD $idct4P2<>(SB), R8
	VLD4R.P 16(R8), [V16.S4, V17.S4, V18.S4, V19.S4]
	VLD4R.P 16(R8), [V20.S4, V21.S4, V22.S4, V23.S4]
	VLD4R.P 16(R8), [V24.S4, V25.S4, V26.S4, V27.S4]
	VLD1 (R8), [V29.B16]

	VADD V28.S4, V0.S4, V0.S4
	VSHL $17, V0.S4, V0.S4       // tmp0, times 8
	VMUL V16.S4, V2.S4, V8.S4    // tmp2 = w2*c0
	VMLA V17.S4, V6.S4, V8.S4    //      + w6*c1
	VADD V8.S4, V0.S4, V9.S4     // tmp10
	VSUB V8.S4, V0.S4, V10.S4    // tmp12
	VMUL V18.S4, V7.S4, V11.S4   // oddA = w7*c2
	VMLA V19.S4, V5.S4, V11.S4   //      + w5*c3
	VMLA V20.S4, V3.S4, V11.S4   //      + w3*c4
	VMLA V21.S4, V1.S4, V11.S4   //      + w1*c5
	VMUL V22.S4, V7.S4, V12.S4   // oddB = w7*c6
	VMLA V23.S4, V5.S4, V12.S4   //      + w5*c7
	VMLA V24.S4, V3.S4, V12.S4   //      + w3*c8
	VMLA V25.S4, V1.S4, V12.S4   //      + w1*c9
	VADD V12.S4, V9.S4, V13.S4   // out0 = tmp10+oddB
	VSUB V12.S4, V9.S4, V14.S4   // out3
	VADD V11.S4, V10.S4, V15.S4  // out1 = tmp12+oddA
	VSUB V11.S4, V10.S4, V0.S4   // out2

	VUZP2 V15.H8, V13.H8, V1.H8 // out0|out1, bits 16-31
	VUZP2 V14.H8, V0.H8, V2.H8  // out2|out3
	VSQSHL $2, V1.H8, V1.H8 // top 10 bits (19-28 of the sum), clamped
	VSQSHL $2, V2.H8, V2.H8
	VUZP2  V2.B16, V1.B16, V3.B16 // byte 4j+r = out j, row r
	VEOR   V31.B16, V3.B16, V3.B16
	VTBL V29.B16, [V3.B16], V4.B16

	MOVD dst+16(FP), R22
	MOVD stride+24(FP), R23
	VST1.P V4.S[0], (R22)(R23)
	VST1.P V4.S[1], (R22)(R23)
	VST1.P V4.S[2], (R22)(R23)
	VST1 V4.S[3], (R22)
	RET

// func colorRowNEONAsm(y, cb, cr, dst *byte, n int)
//
// colorRowGo in NEON, 16 pixels per iteration, in 16-bit lanes. Its
// constants over int16 split off a multiple of 65536, which passes
// through the >>16 exactly: (91881x+32768)>>16 = x+((26345x+32768)>>16),
// (116130x+32768)>>16 = 2x+((32768-14942x)>>16), and G's sum
// -22554xb-46802xr+32768 = -65536xr+(-22554xb+18734xr+32768). So G's
// products are VSMLALs of int16s onto a VZIP-built accumulator whose
// high halfword is the integer part, VUZP2 takes the >>16 halves of
// both result vectors at once, the sums before the clamp stay
// within -285..538, and VSQXTUN is the 0..255 clamp. R, G, B and
// constant alpha interleave with two levels of VZIP and go out in one
// 64-byte VST1 (measured faster here than jpegn's VST4). n must be a
// positive multiple of 16; colorRowNEON runs any remainder through
// colorRowGo.
TEXT ·colorRowNEONAsm(SB), $0-40
	MOVD y+0(FP), R0
	MOVD cb+8(FP), R1
	MOVD cr+16(FP), R2
	MOVD dst+24(FP), R3
	MOVD n+32(FP), R4

	VMOVI $128, V20.B16 // x^0x80 as int8 is x-128
	MOVD $13172, R8
	VDUP R8, V21.H8 // half of 26344, for SQRDMULH
	MOVD $-7471, R8
	VDUP R8, V22.H8 // half of -14942, for SQRDMULH
	MOVD $-22554, R8
	VDUP R8, V23.H8
	MOVD $18734, R8
	VDUP R8, V25.H8
	MOVD $32768, R8
	VDUP R8, V24.H8 // yccOneHalf, as the low halfword
	VMOVI $255, V3.B16 // alpha

colorloop:
	VLD1.P 16(R0), [V4.B16]
	VLD1.P 16(R1), [V5.B16]
	VLD1.P 16(R2), [V6.B16]
	VEOR   V20.B16, V5.B16, V5.B16
	VEOR   V20.B16, V6.B16, V6.B16
	VSXTL  V5.B8, V7.H8    // xb = cb-128, pixels 0-7
	VSXTL  V6.B8, V8.H8    // xr
	VUXTL  V4.B8, V9.H8    // y
	VSXTL2 V5.B16, V14.H8  // pixels 8-15
	VSXTL2 V6.B16, V15.H8
	VUXTL2 V4.B16, V12.H8

	// Each channel is k + (c*x + 32768)>>16 with k an integer, so
	// VZIP puts k in the high and 0x8000 in the low halfword of each
	// 32-bit lane, VSMLAL adds c*x, and VUZP2's high halves are done.
	// R = y+xr+(26345*xr+32768)>>16 and B = y+2*xb+(32768-14942*xb)>>16
	// by SQRDMULH ((2*a*b+32768)>>16, b = 13172 and -7471; Go's
	// assembler has no mnemonic for it). 26344 rounds as 26345 does
	// for every xr in -128..127 (TestColorRConst);
	// G = y-xr+(-22554*xb+18734*xr+32768)>>16.
	VADD    V8.H8, V9.H8, V13.H8
	WORD    $0x6e75b50a // SQRDMULH V10.8H, V8.8H, V21.8H
	VADD    V10.H8, V13.H8, V13.H8
	VSQXTUN V13.H8, V0.B8
	VADD    V7.H8, V9.H8, V13.H8
	VADD    V7.H8, V13.H8, V13.H8
	WORD    $0x6e76b4ea // SQRDMULH V10.8H, V7.8H, V22.8H
	VADD    V10.H8, V13.H8, V13.H8
	VSQXTUN V13.H8, V2.B8
	VSUB    V8.H8, V9.H8, V13.H8
	VZIP1   V13.H8, V24.H8, V10.H8
	VZIP2   V13.H8, V24.H8, V11.H8
	VSMLAL  V23.H4, V7.H4, V10.S4
	VSMLAL2 V23.H8, V7.H8, V11.S4
	VSMLAL  V25.H4, V8.H4, V10.S4
	VSMLAL2 V25.H8, V8.H8, V11.S4
	VUZP2   V11.H8, V10.H8, V13.H8
	VSQXTUN V13.H8, V1.B8
	VADD    V15.H8, V12.H8, V16.H8
	WORD    $0x6e75b5f1 // SQRDMULH V17.8H, V15.8H, V21.8H
	VADD    V17.H8, V16.H8, V16.H8
	VSQXTUN2 V16.H8, V0.B16
	VADD    V14.H8, V12.H8, V16.H8
	VADD    V14.H8, V16.H8, V16.H8
	WORD    $0x6e76b5d1 // SQRDMULH V17.8H, V14.8H, V22.8H
	VADD    V17.H8, V16.H8, V16.H8
	VSQXTUN2 V16.H8, V2.B16
	VSUB    V15.H8, V12.H8, V16.H8
	VZIP1   V16.H8, V24.H8, V17.H8
	VZIP2   V16.H8, V24.H8, V18.H8
	VSMLAL  V23.H4, V14.H4, V17.S4
	VSMLAL2 V23.H8, V14.H8, V18.S4
	VSMLAL  V25.H4, V15.H4, V17.S4
	VSMLAL2 V25.H8, V15.H8, V18.S4
	VUZP2   V18.H8, V17.H8, V16.H8
	VSQXTUN2 V16.H8, V1.B16

	VZIP1  V1.B16, V0.B16, V26.B16 // R0 G0 R1 G1 ... pixels 0-7
	VZIP2  V1.B16, V0.B16, V27.B16 // pixels 8-15
	VZIP1  V3.B16, V2.B16, V28.B16 // B0 A0 B1 A1 ...
	VZIP2  V3.B16, V2.B16, V29.B16
	VZIP1  V28.H8, V26.H8, V4.H8  // pixels 0-3
	VZIP2  V28.H8, V26.H8, V5.H8  // pixels 4-7
	VZIP1  V29.H8, V27.H8, V6.H8  // pixels 8-11
	VZIP2  V29.H8, V27.H8, V7.H8  // pixels 12-15
	VST1.P [V4.B16, V5.B16, V6.B16, V7.B16], 64(R3)

	SUBS $16, R4, R4
	BNE  colorloop

	RET

// func h2v1FancyRowNEONAsm(a, b, c, dst *byte, n int)
//
// h2v1FancyRow's interior-column loop (jdsample.c h2v1_fancy_upsample,
// :295-325) in NEON, 16 columns (a[i]=row[c-1], b[i]=row[c],
// c[i]=row[c+1]) per iteration, entirely in byte lanes with halving
// adds: (3b+a+1)>>2 = UHADD(URHADD(a,b), b) and (3b+c+2)>>2 =
// URHADD(UHADD(c,b), b), both checked for every byte pair. VST2
// interleaves the left and right outputs as it stores them. n must be
// a positive multiple of 16.
TEXT ·h2v1FancyRowNEONAsm(SB), $0-40
	MOVD a+0(FP), R0
	MOVD b+8(FP), R1
	MOVD c+16(FP), R2
	MOVD dst+24(FP), R3
	MOVD n+32(FP), R4

h2v1loop:
	VLD1.P 16(R0), [V0.B16]
	VLD1.P 16(R1), [V1.B16]
	VLD1.P 16(R2), [V2.B16]

	VURHADD V1.B16, V0.B16, V3.B16 // (a+b+1)>>1
	VUHADD  V1.B16, V3.B16, V3.B16 // left = (3b+a+1)>>2
	VUHADD  V1.B16, V2.B16, V4.B16 // (c+b)>>1
	VURHADD V1.B16, V4.B16, V4.B16 // right = (3b+c+2)>>2

	VST2.P [V3.B16, V4.B16], 32(R3)

	SUBS $16, R4, R4
	BNE  h2v1loop

	RET

// func h2v2FancyRowNEONAsm(curA, curB, curC, otherA, otherB, otherC, dst *byte, n int)
//
// h2v2FancyRow's interior-column loop (jdsample.c h2v2_fancy_upsample,
// :381-429) in NEON, matching h2v2FancyRowGo's colsum/blend formula per
// column (9:3:3:1 weighted blend, truncating >>4): colsum(k) =
// rowCur[k]*3+rowOther[k] <= 1020, and 3*1020+1020+8 fits 16 bits.
// 16 columns per iteration, stored interleaved by VST2 like h2v1's
// kernel. n must be a multiple of 16.
TEXT ·h2v2FancyRowNEONAsm(SB), $0-64
	MOVD curA+0(FP), R0
	MOVD curB+8(FP), R1
	MOVD curC+16(FP), R2
	MOVD otherA+24(FP), R5
	MOVD otherB+32(FP), R6
	MOVD otherC+40(FP), R7
	MOVD dst+48(FP), R3
	MOVD n+56(FP), R4

	MOVD $3, R8
	VDUP R8, V16.B16 // three (bytes, for the widening multiply)
	VDUP R8, V19.H8  // three (halfwords, for colsumB*3)
	MOVD $8, R8
	VDUP R8, V17.H8 // eight
	MOVD $7, R8
	VDUP R8, V18.H8 // seven

h2v2loop:
	CBZ R4, h2v2done

	VLD1.P 16(R0), [V0.B16]
	VLD1.P 16(R1), [V1.B16]
	VLD1.P 16(R2), [V2.B16]
	VLD1.P 16(R5), [V3.B16]
	VLD1.P 16(R6), [V4.B16]
	VLD1.P 16(R7), [V5.B16]

	VUADDW  V3.B8, V17.H8, V20.H8 // 8+otherA
	VUADDW2 V3.B16, V17.H8, V21.H8
	VUMLAL  V16.B8, V0.B8, V20.H8 // colsumA+8
	VUMLAL2 V16.B16, V0.B16, V21.H8
	VUMULL  V16.B8, V1.B8, V22.H8 // colsumB
	VUMULL2 V16.B16, V1.B16, V23.H8
	VUADDW  V4.B8, V22.H8, V22.H8
	VUADDW2 V4.B16, V23.H8, V23.H8
	VUADDW  V5.B8, V18.H8, V24.H8 // 7+otherC
	VUADDW2 V5.B16, V18.H8, V25.H8
	VUMLAL  V16.B8, V2.B8, V24.H8 // colsumC+7
	VUMLAL2 V16.B16, V2.B16, V25.H8

	VMLA V19.H8, V22.H8, V20.H8 // left_sum = colsumB*3+colsumA+8
	VMLA V19.H8, V23.H8, V21.H8
	VMLA V19.H8, V22.H8, V24.H8 // right_sum = colsumB*3+colsumC+7
	VMLA V19.H8, V23.H8, V25.H8

	VSHRN  $4, V20.H8, V26.B8
	VSHRN2 $4, V21.H8, V26.B16 // left
	VSHRN  $4, V24.H8, V27.B8
	VSHRN2 $4, V25.H8, V27.B16 // right

	VST2.P [V26.B16, V27.B16], 32(R3)

	SUB $16, R4, R4
	B   h2v2loop

h2v2done:
	RET

#define SQRDMLAH_H(m, n, d) WORD $(0x6E408400 | ((m)<<16) | ((n)<<5) | (d))
#define SSRA8_H(n, d) WORD $(0x4F181400 | ((n)<<5) | (d))
#define SRSRA8_H(n, d) WORD $(0x4F183400 | ((n)<<5) | (d))

// func colorRowNEONRDMAsm(y, cb, cr, dst *byte, n int)
//
// n%16 == 0, n > 0; never reads past n.
TEXT ·colorRowNEONRDMAsm(SB), $0-40
	MOVD y+0(FP), R0
	MOVD cb+8(FP), R1
	MOVD cr+16(FP), R2
	MOVD dst+24(FP), R3
	MOVD n+32(FP), R4

	VMOVI $128, V20.B16
	MOVD $13172, R8
	VDUP R8, V21.H8
	MOVD $-7471, R8
	VDUP R8, V22.H8
	MOVD $-88, R8
	VDUP R8, V23.B16
	MOVD $73, R8
	VDUP R8, V24.B16
	MOVD $-26, R8
	VDUP R8, V25.B16
	MOVD $46, R8
	VDUP R8, V30.B16
	VMOVI $255, V3.B16
	MOVD $16, R10

	VLD1 (R0), [V4.B16]
	VLD1 (R1), [V5.B16]
	VLD1 (R2), [V6.B16]

colorrdmloop:
	VEOR   V20.B16, V5.B16, V7.B16 // u
	VEOR   V20.B16, V6.B16, V8.B16 // v
	VUXTL  V4.B8, V9.H8
	VUXTL2 V4.B16, V12.H8
	SUBS   $16, R4, R4
	CSEL   NE, R10, ZR, R11
	ADD    R11, R0
	ADD    R11, R1
	ADD    R11, R2
	VLD1   (R0), [V4.B16]
	VLD1   (R1), [V5.B16]
	VLD1   (R2), [V6.B16]
	VSXTL  V7.B8, V13.H8  // xb lo
	VSXTL  V8.B8, V15.H8  // xr lo
	VSXTL2 V7.B16, V14.H8 // xb hi
	VSXTL2 V8.B16, V16.H8 // xr hi

	VSMULL  V23.B8, V7.B8, V10.H8
	VSMLAL  V24.B8, V8.B8, V10.H8
	VSMULL2 V23.B16, V7.B16, V11.H8
	VSMLAL2 V24.B16, V8.B16, V11.H8
	VSMULL  V25.B8, V7.B8, V18.H8
	VSMLAL  V30.B8, V8.B8, V18.H8
	VSMULL2 V25.B16, V7.B16, V19.H8
	VSMLAL2 V30.B16, V8.B16, V19.H8

	VADD    V15.H8, V9.H8, V17.H8
	SQRDMLAH_H(21, 15, 17)
	VSQXTUN V17.H8, V0.B8
	VADD    V13.H8, V9.H8, V27.H8
	VADD    V13.H8, V27.H8, V27.H8
	SQRDMLAH_H(22, 13, 27)
	VSQXTUN V27.H8, V2.B8
	VADD    V16.H8, V12.H8, V26.H8
	SQRDMLAH_H(21, 16, 26)
	VSQXTUN2 V26.H8, V0.B16
	VADD    V14.H8, V12.H8, V28.H8
	VADD    V14.H8, V28.H8, V28.H8
	SQRDMLAH_H(22, 14, 28)
	VSQXTUN2 V28.H8, V2.B16

	SSRA8_H(18, 10)
	SSRA8_H(19, 11)
	VSUB    V15.H8, V9.H8, V29.H8
	VSUB    V16.H8, V12.H8, V31.H8
	SRSRA8_H(10, 29)
	SRSRA8_H(11, 31)
	VSQXTUN  V29.H8, V1.B8
	VSQXTUN2 V31.H8, V1.B16

	VZIP1  V1.B16, V0.B16, V9.B16
	VZIP2  V1.B16, V0.B16, V17.B16
	VZIP1  V3.B16, V2.B16, V26.B16
	VZIP2  V3.B16, V2.B16, V27.B16
	VZIP1  V26.H8, V9.H8, V12.H8
	VZIP2  V26.H8, V9.H8, V13.H8
	VZIP1  V27.H8, V17.H8, V14.H8
	VZIP2  V27.H8, V17.H8, V15.H8
	VST1.P [V12.B16, V13.B16, V14.B16, V15.B16], 64(R3)

	BNE colorrdmloop

	RET
