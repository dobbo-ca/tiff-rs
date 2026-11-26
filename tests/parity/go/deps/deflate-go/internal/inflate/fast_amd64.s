//go:build !purego

#include "textflag.h"

// Fast loops; contract in fast_generic.go.
//
// R8 bitbuf, DX bitsleft&63.
// SI in, DI out, CX entry.
// R9, R10: litlen, dist tables.
// BX: last litlen symbol, for slack.
// AX: 11 in the bmi2 loop.
// Locals: inLimit, outLimit-2, hist pointers.
// outLimit-2 covers three unchecked literals.
// Not preemptible: stop within 1 MiB.

// fastState offsets.
#define S_IN 0
#define S_INPOS 8
#define S_INLIMIT 16
#define S_OUT 24
#define S_OUTPOS 32
#define S_OUTLIMIT 40
#define S_HISTBASE 48
#define S_BITBUF 56
#define S_BITSLEFT 64
#define S_SLACK 72
#define S_LITLEN 80
#define S_DIST 88
#define S_PAIR 96

#define LOAD_STATE \
	MOVQ s+0(FP), AX; \
	MOVQ S_IN(AX), SI; \
	MOVQ S_OUT(AX), DI; \
	MOVQ S_INLIMIT(AX), R11; \
	ADDQ SI, R11; \
	MOVQ R11, 0(SP); \
	MOVQ S_OUTLIMIT(AX), R11; \
	LEAQ -2(DI)(R11*1), R11; \
	MOVQ R11, 8(SP); \
	MOVQ S_HISTBASE(AX), R11; \
	ADDQ DI, R11; \
	MOVQ R11, 16(SP); \
	ADDQ S_INPOS(AX), SI; \
	ADDQ S_OUTPOS(AX), DI; \
	LEAQ 0x100000(DI), R11; \
	MOVQ 8(SP), R12; \
	CMPQ R11, R12; \
	CMOVQCS R11, R12; \
	MOVQ R12, 8(SP); \
	MOVQ S_BITBUF(AX), R8; \
	MOVQ S_BITSLEFT(AX), DX; \
	MOVQ S_SLACK(AX), BX; \
	SHLQ $12, BX; \
	MOVQ S_LITLEN(AX), R9; \
	MOVQ S_DIST(AX), R10

// Literal: BX is entry ror 16.
// Then 8-11 slack, 23 litFlag.
// Match: BX = e&0xffff + d&31.
// Then BL used, 12-15 pullN.
#define STORE_STATE \
	MOVQ s+0(FP), AX; \
	SUBQ S_IN(AX), SI; \
	MOVQ SI, S_INPOS(AX); \
	SUBQ S_OUT(AX), DI; \
	MOVQ DI, S_OUTPOS(AX); \
	MOVQ R8, S_BITBUF(AX); \
	ANDQ $63, DX; \
	MOVQ DX, S_BITSLEFT(AX); \
	XORL R13, R13; \
	MOVL BX, R11; \
	SHRL $12, R11; \
	ANDL $15, R11; \
	MOVBLZX BX, R12; \
	SUBL R12, R11; \
	CMOVLLT R13, R11; \
	MOVL BX, R12; \
	SHRL $8, R12; \
	ANDL $15, R12; \
	TESTL $0x800000, BX; \
	CMOVLNE R12, R11; \
	MOVQ R11, S_SLACK(AX)

// Out of input or output: stop.
#define CHECK_LIMITS(done) \
	CMPQ SI, 0(SP); \
	JHI done; \
	CMPQ DI, 8(SP); \
	JHI done

// GOAMD64=v1 loop: shifts use CL.

// >= 56 bits; clobbers CX, R15.
#define REFILL_V1 \
	MOVQ (SI), R15; \
	MOVL DX, CX; \
	SHLQ CL, R15; \
	ORQ R15, R8; \
	NOTL CX; \
	ANDL $63, CX; \
	SHRL $3, CX; \
	ADDQ CX, SI; \
	ORQ $56, DX

#define LOOKUP_LL_V1(r) \
	MOVL R8, r; \
	ANDL $0x7ff, r; \
	MOVL (R9)(r*4), r

