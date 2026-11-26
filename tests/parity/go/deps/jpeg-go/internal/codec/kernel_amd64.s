//go:build !purego

#include "textflag.h"

// func cpuid(eaxArg, ecxArg uint32) (eax, ebx, ecx, edx uint32)
// Copied from jp2k-go/internal/dsp/dwt_amd64.go:16-30 (dwt_amd64.s).
TEXT ·cpuid(SB), NOSPLIT, $0-24
	MOVL eaxArg+0(FP), AX
	MOVL ecxArg+4(FP), CX
	CPUID
	MOVL AX, eax+8(FP)
	MOVL BX, ebx+12(FP)
	MOVL CX, ecx+16(FP)
	MOVL DX, edx+20(FP)
	RET

// func xgetbv() (eax, edx uint32)
TEXT ·xgetbv(SB), NOSPLIT, $0-8
	MOVL $0, CX
	XGETBV
	MOVL AX, eax+0(FP)
	MOVL DX, edx+4(FP)
	RET

// Pass-2 (row) constants, each replicated 8x so VPMULLD/VPADDD can
// read them straight from memory as a second vector operand. Values
// are dct.go's FIX_* (jidctint.c:118-129) times 16, so pass 2's sums
// come out <<4 (bits 18-27 at the top), pre-negated where dct.go always
// uses the negated form. BIAS1 is pass 1's rounding.
#define C541    70928
#define NC1847  -242192
#define C765    100320
#define C1175   154128
#define C298    39136
#define C2053   269104
#define C3072   402752
#define C1501   196784
#define NC899   -117968
#define NC2562  -335920
#define NC1961  -257104
#define NC390   -51136
#define BIAS1   1024

GLOBL avx2Fix<>(SB), RODATA, $416
DATA avx2Fix<>+0(SB)/4, $C541
DATA avx2Fix<>+4(SB)/4, $C541
DATA avx2Fix<>+8(SB)/4, $C541
DATA avx2Fix<>+12(SB)/4, $C541
DATA avx2Fix<>+16(SB)/4, $C541
DATA avx2Fix<>+20(SB)/4, $C541
DATA avx2Fix<>+24(SB)/4, $C541
DATA avx2Fix<>+28(SB)/4, $C541
DATA avx2Fix<>+32(SB)/4, $NC1847
DATA avx2Fix<>+36(SB)/4, $NC1847
DATA avx2Fix<>+40(SB)/4, $NC1847
DATA avx2Fix<>+44(SB)/4, $NC1847
DATA avx2Fix<>+48(SB)/4, $NC1847
DATA avx2Fix<>+52(SB)/4, $NC1847
DATA avx2Fix<>+56(SB)/4, $NC1847
DATA avx2Fix<>+60(SB)/4, $NC1847
DATA avx2Fix<>+64(SB)/4, $C765
DATA avx2Fix<>+68(SB)/4, $C765
DATA avx2Fix<>+72(SB)/4, $C765
DATA avx2Fix<>+76(SB)/4, $C765
DATA avx2Fix<>+80(SB)/4, $C765
DATA avx2Fix<>+84(SB)/4, $C765
DATA avx2Fix<>+88(SB)/4, $C765
DATA avx2Fix<>+92(SB)/4, $C765
DATA avx2Fix<>+96(SB)/4, $C1175
DATA avx2Fix<>+100(SB)/4, $C1175
DATA avx2Fix<>+104(SB)/4, $C1175
DATA avx2Fix<>+108(SB)/4, $C1175
DATA avx2Fix<>+112(SB)/4, $C1175
DATA avx2Fix<>+116(SB)/4, $C1175
DATA avx2Fix<>+120(SB)/4, $C1175
DATA avx2Fix<>+124(SB)/4, $C1175
DATA avx2Fix<>+128(SB)/4, $C298
DATA avx2Fix<>+132(SB)/4, $C298
DATA avx2Fix<>+136(SB)/4, $C298
DATA avx2Fix<>+140(SB)/4, $C298
DATA avx2Fix<>+144(SB)/4, $C298
DATA avx2Fix<>+148(SB)/4, $C298
DATA avx2Fix<>+152(SB)/4, $C298
DATA avx2Fix<>+156(SB)/4, $C298
DATA avx2Fix<>+160(SB)/4, $C2053
DATA avx2Fix<>+164(SB)/4, $C2053
DATA avx2Fix<>+168(SB)/4, $C2053
DATA avx2Fix<>+172(SB)/4, $C2053
DATA avx2Fix<>+176(SB)/4, $C2053
DATA avx2Fix<>+180(SB)/4, $C2053
DATA avx2Fix<>+184(SB)/4, $C2053
DATA avx2Fix<>+188(SB)/4, $C2053
DATA avx2Fix<>+192(SB)/4, $C3072
DATA avx2Fix<>+196(SB)/4, $C3072
DATA avx2Fix<>+200(SB)/4, $C3072
DATA avx2Fix<>+204(SB)/4, $C3072
DATA avx2Fix<>+208(SB)/4, $C3072
DATA avx2Fix<>+212(SB)/4, $C3072
DATA avx2Fix<>+216(SB)/4, $C3072
DATA avx2Fix<>+220(SB)/4, $C3072
DATA avx2Fix<>+224(SB)/4, $C1501
DATA avx2Fix<>+228(SB)/4, $C1501
DATA avx2Fix<>+232(SB)/4, $C1501
DATA avx2Fix<>+236(SB)/4, $C1501
DATA avx2Fix<>+240(SB)/4, $C1501
DATA avx2Fix<>+244(SB)/4, $C1501
DATA avx2Fix<>+248(SB)/4, $C1501
DATA avx2Fix<>+252(SB)/4, $C1501
DATA avx2Fix<>+256(SB)/4, $NC899
DATA avx2Fix<>+260(SB)/4, $NC899
DATA avx2Fix<>+264(SB)/4, $NC899
DATA avx2Fix<>+268(SB)/4, $NC899
DATA avx2Fix<>+272(SB)/4, $NC899
DATA avx2Fix<>+276(SB)/4, $NC899
DATA avx2Fix<>+280(SB)/4, $NC899
DATA avx2Fix<>+284(SB)/4, $NC899
DATA avx2Fix<>+288(SB)/4, $NC2562
DATA avx2Fix<>+292(SB)/4, $NC2562
DATA avx2Fix<>+296(SB)/4, $NC2562
DATA avx2Fix<>+300(SB)/4, $NC2562
DATA avx2Fix<>+304(SB)/4, $NC2562
DATA avx2Fix<>+308(SB)/4, $NC2562
DATA avx2Fix<>+312(SB)/4, $NC2562
DATA avx2Fix<>+316(SB)/4, $NC2562
DATA avx2Fix<>+320(SB)/4, $NC1961
DATA avx2Fix<>+324(SB)/4, $NC1961
DATA avx2Fix<>+328(SB)/4, $NC1961
DATA avx2Fix<>+332(SB)/4, $NC1961
DATA avx2Fix<>+336(SB)/4, $NC1961
DATA avx2Fix<>+340(SB)/4, $NC1961
DATA avx2Fix<>+344(SB)/4, $NC1961
DATA avx2Fix<>+348(SB)/4, $NC1961
DATA avx2Fix<>+352(SB)/4, $NC390
DATA avx2Fix<>+356(SB)/4, $NC390
DATA avx2Fix<>+360(SB)/4, $NC390
DATA avx2Fix<>+364(SB)/4, $NC390
DATA avx2Fix<>+368(SB)/4, $NC390
DATA avx2Fix<>+372(SB)/4, $NC390
DATA avx2Fix<>+376(SB)/4, $NC390
DATA avx2Fix<>+380(SB)/4, $NC390
DATA avx2Fix<>+384(SB)/4, $BIAS1
DATA avx2Fix<>+388(SB)/4, $BIAS1
DATA avx2Fix<>+392(SB)/4, $BIAS1
DATA avx2Fix<>+396(SB)/4, $BIAS1
DATA avx2Fix<>+400(SB)/4, $BIAS1
DATA avx2Fix<>+404(SB)/4, $BIAS1
DATA avx2Fix<>+408(SB)/4, $BIAS1
DATA avx2Fix<>+412(SB)/4, $BIAS1