#define LIT_V1(off) \
	MOVL CX, BX; \
	RORL $16, BX; \
	SHRQ CL, R8; \
	SUBQ CX, DX; \
	MOVB BX, off(DI)

// Resolves CX; clobbers AX, R12, R15.
#define LINK_LL_V1 \
	MOVL CX, AX; \
	SHRL $16, AX; \
	SHRL $8, CX; \
	MOVL $1, R12; \
	SHLL CL, R12; \
	DECL R12; \
	MOVQ R8, R15; \
	SHRQ $11, R15; \
	ANDL R12, R15; \
	ADDL R15, AX; \
	MOVL (R9)(AX*4), CX

// Resolves dist CX; clobbers R13-R15.
#define LINK_DIST_V1 \
	MOVL CX, R13; \
	SHRL $16, R13; \
	SHRL $8, CX; \
	MOVL $1, R14; \
	SHLL CL, R14; \
	DECL R14; \
	MOVQ R8, R15; \
	SHRQ $8, R15; \
	ANDL R14, R15; \
	ADDL R15, R13; \
	MOVL (R10)(R13*4), CX

// func fastLoopV1(s *fastState)
TEXT ·fastLoopV1(SB), NOSPLIT, $24-8
	LOAD_STATE
	CHECK_LIMITS(v1done)
	REFILL_V1
	LOOKUP_LL_V1(CX)
	PCALIGN $32

v1decode:
	// CX may still be a link.
	TESTB $0x80, CL
	JZ    v1nonlit1
	LIT_V1(0)
	LOOKUP_LL_V1(CX)

v1lit2:
	TESTB $0x80, CL
	JZ    v1nonlit2
	LIT_V1(1)
	LOOKUP_LL_V1(CX)

v1lit3:
	TESTB $0x80, CL
	JZ    v1nonlit3
	LIT_V1(2)
	ADDQ $3, DI
	// Lookup needs 11 bits; links later.
	LOOKUP_LL_V1(R11)
	CHECK_LIMITS(v1done)
	REFILL_V1
	MOVL R11, CX
	JMP  v1decode

v1nonlit1:
	TESTB $0x20, CL
	JZ    v1match
	LINK_LL_V1
	JMP   v1decode

v1nonlit2:
	INCQ  DI
	TESTB $0x20, CL
	JZ    v1matchRefill
	DECQ  DI
	LINK_LL_V1
	JMP   v1lit2

v1nonlit3:
	ADDQ  $2, DI
	TESTB $0x20, CL
	JZ    v1matchRefill
	SUBQ  $2, DI
	LINK_LL_V1
	JMP   v1lit3

v1matchRefill:
	MOVL CX, R11
	REFILL_V1
	MOVL R11, CX

v1match:
	// CX: resolved non-literal; >= 56 bits.
	TESTB $0x40, CL
	JNZ   v1done
	MOVQ  R8, AX // rollback
	SHRQ  CL, R8
	MOVQ  R8, R11
	SHLQ  CL, R11
	XORQ  AX, R11 // code and extra bits
	MOVL  CX, R12
	SHRL  $8, CX
	ANDL  $15, CX
	SHRQ  CL, R11
	MOVL  R12, CX
	SHRL  $16, CX
	ADDL  CX, R11 // length
	MOVBLZX R8, CX
	MOVL  (R10)(CX*4), CX
	TESTB $0x20, CL
	JNZ   v1dlink

v1dlinked:
	TESTB $0x40, CL
	JNZ   v1rollback
	MOVL  CX, R14
	MOVQ  R8, R15
	SHRQ  CL, R8
	MOVQ  R8, R13
	SHLQ  CL, R13
	XORQ  R15, R13
	SHRL  $8, CX
	ANDL  $15, CX
	SHRQ  CL, R13
	MOVL  R14, CX
	SHRL  $16, CX
	ADDL  CX, R13 // dist
	MOVQ  DI, R15
	SUBQ  16(SP), R15
	CMPQ  R13, R15
	JHI   v1rollback

	// Commit.
	MOVWLZX R12, BX
	ANDL    $31, R14
	ADDL    R14, BX
	SUBQ    BX, DX

	// Preload first when 11 bits remain.
	CMPB BX, $45
	JHI  v1refillFirst
	LOOKUP_LL_V1(R14)
	REFILL_V1
	MOVL R14, CX