#define fC541   avx2Fix<>+0(SB)
#define fNC1847 avx2Fix<>+32(SB)
#define fC765   avx2Fix<>+64(SB)
#define fC1175  avx2Fix<>+96(SB)
#define fC298   avx2Fix<>+128(SB)
#define fC2053  avx2Fix<>+160(SB)
#define fC3072  avx2Fix<>+192(SB)
#define fC1501  avx2Fix<>+224(SB)
#define fNC899  avx2Fix<>+256(SB)
#define fNC2562 avx2Fix<>+288(SB)
#define fNC1961 avx2Fix<>+320(SB)
#define fNC390  avx2Fix<>+352(SB)
#define fBIAS1  avx2Fix<>+384(SB)

// Pass-2 extras: 0x80 bytes (sign flip) and the VPSHUFB index
// gathering row r from byte 4j+r in each 128-bit lane.
GLOBL idct8P2x<>(SB), RODATA, $64
DATA idct8P2x<>+0(SB)/8, $0x8080808080808080
DATA idct8P2x<>+8(SB)/8, $0x8080808080808080
DATA idct8P2x<>+16(SB)/8, $0x8080808080808080
DATA idct8P2x<>+24(SB)/8, $0x8080808080808080
DATA idct8P2x<>+32(SB)/8, $0x0D0905010C080400
DATA idct8P2x<>+40(SB)/8, $0x0F0B07030E0A0602
DATA idct8P2x<>+48(SB)/8, $0x0D0905010C080400
DATA idct8P2x<>+56(SB)/8, $0x0F0B07030E0A0602

#define fX80   idct8P2x<>+0(SB)
#define fTRANS idct8P2x<>+32(SB)

// Pass-2 constants for idct4AVX2Asm, 32 bytes each, low half for one
// column of a pair and high half for the other, all times 8 so each
// sum comes out <<3: K26 C1847|NC765, KA15
// C1061|C1451, KA37 NC2172|NC211, KB15 C2562|NC601, KB37 C899|NC509,
// KSign 1|-1, then 16s and the VPSHUFB index for out0,out1,out3,out2.
GLOBL idct4P2x<>(SB), RODATA, $240
DATA idct4P2x<>+0(SB)/8, $0x0001D9080001D908
DATA idct4P2x<>+8(SB)/8, $0x0001D9080001D908
DATA idct4P2x<>+16(SB)/8, $0xFFFF3C10FFFF3C10
DATA idct4P2x<>+24(SB)/8, $0xFFFF3C10FFFF3C10
DATA idct4P2x<>+32(SB)/8, $0x00010FC800010FC8
DATA idct4P2x<>+40(SB)/8, $0x00010FC800010FC8
DATA idct4P2x<>+48(SB)/8, $0x000173A8000173A8
DATA idct4P2x<>+56(SB)/8, $0x000173A8000173A8
DATA idct4P2x<>+64(SB)/8, $0xFFFDD3C8FFFDD3C8
DATA idct4P2x<>+72(SB)/8, $0xFFFDD3C8FFFDD3C8
DATA idct4P2x<>+80(SB)/8, $0xFFFFC9F0FFFFC9F0
DATA idct4P2x<>+88(SB)/8, $0xFFFFC9F0FFFFC9F0
DATA idct4P2x<>+96(SB)/8, $0x0002901800029018
DATA idct4P2x<>+104(SB)/8, $0x0002901800029018
DATA idct4P2x<>+112(SB)/8, $0xFFFF6610FFFF6610
DATA idct4P2x<>+120(SB)/8, $0xFFFF6610FFFF6610
DATA idct4P2x<>+128(SB)/8, $0x0000E6680000E668
DATA idct4P2x<>+136(SB)/8, $0x0000E6680000E668
DATA idct4P2x<>+144(SB)/8, $0xFFFF7D80FFFF7D80
DATA idct4P2x<>+152(SB)/8, $0xFFFF7D80FFFF7D80
DATA idct4P2x<>+160(SB)/8, $0x0000000100000001
DATA idct4P2x<>+168(SB)/8, $0x0000000100000001
DATA idct4P2x<>+176(SB)/8, $0xFFFFFFFFFFFFFFFF
DATA idct4P2x<>+184(SB)/8, $0xFFFFFFFFFFFFFFFF
DATA idct4P2x<>+192(SB)/8, $0x0000001000000010
DATA idct4P2x<>+200(SB)/8, $0x0000001000000010
DATA idct4P2x<>+208(SB)/8, $0x0000001000000010
DATA idct4P2x<>+216(SB)/8, $0x0000001000000010
DATA idct4P2x<>+224(SB)/8, $0x090D0501080C0400
DATA idct4P2x<>+232(SB)/8, $0x0B0F07030A0E0602

#define fK26     idct4P2x<>+0(SB)
#define fKA15    idct4P2x<>+32(SB)
#define fKA37    idct4P2x<>+64(SB)
#define fKB15    idct4P2x<>+96(SB)
#define fKB37    idct4P2x<>+128(SB)
#define fKSign   idct4P2x<>+160(SB)
#define fR16     idct4P2x<>+192(SB)
#define fR4TRANS idct4P2x<>+224(SB)