v1preloaded:
	MOVQ DI, AX
	SUBQ R13, AX
	LEAQ (DI)(R11*1), R12
	CMPQ R13, $16
	JCS  v1short
	MOVOU (AX), X0
	MOVOU X0, (DI)
	MOVOU 16(AX), X1
	MOVOU X1, 16(DI)
	CMPQ  R11, $32
	JLS   v1copied
	ADDQ  $32, AX
	ADDQ  $32, DI

v1copy32:
	MOVOU (AX), X0
	MOVOU X0, (DI)
	MOVOU 16(AX), X1
	MOVOU X1, 16(DI)
	ADDQ  $32, AX
	ADDQ  $32, DI
	CMPQ  DI, R12
	JCS   v1copy32

v1copied:
	MOVQ R12, DI
	CMPQ SI, 0(SP)
	JHI  v1done
	CMPQ DI, 8(SP)
	JLS  v1decode

v1done:
	STORE_STATE
	RET

v1rollback:
	MOVQ AX, R8
	JMP  v1done

v1dlink:
	LINK_DIST_V1
	JMP v1dlinked

v1refillFirst:
	REFILL_V1
	LOOKUP_LL_V1(CX)
	JMP v1preloaded

v1short:
	CMPQ R13, $1
	JNE  v1stride
	MOVBQZX (AX), R14
	MOVQ    $0x0101010101010101, R15
	IMULQ   R15, R14
	MOVQ    R14, X0
	PUNPCKLQDQ X0, X0

v1fill:
	MOVOU X0, (DI)
	MOVOU X0, 16(DI)
	ADDQ  $32, DI
	CMPQ  DI, R12
	JCS   v1fill
	JMP   v1copied

v1stride:
	// Loads hit the last store: forwards.
	MOVOU (AX), X0
	MOVOU X0, (DI)
	ADDQ  R13, AX
	ADDQ  R13, DI
	CMPQ  DI, R12
	JCS   v1stride
	JMP   v1copied

// BMI2 and SSSE3 loop.

// >= 56 bits; clobbers t.
#define REFILL_BMI2(t) \
	SHLXQ DX, (SI), t; \
	ORQ t, R8; \
	MOVL DX, t; \
	NOTL t; \
	ANDL $63, t; \
	SHRL $3, t; \
	ADDQ t, SI; \
	ORQ $56, DX

#define LOOKUP_LL_BMI2(src) \
	BZHIQ AX, src, CX; \
	MOVL (R9)(CX*4), CX

#define LIT_BMI2(off) \
	RORXL $16, CX, BX; \
	SHRXQ CX, R8, R8; \
	SUBQ CX, DX; \
	MOVB BX, off(DI)

// Resolves CX; clobbers R14, R15.
#define LINK_LL_BMI2 \
	MOVL CX, R14; \
	SHRL $16, R14; \
	SHRL $8, CX; \
	SHRXQ AX, R8, R15; \
	BZHIQ CX, R15, R15; \
	ADDL R15, R14; \
	MOVL (R9)(R14*4), CX

// func fastLoopBMI2(s *fastState)
TEXT ·fastLoopBMI2(SB), NOSPLIT, $24-8
	LOAD_STATE
	MOVL $11, AX
	CHECK_LIMITS(b2done)
	REFILL_BMI2(R15)
	LOOKUP_LL_BMI2(R8)
	PCALIGN $32

b2decode:
	TESTB $0x80, CL
	JZ    b2nonlit1
	LIT_BMI2(0)
	LOOKUP_LL_BMI2(R8)

b2lit2:
	TESTB $0x80, CL
	JZ    b2nonlit2
	LIT_BMI2(1)
	LOOKUP_LL_BMI2(R8)

b2lit3:
	TESTB $0x80, CL
	JZ    b2nonlit3
	LIT_BMI2(2)
	ADDQ $3, DI
	LOOKUP_LL_BMI2(R8)
	CHECK_LIMITS(b2done)
	REFILL_BMI2(R15)
	JMP  b2decode

b2nonlit2:
	INCQ  DI
	TESTB $0x60, CL
	JNZ   b2exc2
	REFILL_BMI2(R15)
	JMP   b2match

b2nonlit3:
	ADDQ  $2, DI
	TESTB $0x60, CL
	JNZ   b2exc3
	REFILL_BMI2(R15)
	JMP   b2match

b2nonlit1:
	TESTB $0x60, CL
	JNZ   b2exc1

b2match:
	// R8 unchanged until commit.
	BZHIQ CX, R8, R11 // code and extra bits
	SHRXQ CX, R8, R13
	MOVL  CX, R12
	SHRL  $8, R12
	ANDL  $15, R12
	SHRXQ R12, R11, R11
	MOVL  CX, R12
	SHRL  $16, R12
	ADDL  R12, R11 // length
	MOVBLZX R13, R14
	MOVL  (R10)(R14*4), R14
	TESTB $0x60, R14B
	JNZ   b2dexc

b2dlinked:
	BZHIQ R14, R13, R12
	SHRXQ R14, R13, R13
	MOVL  R14, R15
	SHRL  $8, R15
	ANDL  $15, R15
	SHRXQ R15, R12, R12
	MOVL  R14, R15
	SHRL  $16, R15
	ADDL  R15, R12 // dist
	MOVQ  DI, R15
	SUBQ  R12, R15 // src
	CMPQ  R15, 16(SP)
	JCS   b2done

	// Commit.
	MOVQ    R13, R8
	MOVWLZX CX, BX
	ANDL    $31, R14
	ADDL    R14, BX
	SUBQ    BX, DX

	// Preload first when 11 bits remain.
	CMPB BX, $45
	JHI  b2refillFirst
	LOOKUP_LL_BMI2(R13)
	REFILL_BMI2(R14)

b2preloaded:
	CMPQ  R12, $16
	JCS   b2short
	MOVOU (R15), X0
	MOVOU X0, (DI)
	MOVOU 16(R15), X1
	MOVOU X1, 16(DI)
	CMPQ  R11, $32
	JHI   b2long
	ADDQ  R11, DI

b2copied:
	CMPQ SI, 0(SP)
	JHI  b2done
	CMPQ DI, 8(SP)
	JLS  b2decode

b2done:
	STORE_STATE
	RET

b2long:
	LEAQ (DI)(R11*1), R14
	ADDQ $32, R15
	ADDQ $32, DI

b2copy32:
	MOVOU (R15), X0
	MOVOU X0, (DI)
	MOVOU 16(R15), X1
	MOVOU X1, 16(DI)
	ADDQ  $32, R15
	ADDQ  $32, DI
	CMPQ  DI, R14
	JCS   b2copy32
	MOVQ  R14, DI
	JMP   b2copied

b2short:
	// Repeat a dist-periodic pshufb pattern.
	LEAQ    (DI)(R11*1), R14
	CMPQ    R12, $1
	JEQ     b2run
	MOVOU   (R15), X0
	MOVQ    R12, R15
	SHLQ    $4, R15
	LEAQ    shufMasks<>(SB), R11
	MOVOU   (R11)(R15*1), X1
	PSHUFB  X1, X0
	LEAQ    shufAdv<>(SB), R11
	MOVBQZX (R11)(R12*1), R15

b2pattern:
	MOVOU X0, (DI)
	MOVOU X0, (DI)(R15*1)
	LEAQ  (DI)(R15*2), DI
	CMPQ  DI, R14
	JCS   b2pattern
	MOVQ  R14, DI
	JMP   b2copied

b2run:
	// Byte load forwards from last store.
	MOVBLZX (R15), R15
	MOVQ    R15, X0
	PXOR    X1, X1
	PSHUFB  X1, X0
	MOVL    $16, R15
	JMP     b2pattern

b2refillFirst:
	REFILL_BMI2(R14)
	LOOKUP_LL_BMI2(R8)
	JMP b2preloaded

	// Links and stops.