// IDCT8AVX2 is idct8Go's pass-2 row IDCT across 8 lanes, every sum
// times 16: inputs Y0-Y7 (coefficient k in Yk), bias added to w0.
// Leaves tmp10=Y8 tmp13=Y9 tmp11=Y10 tmp12=Y11 and the odd sums
// tmp0-3 in Y12-Y15; clobbers Y0-Y7.
#define IDCT8AVX2(bias) \
	VPADDD  Y6, Y2, Y14; \
	VPMULLD fC541, Y14, Y14; \
	VPMULLD fNC1847, Y6, Y12; \
	VPADDD  Y12, Y14, Y12; \
	VPMULLD fC765, Y2, Y13; \
	VPADDD  Y13, Y14, Y13; \
	VPADDD  bias, Y0, Y0; \
	VPADDD  Y4, Y0, Y14; \
	VPSUBD  Y4, Y0, Y15; \
	VPSLLD  $17, Y14, Y14; \
	VPSLLD  $17, Y15, Y15; \
	VPADDD  Y13, Y14, Y8; \
	VPSUBD  Y13, Y14, Y9; \
	VPADDD  Y12, Y15, Y10; \
	VPSUBD  Y12, Y15, Y11; \
	VPADDD  Y1, Y7, Y0; \
	VPADDD  Y3, Y5, Y2; \
	VPADDD  Y3, Y7, Y4; \
	VPADDD  Y1, Y5, Y6; \
	VPMULLD fC298, Y7, Y12; \
	VPMULLD fC2053, Y5, Y13; \
	VPMULLD fC3072, Y3, Y14; \
	VPMULLD fC1501, Y1, Y15; \
	VPMULLD fNC899, Y0, Y0; \
	VPMULLD fNC2562, Y2, Y2; \
	VPADDD  Y6, Y4, Y1; \
	VPMULLD fC1175, Y1, Y1; \
	VPMULLD fNC1961, Y4, Y4; \
	VPMULLD fNC390, Y6, Y6; \
	VPADDD  Y1, Y4, Y4; \
	VPADDD  Y1, Y6, Y6; \
	VPADDD  Y0, Y12, Y12; \
	VPADDD  Y4, Y12, Y12; \
	VPADDD  Y2, Y13, Y13; \
	VPADDD  Y6, Y13, Y13; \
	VPADDD  Y2, Y14, Y14; \
	VPADDD  Y4, Y14, Y14; \
	VPADDD  Y0, Y15, Y15; \
	VPADDD  Y6, Y15, Y15

// Pass-1 word-pair constants for idct8AVX2Asm's VPMADDWD, each 8x.
// Pairs are (row0,row4) and (row2,row6) for the even part, then
// (row1,row5) and (row3,row7) for tmp0-tmp3 of the odd part: the
// jidctint.c odd part multiplied out per coefficient, so no 16-bit sum
// can wrap.
GLOBL idct8P1x<>(SB), RODATA, $384
DATA idct8P1x<>+0(SB)/8, $0x2000200020002000
DATA idct8P1x<>+8(SB)/8, $0x2000200020002000
DATA idct8P1x<>+16(SB)/8, $0x2000200020002000
DATA idct8P1x<>+24(SB)/8, $0x2000200020002000
DATA idct8P1x<>+32(SB)/8, $0xE0002000E0002000
DATA idct8P1x<>+40(SB)/8, $0xE0002000E0002000
DATA idct8P1x<>+48(SB)/8, $0xE0002000E0002000
DATA idct8P1x<>+56(SB)/8, $0xE0002000E0002000
DATA idct8P1x<>+64(SB)/8, $0xD6301151D6301151
DATA idct8P1x<>+72(SB)/8, $0xD6301151D6301151
DATA idct8P1x<>+80(SB)/8, $0xD6301151D6301151
DATA idct8P1x<>+88(SB)/8, $0xD6301151D6301151
DATA idct8P1x<>+96(SB)/8, $0x115129CF115129CF
DATA idct8P1x<>+104(SB)/8, $0x115129CF115129CF
DATA idct8P1x<>+112(SB)/8, $0x115129CF115129CF
DATA idct8P1x<>+120(SB)/8, $0x115129CF115129CF
DATA idct8P1x<>+128(SB)/8, $0x25A108D425A108D4
DATA idct8P1x<>+136(SB)/8, $0x25A108D425A108D4
DATA idct8P1x<>+144(SB)/8, $0x25A108D425A108D4
DATA idct8P1x<>+152(SB)/8, $0x25A108D425A108D4
DATA idct8P1x<>+160(SB)/8, $0xD39DE6DCD39DE6DC
DATA idct8P1x<>+168(SB)/8, $0xD39DE6DCD39DE6DC
DATA idct8P1x<>+176(SB)/8, $0xD39DE6DCD39DE6DC
DATA idct8P1x<>+184(SB)/8, $0xD39DE6DCD39DE6DC
DATA idct8P1x<>+192(SB)/8, $0x08D5192508D51925
DATA idct8P1x<>+200(SB)/8, $0x08D5192508D51925
DATA idct8P1x<>+208(SB)/8, $0x08D5192508D51925
DATA idct8P1x<>+216(SB)/8, $0x08D5192508D51925
DATA idct8P1x<>+224(SB)/8, $0x25A1D39E25A1D39E
DATA idct8P1x<>+232(SB)/8, $0x25A1D39E25A1D39E
DATA idct8P1x<>+240(SB)/8, $0x25A1D39E25A1D39E
DATA idct8P1x<>+248(SB)/8, $0x25A1D39E25A1D39E
DATA idct8P1x<>+256(SB)/8, $0xD39E25A1D39E25A1
DATA idct8P1x<>+264(SB)/8, $0xD39E25A1D39E25A1
DATA idct8P1x<>+272(SB)/8, $0xD39E25A1D39E25A1
DATA idct8P1x<>+280(SB)/8, $0xD39E25A1D39E25A1
DATA idct8P1x<>+288(SB)/8, $0xE6DCF72DE6DCF72D
DATA idct8P1x<>+296(SB)/8, $0xE6DCF72DE6DCF72D
DATA idct8P1x<>+304(SB)/8, $0xE6DCF72DE6DCF72D
DATA idct8P1x<>+312(SB)/8, $0xE6DCF72DE6DCF72D
DATA idct8P1x<>+320(SB)/8, $0x19252C6319252C63
DATA idct8P1x<>+328(SB)/8, $0x19252C6319252C63
DATA idct8P1x<>+336(SB)/8, $0x19252C6319252C63
DATA idct8P1x<>+344(SB)/8, $0x19252C6319252C63
DATA idct8P1x<>+352(SB)/8, $0x08D425A108D425A1
DATA idct8P1x<>+360(SB)/8, $0x08D425A108D425A1
DATA idct8P1x<>+368(SB)/8, $0x08D425A108D425A1
DATA idct8P1x<>+376(SB)/8, $0x08D425A108D425A1