b2exc1:
	TESTB $0x40, CL
	JNZ   b2done
	LINK_LL_BMI2
	JMP   b2decode

b2exc2:
	TESTB $0x40, CL
	JNZ   b2done
	DECQ  DI
	LINK_LL_BMI2
	JMP   b2lit2

b2exc3:
	TESTB $0x40, CL
	JNZ   b2done
	SUBQ  $2, DI
	LINK_LL_BMI2
	JMP   b2lit3

b2dexc:
	TESTB $0x40, R14B
	JNZ   b2done
	MOVL  R14, R12
	SHRL  $16, R12
	SHRL  $8, R14
	MOVQ  R13, R15
	SHRQ  $8, R15
	BZHIQ R14, R15, R15
	ADDL  R15, R12
	MOVL  (R10)(R12*4), R14
	TESTB $0x40, R14B
	JNZ   b2done
	JMP   b2dlinked

// Pair loop: the bmi2 loop with R9 = pair
// table, litlen at 24(SP), outLimit-5
// for three pairs. Literal entries are
// primary or pairs (tables.go).

#define LOOKUP_LL_PAIR(src) \
	BZHIQ AX, src, CX; \
	MOVL (R9)(CX*4), CX

// A single stores junk at 1(DI).
#define LIT_PAIR \
	RORXL $16, CX, BX; \
	SHRXQ CX, R8, R8; \
	SUBQ CX, DX; \
	MOVW BX, (DI); \
	BTL $6, CX; \
	ADCQ $1, DI

// Resolves CX; clobbers R14, R15.
#define LINK_LL_PAIR \
	MOVL CX, R14; \
	SHRL $16, R14; \
	SHRL $8, CX; \
	SHRXQ AX, R8, R15; \
	BZHIQ CX, R15, R15; \
	ADDL R15, R14; \
	MOVQ 24(SP), R15; \
	MOVL (R15)(R14*4), CX

// STORE_STATE, but a literal's slack is
// pullN - n from entry bits 8-15.
#define STORE_STATE_PAIR \
	MOVQ s+0(FP), AX; \
	SUBQ S_IN(AX), SI; \
	MOVQ SI, S_INPOS(AX); \
	SUBQ S_OUT(AX), DI; \
	MOVQ DI, S_OUTPOS(AX); \
	MOVQ R8, S_BITBUF(AX); \
	ANDQ $63, DX; \
	MOVQ DX, S_BITSLEFT(AX); \
	XORL R13, R13; \
	MOVL BX, R11; \
	SHRL $12, R11; \
	ANDL $15, R11; \
	MOVBLZX BX, R12; \
	SUBL R12, R11; \
	CMOVLLT R13, R11; \
	MOVL BX, R12; \
	SHRL $28, R12; \
	MOVL BX, R13; \
	SHRL $24, R13; \
	ANDL $15, R13; \
	SUBL R13, R12; \
	TESTL $0x800000, BX; \
	CMOVLNE R12, R11; \
	MOVQ R11, S_SLACK(AX)

// func fastLoopBMI2Pair(s *fastState)
TEXT ·fastLoopBMI2Pair(SB), NOSPLIT, $32-8
	LOAD_STATE
	SUBQ $3, 8(SP)
	MOVQ R9, 24(SP)
	MOVQ S_PAIR(AX), R9
	MOVL $11, AX
	CHECK_LIMITS(p2done)
	REFILL_BMI2(R15)
	LOOKUP_LL_PAIR(R8)
	PCALIGN $32

p2decode:
	TESTB $0x80, CL
	JZ    p2nonlit1
	LIT_PAIR
	LOOKUP_LL_PAIR(R8)

p2lit2:
	TESTB $0x80, CL
	JZ    p2nonlit2
	LIT_PAIR
	LOOKUP_LL_PAIR(R8)

p2lit3:
	TESTB $0x80, CL
	JZ    p2nonlit3
	LIT_PAIR
	LOOKUP_LL_PAIR(R8)
	CHECK_LIMITS(p2done)
	REFILL_BMI2(R15)
	JMP  p2decode