#define fP04a  idct8P1x<>+0(SB)
#define fP04b  idct8P1x<>+32(SB)
#define fP26a  idct8P1x<>+64(SB)
#define fP26b  idct8P1x<>+96(SB)
#define fO0a   idct8P1x<>+128(SB)
#define fO0b   idct8P1x<>+160(SB)
#define fO1a   idct8P1x<>+192(SB)
#define fO1b   idct8P1x<>+224(SB)
#define fO2a   idct8P1x<>+256(SB)
#define fO2b   idct8P1x<>+288(SB)
#define fO3a   idct8P1x<>+320(SB)
#define fO3b   idct8P1x<>+352(SB)

// func idct8AVX2Asm(coef *[64]int16, quant *[64]uint16, dst *byte, stride int)
//
// Ports turbo's jsimd_idct_islow_avx2 (simd/x86_64/jidctint-avx2.asm:278)
// in spirit, not instruction for instruction. Pass 1 (the column IDCT)
// dequantizes with a 16-bit truncating multiply (VPMULLW), as turbo's
// AVX2 and kernel_arm64.s do, then does every product with VPMADDWD on
// (row, row) word pairs, one dword lane per column. That matches
// idct8Go whenever each coef*quant fits int16 (docs/design.md section
// 4); unlike turbo it never adds two 16-bit values. Its 8 rows stay in
// registers; an in-register 8x8 dword transpose turns them into
// columns and pass 2 runs the same body across the 8 rows. Pass 2 is
// 32-bit too, exact for any int32 ws: the low 32 bits of idct8Go's
// int64 sums hold bits 18-27, all rangeLimit[x&1023] reads. The output
// packs to bytes and a VPSHUFB/VPUNPCK*DQ transpose gives 8-byte rows.
TEXT ·idct8AVX2Asm(SB), NOSPLIT, $0-32
	MOVQ coef+0(FP), BX
	MOVQ quant+8(FP), CX

	// ---- Pass 1, 16-bit lanes: dequantize two rows per VPMULLW. ----
	VMOVDQU (BX), Y0
	VPMULLW (CX), Y0, Y0     // rows 0,1
	VMOVDQU 32(BX), Y1
	VPMULLW 32(CX), Y1, Y1   // rows 2,3
	VMOVDQU 64(BX), Y2
	VPMULLW 64(CX), Y2, Y2   // rows 4,5
	VMOVDQU 96(BX), Y3
	VPMULLW 96(CX), Y3, Y3   // rows 6,7
	VPERMQ  $0xD8, Y0, Y0    // lane 0: cols 0-3 of both rows
	VPERMQ  $0xD8, Y1, Y1
	VPERMQ  $0xD8, Y2, Y2
	VPERMQ  $0xD8, Y3, Y3
	VPUNPCKLWD Y2, Y0, Y4    // (row0,row4) per column
	VPUNPCKHWD Y2, Y0, Y5    // (row1,row5)
	VPUNPCKLWD Y3, Y1, Y6    // (row2,row6)
	VPUNPCKHWD Y3, Y1, Y7    // (row3,row7)

	VPMADDWD fP04a, Y4, Y14  // tmp0 = (r0+r4)<<13
	VPMADDWD fP04b, Y4, Y15  // tmp1 = (r0-r4)<<13
	VPADDD   fBIAS1, Y14, Y14
	VPADDD   fBIAS1, Y15, Y15
	VPMADDWD fP26a, Y6, Y12  // tmp2
	VPMADDWD fP26b, Y6, Y13  // tmp3
	VPADDD   Y13, Y14, Y8    // tmp10
	VPSUBD   Y13, Y14, Y9    // tmp13
	VPADDD   Y12, Y15, Y10   // tmp11
	VPSUBD   Y12, Y15, Y11   // tmp12

	VPMADDWD fO0a, Y5, Y12
	VPMADDWD fO0b, Y7, Y0
	VPADDD   Y0, Y12, Y12    // odd tmp0
	VPMADDWD fO1a, Y5, Y13
	VPMADDWD fO1b, Y7, Y0
	VPADDD   Y0, Y13, Y13    // odd tmp1
	VPMADDWD fO2a, Y5, Y14
	VPMADDWD fO2b, Y7, Y0
	VPADDD   Y0, Y14, Y14    // odd tmp2
	VPMADDWD fO3a, Y5, Y15
	VPMADDWD fO3b, Y7, Y0
	VPADDD   Y0, Y15, Y15    // odd tmp3

	VPADDD Y15, Y8, Y0
	VPSRAD $11, Y0, Y0 // ws row0 = tmp10+tmp3
	VPSUBD Y15, Y8, Y1
	VPSRAD $11, Y1, Y1 // row7
	VPADDD Y14, Y10, Y2
	VPSRAD $11, Y2, Y2 // row1
	VPSUBD Y14, Y10, Y3
	VPSRAD $11, Y3, Y3 // row6
	VPADDD Y13, Y11, Y4
	VPSRAD $11, Y4, Y4 // row2
	VPSUBD Y13, Y11, Y5
	VPSRAD $11, Y5, Y5 // row5
	VPADDD Y12, Y9, Y6
	VPSRAD $11, Y6, Y6 // row3
	VPSUBD Y12, Y9, Y7
	VPSRAD $11, Y7, Y7 // row4

	// ---- Transpose: rows 0-7 are Y0 Y2 Y4 Y6 Y7 Y5 Y3 Y1. ----
	VPUNPCKLDQ  Y2, Y0, Y8
	VPUNPCKHDQ  Y2, Y0, Y9
	VPUNPCKLDQ  Y6, Y4, Y10
	VPUNPCKHDQ  Y6, Y4, Y11
	VPUNPCKLDQ  Y5, Y7, Y12
	VPUNPCKHDQ  Y5, Y7, Y13
	VPUNPCKLDQ  Y1, Y3, Y14
	VPUNPCKHDQ  Y1, Y3, Y15
	VPUNPCKLQDQ Y10, Y8, Y0  // col0 rows 0-3 | col4
	VPUNPCKHQDQ Y10, Y8, Y1  // col1 | col5
	VPUNPCKLQDQ Y11, Y9, Y2  // col2 | col6
	VPUNPCKHQDQ Y11, Y9, Y3  // col3 | col7
	VPUNPCKLQDQ Y14, Y12, Y4 // col0 rows 4-7 | col4
	VPUNPCKHQDQ Y14, Y12, Y5
	VPUNPCKLQDQ Y15, Y13, Y6
	VPUNPCKHQDQ Y15, Y13, Y7
	VPERM2I128  $0x20, Y4, Y0, Y8
	VPERM2I128  $0x31, Y4, Y0, Y12
	VPERM2I128  $0x20, Y5, Y1, Y9
	VPERM2I128  $0x31, Y5, Y1, Y13
	VPERM2I128  $0x20, Y6, Y2, Y10
	VPERM2I128  $0x31, Y6, Y2, Y14
	VPERM2I128  $0x20, Y7, Y3, Y11
	VPERM2I128  $0x31, Y7, Y3, Y15
	VMOVDQU     Y8, Y0
	VMOVDQU     Y9, Y1
	VMOVDQU     Y10, Y2
	VMOVDQU     Y11, Y3
	VMOVDQU     Y12, Y4
	VMOVDQU     Y13, Y5
	VMOVDQU     Y14, Y6
	VMOVDQU     Y15, Y7

	// ---- Pass 2: rows, one YMM lane per row. ----
	IDCT8AVX2(fR16) // (w0+16)<<17 is 16x DESCALE's 1<<17

	VPADDD Y15, Y8, Y0  // out0
	VPSUBD Y15, Y8, Y1  // out7
	VPADDD Y14, Y10, Y2 // out1
	VPSUBD Y14, Y10, Y3 // out6
	VPADDD Y13, Y11, Y4 // out2
	VPSUBD Y13, Y11, Y5 // out5
	VPADDD Y12, Y9, Y6  // out3
	VPSUBD Y12, Y9, Y7  // out4

	// Sums are 16x: >>22 sign-extends bits 18-27, x>>18 & 1023.
	VPSRAD $22, Y0, Y0
	VPSRAD $22, Y1, Y1
	VPSRAD $22, Y2, Y2
	VPSRAD $22, Y3, Y3
	VPSRAD $22, Y4, Y4
	VPSRAD $22, Y5, Y5
	VPSRAD $22, Y6, Y6
	VPSRAD $22, Y7, Y7

	// rangeLimit is clamp(v+128): saturate to int8, flip the sign bit.
	VPACKSSDW Y2, Y0, Y8    // out0|out1, rows 0-3 | rows 4-7
	VPACKSSDW Y6, Y4, Y9    // out2|out3
	VPACKSSDW Y5, Y7, Y10   // out4|out5
	VPACKSSDW Y1, Y3, Y11   // out6|out7
	VPACKSSWB Y9, Y8, Y8    // byte 4j+r = out j, row r
	VPACKSSWB Y11, Y10, Y10
	VPXOR     fX80, Y8, Y8
	VPXOR     fX80, Y10, Y10
	VPSHUFB   fTRANS, Y8, Y8   // dword r = row r, cols 0-3
	VPSHUFB   fTRANS, Y10, Y10 // cols 4-7
	VPUNPCKLDQ Y10, Y8, Y0     // rows 0,1 | rows 4,5
	VPUNPCKHDQ Y10, Y8, Y1     // rows 2,3 | rows 6,7

	MOVQ    dst+16(FP), R10
	MOVQ    stride+24(FP), R11
	LEAQ    (R11)(R11*2), R12
	VMOVQ   X0, (R10)
	VMOVHPS X0, (R10)(R11*1)
	VMOVQ   X1, (R10)(R11*2)
	VMOVHPS X1, (R10)(R12*1)
	LEAQ    (R10)(R11*4), R10
	VEXTRACTI128 $1, Y0, X0
	VEXTRACTI128 $1, Y1, X1
	VMOVQ   X0, (R10)
	VMOVHPS X0, (R10)(R11*1)
	VMOVQ   X1, (R10)(R11*2)
	VMOVHPS X1, (R10)(R12*1)

	VZEROUPPER
	RET