p2nonlit2:
	TESTB $0x60, CL
	JNZ   p2exc2
	REFILL_BMI2(R15)
	JMP   p2match

p2nonlit3:
	TESTB $0x60, CL
	JNZ   p2exc3
	REFILL_BMI2(R15)
	JMP   p2match

p2nonlit1:
	TESTB $0x60, CL
	JNZ   p2exc1

p2match:
	// R8 unchanged until commit.
	BZHIQ CX, R8, R11 // code and extra bits
	SHRXQ CX, R8, R13
	MOVL  CX, R12
	SHRL  $8, R12
	ANDL  $15, R12
	SHRXQ R12, R11, R11
	MOVL  CX, R12
	SHRL  $16, R12
	ADDL  R12, R11 // length
	MOVBLZX R13, R14
	MOVL  (R10)(R14*4), R14
	TESTB $0x60, R14B
	JNZ   p2dexc

p2dlinked:
	BZHIQ R14, R13, R12
	SHRXQ R14, R13, R13
	MOVL  R14, R15
	SHRL  $8, R15
	ANDL  $15, R15
	SHRXQ R15, R12, R12
	MOVL  R14, R15
	SHRL  $16, R15
	ADDL  R15, R12 // dist
	MOVQ  DI, R15
	SUBQ  R12, R15 // src
	CMPQ  R15, 16(SP)
	JCS   p2done

	// Commit.
	MOVQ    R13, R8
	MOVWLZX CX, BX
	ANDL    $31, R14
	ADDL    R14, BX
	SUBQ    BX, DX

	// Preload first when 11 bits remain.
	CMPB BX, $45
	JHI  p2refillFirst
	LOOKUP_LL_PAIR(R13)
	REFILL_BMI2(R14)

p2preloaded:
	CMPQ  R12, $16
	JCS   p2short
	MOVOU (R15), X0
	MOVOU X0, (DI)
	MOVOU 16(R15), X1
	MOVOU X1, 16(DI)
	CMPQ  R11, $32
	JHI   p2long
	ADDQ  R11, DI

p2copied:
	CMPQ SI, 0(SP)
	JHI  p2done
	CMPQ DI, 8(SP)
	JLS  p2decode

p2done:
	STORE_STATE_PAIR
	RET

p2long:
	LEAQ (DI)(R11*1), R14
	ADDQ $32, R15
	ADDQ $32, DI

p2copy32:
	MOVOU (R15), X0
	MOVOU X0, (DI)
	MOVOU 16(R15), X1
	MOVOU X1, 16(DI)
	ADDQ  $32, R15
	ADDQ  $32, DI
	CMPQ  DI, R14
	JCS   p2copy32
	MOVQ  R14, DI
	JMP   p2copied

p2short:
	// Repeat a dist-periodic pshufb pattern.
	LEAQ    (DI)(R11*1), R14
	CMPQ    R12, $1
	JEQ     p2run
	MOVOU   (R15), X0
	MOVQ    R12, R15
	SHLQ    $4, R15
	LEAQ    shufMasks<>(SB), R11
	MOVOU   (R11)(R15*1), X1
	PSHUFB  X1, X0
	LEAQ    shufAdv<>(SB), R11
	MOVBQZX (R11)(R12*1), R15

p2pattern:
	MOVOU X0, (DI)
	MOVOU X0, (DI)(R15*1)
	LEAQ  (DI)(R15*2), DI
	CMPQ  DI, R14
	JCS   p2pattern
	MOVQ  R14, DI
	JMP   p2copied

p2run:
	// Byte load forwards from last store.
	MOVBLZX (R15), R15
	MOVQ    R15, X0
	PXOR    X1, X1
	PSHUFB  X1, X0
	MOVL    $16, R15
	JMP     p2pattern

p2refillFirst:
	REFILL_BMI2(R14)
	LOOKUP_LL_PAIR(R8)
	JMP p2preloaded

	// Links and stops.
p2exc1:
	TESTB $0x40, CL
	JNZ   p2done
	LINK_LL_PAIR
	JMP   p2decode

p2exc2:
	TESTB $0x40, CL
	JNZ   p2done
	LINK_LL_PAIR
	JMP   p2lit2