// Pass-1 word-pair constants for idct4AVX2Asm's VPMADDWD, each 8x:
// (row0,1) times (1<<14, 2048) gives tmp0 plus the rounding, since row
// 4 is unused and replaced by ones; (row2,row6), then (row1,row5) and
// (row3,row7) for oddA and oddB; then the ones words.
GLOBL idct4P1x<>(SB), RODATA, $224
DATA idct4P1x<>+0(SB)/8, $0x0800400008004000
DATA idct4P1x<>+8(SB)/8, $0x0800400008004000
DATA idct4P1x<>+16(SB)/8, $0x0800400008004000
DATA idct4P1x<>+24(SB)/8, $0x0800400008004000
DATA idct4P1x<>+32(SB)/8, $0xE7823B21E7823B21
DATA idct4P1x<>+40(SB)/8, $0xE7823B21E7823B21
DATA idct4P1x<>+48(SB)/8, $0xE7823B21E7823B21
DATA idct4P1x<>+56(SB)/8, $0xE7823B21E7823B21
DATA idct4P1x<>+64(SB)/8, $0x2E7521F92E7521F9
DATA idct4P1x<>+72(SB)/8, $0x2E7521F92E7521F9
DATA idct4P1x<>+80(SB)/8, $0x2E7521F92E7521F9
DATA idct4P1x<>+88(SB)/8, $0x2E7521F92E7521F9
DATA idct4P1x<>+96(SB)/8, $0xF93EBA79F93EBA79
DATA idct4P1x<>+104(SB)/8, $0xF93EBA79F93EBA79
DATA idct4P1x<>+112(SB)/8, $0xF93EBA79F93EBA79
DATA idct4P1x<>+120(SB)/8, $0xF93EBA79F93EBA79
DATA idct4P1x<>+128(SB)/8, $0xECC25203ECC25203
DATA idct4P1x<>+136(SB)/8, $0xECC25203ECC25203
DATA idct4P1x<>+144(SB)/8, $0xECC25203ECC25203
DATA idct4P1x<>+152(SB)/8, $0xECC25203ECC25203
DATA idct4P1x<>+160(SB)/8, $0xEFB01CCDEFB01CCD
DATA idct4P1x<>+168(SB)/8, $0xEFB01CCDEFB01CCD
DATA idct4P1x<>+176(SB)/8, $0xEFB01CCDEFB01CCD
DATA idct4P1x<>+184(SB)/8, $0xEFB01CCDEFB01CCD
DATA idct4P1x<>+192(SB)/8, $0x0001000100010001
DATA idct4P1x<>+200(SB)/8, $0x0001000100010001
DATA idct4P1x<>+208(SB)/8, $0x0001000100010001
DATA idct4P1x<>+216(SB)/8, $0x0001000100010001

#define fQ04   idct4P1x<>+0(SB)
#define fQ26   idct4P1x<>+32(SB)
#define fQA15  idct4P1x<>+64(SB)
#define fQA37  idct4P1x<>+96(SB)
#define fQB15  idct4P1x<>+128(SB)
#define fQB37  idct4P1x<>+160(SB)
#define fQOne  idct4P1x<>+192(SB)

// func idct4AVX2Asm(coef *[64]int16, quant *[64]uint16, dst *byte, stride int)
//
// Ports jsimd_idct_4x4 in spirit (turbo has no AVX2 jidctred; the closest
// reference is simd/x86_64/jidctred-sse2.asm, per docs/design.md section
// 8's row 8: AVX2 or SSE2 both satisfy the task, gated by the same AVX2
// check as idct8AVX2Asm for simplicity). Structured like idct8AVX2Asm and
// kernel_arm64.s's idct4NEONAsm: pass 1 (columns) is AVX2, one lane per
// column, dequantizing with VPMULLW and multiplying with VPMADDWD; row 4
// (idct4Go's column loop never reads it) is paired with a zero
// constant, and column 4's dead lane is computed anyway
// rather than special-cased out, since pass 2 below never reads it back
// (ws slot 4 of each row). The 4 rows stay in registers; a 4x8 dword
// transpose leaves two columns per YMM ([w1|w5], [w3|w7], ...), so pass
// 2 multiplies both halves by per-half constants and folds them with
// VPERM2I128. It is 32-bit and exact for any int32 ws, as in
// idct8AVX2Asm; the bytes go out through VPSHUFB, 4 per row.
TEXT ·idct4AVX2Asm(SB), NOSPLIT, $0-32
	MOVQ coef+0(FP), BX
	MOVQ quant+8(FP), CX

	// ---- Pass 1, 16-bit lanes, as idct8AVX2Asm's. ----
	VMOVDQU (BX), Y0
	VPMULLW (CX), Y0, Y0     // rows 0,1
	VMOVDQU 32(BX), Y1
	VPMULLW 32(CX), Y1, Y1   // rows 2,3
	VMOVDQU 64(BX), Y2
	VPMULLW 64(CX), Y2, Y2   // rows 4,5
	VPBLENDD $0x0F, fQOne, Y2, Y2 // row 4 is unused: ones
	VMOVDQU 96(BX), Y3
	VPMULLW 96(CX), Y3, Y3   // rows 6,7
	VPERMQ  $0xD8, Y0, Y0
	VPERMQ  $0xD8, Y1, Y1
	VPERMQ  $0xD8, Y2, Y2
	VPERMQ  $0xD8, Y3, Y3
	VPUNPCKLWD Y2, Y0, Y4    // (row0,row4) per column
	VPUNPCKHWD Y2, Y0, Y5    // (row1,row5)
	VPUNPCKLWD Y3, Y1, Y6    // (row2,row6)
	VPUNPCKHWD Y3, Y1, Y7    // (row3,row7)

	VPMADDWD fQ04, Y4, Y0    // tmp0 = row0<<14, plus 2048
	VPMADDWD fQ26, Y6, Y12   // tmp2
	VPADDD   Y12, Y0, Y8     // tmp10
	VPSUBD   Y12, Y0, Y9     // tmp12
	VPMADDWD fQA15, Y5, Y10
	VPMADDWD fQA37, Y7, Y1
	VPADDD   Y1, Y10, Y10    // oddA
	VPMADDWD fQB15, Y5, Y11
	VPMADDWD fQB37, Y7, Y1
	VPADDD   Y1, Y11, Y11    // oddB

	// -- Descale (shift 12): rows 0,3,1,2 in Y0-Y3. --
	VPADDD Y11, Y8, Y0
	VPSRAD $12, Y0, Y0 // ws row0 = tmp10+oddB
	VPSUBD Y11, Y8, Y1
	VPSRAD $12, Y1, Y1 // row3 = tmp10-oddB
	VPADDD Y10, Y9, Y2
	VPSRAD $12, Y2, Y2 // row1 = tmp12+oddA
	VPSUBD Y10, Y9, Y3
	VPSRAD $12, Y3, Y3 // row2 = tmp12-oddA

	// ---- Transpose rows Y0 Y2 Y3 Y1 into column pairs. ----
	VPUNPCKLDQ  Y2, Y0, Y8
	VPUNPCKHDQ  Y2, Y0, Y9
	VPUNPCKLDQ  Y1, Y3, Y10
	VPUNPCKHDQ  Y1, Y3, Y11
	VPUNPCKLQDQ Y10, Y8, Y4 // w0|w4, rows 0-3 in each half
	VPUNPCKHQDQ Y10, Y8, Y5 // w1|w5
	VPUNPCKLQDQ Y11, Y9, Y6 // w2|w6
	VPUNPCKHQDQ Y11, Y9, Y7 // w3|w7

	// ---- Pass 2: the 4 rows, one lane each. ----
	VPERM2I128 $0x00, Y4, Y4, Y8 // w0|w0
	VPADDD     fR16, Y8, Y8      // +16 is 8x DESCALE's 1<<18 after <<17
	VPSLLD     $17, Y8, Y8       // tmp0|tmp0, times 8

	VPMULLD    fK26, Y6, Y9          // w2*C1847 | w6*NC765
	VPERM2I128 $0x01, Y9, Y9, Y10
	VPADDD     Y10, Y9, Y9           // tmp2|tmp2
	VPSIGND    fKSign, Y9, Y9        // tmp2|-tmp2
	VPADDD     Y9, Y8, Y8            // tmp10|tmp12

	VPMULLD fKA15, Y5, Y10
	VPMULLD fKA37, Y7, Y11
	VPADDD  Y11, Y10, Y10 // oddA terms: w1,w3 | w5,w7
	VPMULLD fKB15, Y5, Y11
	VPMULLD fKB37, Y7, Y12
	VPADDD  Y12, Y11, Y11 // oddB terms
	VPERM2I128 $0x20, Y10, Y11, Y12
	VPERM2I128 $0x31, Y10, Y11, Y13
	VPADDD     Y13, Y12, Y12 // oddB|oddA

	VPADDD Y12, Y8, Y0 // out0|out1
	VPSUBD Y12, Y8, Y1 // out3|out2

	// Sums are 8x: >>22 sign-extends bits 19-28, then clamp(v+128).
	VPSRAD    $22, Y0, Y0
	VPSRAD    $22, Y1, Y1
	VPACKSSDW Y1, Y0, Y0 // out0,out3 | out1,out2
	VPACKSSWB Y0, Y0, Y0
	VPXOR     fX80, Y0, Y0
	VEXTRACTI128 $1, Y0, X1
	VPUNPCKLDQ   X1, X0, X0 // out0,out1,out3,out2, 4 rows each
	VPSHUFB      fR4TRANS, X0, X0

	MOVQ    dst+16(FP), R10
	MOVQ    stride+24(FP), R11
	LEAQ    (R11)(R11*2), R12
	VMOVD   X0, (R10)
	VPEXTRD $1, X0, (R10)(R11*1)
	VPEXTRD $2, X0, (R10)(R11*2)
	VPEXTRD $3, X0, (R10)(R12*1)

	VZEROUPPER
	RET