p2exc3:
	TESTB $0x40, CL
	JNZ   p2done
	LINK_LL_PAIR
	JMP   p2lit3

p2dexc:
	TESTB $0x40, R14B
	JNZ   p2done
	MOVL  R14, R12
	SHRL  $16, R12
	SHRL  $8, R14
	MOVQ  R13, R15
	SHRQ  $8, R15
	BZHIQ R14, R15, R15
	ADDL  R15, R12
	MOVL  (R10)(R12*4), R14
	TESTB $0x40, R14B
	JNZ   p2done
	JMP   p2dlinked

// Row d, byte j holds j%d.
DATA shufMasks<>+0(SB)/8, $0x0000000000000000
DATA shufMasks<>+8(SB)/8, $0x0000000000000000
DATA shufMasks<>+16(SB)/8, $0x0000000000000000
DATA shufMasks<>+24(SB)/8, $0x0000000000000000
DATA shufMasks<>+32(SB)/8, $0x0100010001000100
DATA shufMasks<>+40(SB)/8, $0x0100010001000100
DATA shufMasks<>+48(SB)/8, $0x0100020100020100
DATA shufMasks<>+56(SB)/8, $0x0002010002010002
DATA shufMasks<>+64(SB)/8, $0x0302010003020100
DATA shufMasks<>+72(SB)/8, $0x0302010003020100
DATA shufMasks<>+80(SB)/8, $0x0201000403020100
DATA shufMasks<>+88(SB)/8, $0x0004030201000403
DATA shufMasks<>+96(SB)/8, $0x0100050403020100
DATA shufMasks<>+104(SB)/8, $0x0302010005040302
DATA shufMasks<>+112(SB)/8, $0x0006050403020100
DATA shufMasks<>+120(SB)/8, $0x0100060504030201
DATA shufMasks<>+128(SB)/8, $0x0706050403020100
DATA shufMasks<>+136(SB)/8, $0x0706050403020100
DATA shufMasks<>+144(SB)/8, $0x0706050403020100
DATA shufMasks<>+152(SB)/8, $0x0605040302010008
DATA shufMasks<>+160(SB)/8, $0x0706050403020100
DATA shufMasks<>+168(SB)/8, $0x0504030201000908
DATA shufMasks<>+176(SB)/8, $0x0706050403020100
DATA shufMasks<>+184(SB)/8, $0x04030201000a0908
DATA shufMasks<>+192(SB)/8, $0x0706050403020100
DATA shufMasks<>+200(SB)/8, $0x030201000b0a0908
DATA shufMasks<>+208(SB)/8, $0x0706050403020100
DATA shufMasks<>+216(SB)/8, $0x0201000c0b0a0908
DATA shufMasks<>+224(SB)/8, $0x0706050403020100
DATA shufMasks<>+232(SB)/8, $0x01000d0c0b0a0908
DATA shufMasks<>+240(SB)/8, $0x0706050403020100
DATA shufMasks<>+248(SB)/8, $0x000e0d0c0b0a0908
GLOBL shufMasks<>(SB), RODATA|NOPTR, $256

// shufAdv[d] = 16 - 16%d.
DATA shufAdv<>+0(SB)/8, $0x0e0c0f100f101010
DATA shufAdv<>+8(SB)/8, $0x0f0e0d0c0b0a0910
GLOBL shufAdv<>(SB), RODATA|NOPTR, $16

// func hasBMI2() bool
TEXT ·hasBMI2(SB), NOSPLIT, $0-1
	XORL AX, AX
	XORL CX, CX
	CPUID
	CMPL AX, $7
	JCS  nobmi2
	MOVL $1, AX
	XORL CX, CX
	CPUID
	MOVL CX, R8 // bit 9: SSSE3
	MOVL $7, AX
	XORL CX, CX
	CPUID
	SHRL $9, R8
	SHRL $8, BX // bit 8: BMI2
	ANDL R8, BX
	ANDL $1, BX
	MOVB BX, ret+0(FP)
	RET

nobmi2:
	MOVB $0, ret+0(FP)
	RET