// Upsample-kernel constants: h2v2FancyRowGo's 3/8/7 weights as 16
// words each, then a VPSHUFB index interleaving each 128-bit lane's 8
// left and 8 right bytes into left0,right0,left1,...
GLOBL avx2Up<>(SB), RODATA, $128
DATA avx2Up<>+0(SB)/8, $0x0003000300030003
DATA avx2Up<>+8(SB)/8, $0x0003000300030003
DATA avx2Up<>+16(SB)/8, $0x0003000300030003
DATA avx2Up<>+24(SB)/8, $0x0003000300030003
DATA avx2Up<>+32(SB)/8, $0x0008000800080008
DATA avx2Up<>+40(SB)/8, $0x0008000800080008
DATA avx2Up<>+48(SB)/8, $0x0008000800080008
DATA avx2Up<>+56(SB)/8, $0x0008000800080008
DATA avx2Up<>+64(SB)/8, $0x0007000700070007
DATA avx2Up<>+72(SB)/8, $0x0007000700070007
DATA avx2Up<>+80(SB)/8, $0x0007000700070007
DATA avx2Up<>+88(SB)/8, $0x0007000700070007
DATA avx2Up<>+96(SB)/8, $0x0B030A0209010800
DATA avx2Up<>+104(SB)/8, $0x0F070E060D050C04
DATA avx2Up<>+112(SB)/8, $0x0B030A0209010800
DATA avx2Up<>+120(SB)/8, $0x0F070E060D050C04

#define uThree avx2Up<>+0(SB)
#define uEight avx2Up<>+32(SB)
#define uSeven avx2Up<>+64(SB)
#define uZip   avx2Up<>+96(SB)

// Colour-kernel constants (see colorRowAVX2Asm): 128, 26345 and -7471
// (half of -14942) as words, the VPMADDWD word pair [-22554,18734],
// yccOneHalf as dwords, 255 as words.
GLOBL avx2Color<>(SB), RODATA, $192
DATA avx2Color<>+0(SB)/8, $0x0080008000800080
DATA avx2Color<>+8(SB)/8, $0x0080008000800080
DATA avx2Color<>+16(SB)/8, $0x0080008000800080
DATA avx2Color<>+24(SB)/8, $0x0080008000800080
DATA avx2Color<>+32(SB)/8, $0x66E966E966E966E9
DATA avx2Color<>+40(SB)/8, $0x66E966E966E966E9
DATA avx2Color<>+48(SB)/8, $0x66E966E966E966E9
DATA avx2Color<>+56(SB)/8, $0x66E966E966E966E9
DATA avx2Color<>+64(SB)/8, $0xE2D1E2D1E2D1E2D1
DATA avx2Color<>+72(SB)/8, $0xE2D1E2D1E2D1E2D1
DATA avx2Color<>+80(SB)/8, $0xE2D1E2D1E2D1E2D1
DATA avx2Color<>+88(SB)/8, $0xE2D1E2D1E2D1E2D1
DATA avx2Color<>+96(SB)/8, $0x492EA7E6492EA7E6
DATA avx2Color<>+104(SB)/8, $0x492EA7E6492EA7E6
DATA avx2Color<>+112(SB)/8, $0x492EA7E6492EA7E6
DATA avx2Color<>+120(SB)/8, $0x492EA7E6492EA7E6
DATA avx2Color<>+128(SB)/8, $0x0000800000008000
DATA avx2Color<>+136(SB)/8, $0x0000800000008000
DATA avx2Color<>+144(SB)/8, $0x0000800000008000
DATA avx2Color<>+152(SB)/8, $0x0000800000008000
DATA avx2Color<>+160(SB)/8, $0x00FF00FF00FF00FF
DATA avx2Color<>+168(SB)/8, $0x00FF00FF00FF00FF
DATA avx2Color<>+176(SB)/8, $0x00FF00FF00FF00FF
DATA avx2Color<>+184(SB)/8, $0x00FF00FF00FF00FF

#define cC128   avx2Color<>+0(SB)
#define cCrR    avx2Color<>+32(SB)
#define cCbB    avx2Color<>+64(SB)
#define cG      avx2Color<>+96(SB)
#define cBias   avx2Color<>+128(SB)
#define cC255   avx2Color<>+160(SB)

// func colorRowAVX2Asm(y, cb, cr, dst *byte, n int)
//
// colorRowGo in AVX2, 16 pixels per iteration in 16-bit lanes, with
// the same exact constant split as kernel_arm64.s's colorRowNEONAsm:
// (91881x+32768)>>16 = x+((26345x+32768)>>16), (116130x+32768)>>16 =
// 2x+((32768-14942x)>>16), and G's >>16 = -xr+((-22554xb+18734xr
// +32768)>>16). For R, (c*x+32768)>>16 is VPMULHW(x,c) plus bit 15 of
// VPMULLW(x,c), the rounding carry. B's c is even, so it is one
// VPMULHRSW by c/2: (2*x*(c/2)+32768)>>16. G's two products go through
// VPMADDWD on [xb,xr] word pairs, VPSRAD/VPACKSSDW bring it back to
// words, and VPACKUSWB clamps to 0..255 (sums stay in -285..538). The bytes are interleaved to RGBA with VPSHUFB and
// VPUNPCK[LH]WD and stored as two 32-byte rows of 8 pixels. n must be
// a positive multiple of 16; colorRowAVX2 runs the remainder through
// colorRowGo.
TEXT ·colorRowAVX2Asm(SB), NOSPLIT, $0-40
	MOVQ y+0(FP), SI
	MOVQ cb+8(FP), DI
	MOVQ cr+16(FP), DX
	MOVQ dst+24(FP), R8
	MOVQ n+32(FP), R9

	VMOVDQU  uZip, Y8
	VMOVDQU  cC255, Y9
	VMOVDQU  cBias, Y10
	VMOVDQU  cG, Y12
	VMOVDQU  cCbB, Y13
	VMOVDQU  cCrR, Y14
	VMOVDQU  cC128, Y15

	XORQ R10, R10

colorloop:

	VPMOVZXBW (SI)(R10*1), Y0 // y
	VPMOVZXBW (DI)(R10*1), Y1 // cb
	VPMOVZXBW (DX)(R10*1), Y2 // cr
	VPSUBW    Y15, Y1, Y1 // xb
	VPSUBW    Y15, Y2, Y2 // xr

	VPMULHW Y14, Y2, Y3
	VPMULLW Y14, Y2, Y4
	VPSRLW  $15, Y4, Y4
	VPADDW  Y4, Y3, Y3 // (26345*xr+32768)>>16
	VPADDW  Y2, Y3, Y3
	VPADDW  Y0, Y3, Y3 // R

	VPMULHRSW Y13, Y1, Y4 // (32768-14942*xb)>>16
	VPADDW  Y1, Y4, Y4
	VPADDW  Y1, Y4, Y4
	VPADDW  Y0, Y4, Y4 // B

	VPUNPCKLWD Y2, Y1, Y5
	VPUNPCKHWD Y2, Y1, Y6
	VPMADDWD   Y12, Y5, Y5
	VPMADDWD   Y12, Y6, Y6
	VPADDD     Y10, Y5, Y5
	VPADDD     Y10, Y6, Y6
	VPSRAD     $16, Y5, Y5
	VPSRAD     $16, Y6, Y6
	VPACKSSDW  Y6, Y5, Y5
	VPSUBW     Y2, Y0, Y0
	VPADDW     Y0, Y5, Y5 // G

	VPACKUSWB  Y5, Y3, Y3 // per lane: R0-7, G0-7
	VPACKUSWB  Y9, Y4, Y4 // B0-7, A0-7
	VPSHUFB    Y8, Y3, Y3 // R0 G0 R1 G1 ...
	VPSHUFB    Y8, Y4, Y4 // B0 A0 B1 A1 ...
	VPUNPCKLWD Y4, Y3, Y5 // pixels 0-3 | 8-11
	VPUNPCKHWD Y4, Y3, Y6 // pixels 4-7 | 12-15
	VPERM2I128 $0x20, Y6, Y5, Y0
	VPERM2I128 $0x31, Y6, Y5, Y1
	VMOVDQU    Y0, (R8)(R10*4)
	VMOVDQU    Y1, 32(R8)(R10*4)

	ADDQ $16, R10
	CMPQ R10, R9
	JNE  colorloop

	VZEROUPPER
	RET

// func h2v1FancyRowAVX2Asm(a, b, c, dst *byte, n int)
//
// h2v1FancyRow's interior-column loop (jdsample.c h2v1_fancy_upsample,
// :295-325) in AVX2, 32 columns per iteration, entirely in byte lanes
// with the NEON kernel's halving-add identities: (3b+a+1)>>2 =
// floor((ceil((a+b)/2) + b)/2) and (3b+c+2)>>2 = ceil((floor((c+b)/2)
// + b)/2). VPAVGB is the rounding average; the floor average is
// NOT(VPAVGB(NOT x, NOT y)). VPUNPCK[LH]BW interleave left and right,
// VPERM2I128 restores column order across the two 128-bit lanes, and
// two VMOVDQUs store 64 bytes. n must be a positive multiple of 32.
TEXT ·h2v1FancyRowAVX2Asm(SB), NOSPLIT, $0-40
	MOVQ a+0(FP), SI
	MOVQ b+8(FP), DI
	MOVQ c+16(FP), DX
	MOVQ dst+24(FP), R8
	MOVQ n+32(FP), R9

	VPCMPEQB Y15, Y15, Y15 // all ones, for NOT

h2v1loop:
	VMOVDQU (SI), Y0 // a
	VMOVDQU (DI), Y1 // b
	VMOVDQU (DX), Y2 // c

	VPXOR  Y15, Y1, Y3 // ~b
	VPAVGB Y1, Y0, Y4  // ceil((a+b)/2)
	VPXOR  Y15, Y4, Y4
	VPAVGB Y3, Y4, Y4
	VPXOR  Y15, Y4, Y4 // left = floor((that+b)/2)
	VPXOR  Y15, Y2, Y5
	VPAVGB Y3, Y5, Y5
	VPXOR  Y15, Y5, Y5 // floor((c+b)/2)
	VPAVGB Y1, Y5, Y5  // right

	VPUNPCKLBW Y5, Y4, Y6 // columns 0-7 | 16-23
	VPUNPCKHBW Y5, Y4, Y7 // columns 8-15 | 24-31
	VPERM2I128 $0x20, Y7, Y6, Y8
	VPERM2I128 $0x31, Y7, Y6, Y9
	VMOVDQU    Y8, (R8)
	VMOVDQU    Y9, 32(R8)

	ADDQ $32, SI
	ADDQ $32, DI
	ADDQ $32, DX
	ADDQ $64, R8
	SUBQ $32, R9
	JNZ  h2v1loop

	VZEROUPPER
	RET

// func h2v2FancyRowAVX2Asm(curA, curB, curC, otherA, otherB, otherC, dst *byte, n int)
//
// h2v2FancyRow's interior-column loop (jdsample.c h2v2_fancy_upsample,
// :381-429) in AVX2, matching h2v2FancyRowGo's colsum/blend formula per
// column (9:3:3:1 weighted blend, truncating >>4): colsum(k) =
// rowCur[k]*3+rowOther[k] <= 1020, and 3*1020+1020+8 = 4088 fits a
// word, so the 16-bit lanes are exact. The left neighbours' colsums are
// the previous block's shifted one word (VPERM2I128 + VPALIGNR), seeded
// with colsum(c-1), so two colsums are computed per column. Same pack,
// interleave and store as h2v1FancyRowAVX2Asm. n must be a positive
// multiple of 16.
TEXT ·h2v2FancyRowAVX2Asm(SB), NOSPLIT, $0-64
	MOVQ curA+0(FP), SI
	MOVQ curB+8(FP), DI
	MOVQ curC+16(FP), DX
	MOVQ otherA+24(FP), R8
	MOVQ otherB+32(FP), R9
	MOVQ otherC+40(FP), R10
	MOVQ dst+48(FP), R11
	MOVQ n+56(FP), R12

	VMOVDQU uThree, Y12
	VMOVDQU uEight, Y13
	VMOVDQU uSeven, Y14
	VMOVDQU uZip, Y15

	MOVBLZX      (SI), R13
	MOVBLZX      (R8), R14
	LEAL         (R13)(R13*2), R13
	ADDL         R14, R13
	VMOVD        R13, X11
	VPBROADCASTW X11, Y11 // word 15 = colsum(c-1)

	XORQ R13, R13

h2v2loop:

	VPMOVZXBW (DI)(R13*1), Y1  // curB
	VPMOVZXBW (DX)(R13*1), Y2  // curC
	VPMOVZXBW (R9)(R13*1), Y4  // otherB
	VPMOVZXBW (R10)(R13*1), Y5 // otherC

	VPMULLW Y12, Y1, Y1
	VPADDW  Y4, Y1, Y1 // colsumB = curB*3+otherB
	VPMULLW Y12, Y2, Y2
	VPADDW  Y5, Y2, Y2 // colsumC

	VPERM2I128 $0x21, Y1, Y11, Y3 // previous high half | this low half
	VPALIGNR   $14, Y3, Y1, Y0    // colsumA: one word later
	VMOVDQU    Y1, Y11

	VPMULLW Y12, Y1, Y1 // colsumB*3
	VPADDW  Y1, Y0, Y0
	VPADDW  Y13, Y0, Y0 // left = colsumB*3+colsumA+8
	VPADDW  Y1, Y2, Y2
	VPADDW  Y14, Y2, Y2 // right = colsumB*3+colsumC+7
	VPSRLW  $4, Y0, Y0
	VPSRLW  $4, Y2, Y2

	VPACKUSWB Y2, Y0, Y0
	VPSHUFB   Y15, Y0, Y0
	VMOVDQU   Y0, (R11)(R13*2)

	ADDQ $16, R13
	CMPQ R13, R12
	JNE  h2v2loop

	VZEROUPPER
	RET
