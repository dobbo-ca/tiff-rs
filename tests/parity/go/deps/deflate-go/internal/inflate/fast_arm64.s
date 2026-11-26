//go:build !purego

#include "textflag.h"

// Assembly fastLoopGo, contract in fast_generic.go.
//
// Registers, whole loop:
//	R0 s           R1 in          R2 in+inLimit
//	R3 out         R4 out+outLimit-2 (two literals per check)
//	R5 out+histBase  R6 bitbuf    R7 bitsleft (low 6 bits)
//	R8 slack       R9 litlen      R10 dist
//	R11 next litlen entry, resolved
//	R19 -1         R22 pattIdx    R23 pattStep
//	R24, R25 next entry and bitbuf, matches
//
// Refill leaves 64 input bits, so
// lookups survive 49 consumed bits.
// Three refills at most per check.

// Refill whole bytes: bitsleft >= 56.
#define REFILL \
	MOVD	(R1), R12; \
	LSL	R7, R12, R12; \
	ORR	R12, R6, R6; \
	MVN	R7, R13; \
	UBFX	$3, R13, $3, R13; \
	ADD	R13, R1, R1; \
	ORR	$56, R7, R7

#define LOOKUP(link) \
	UBFIZ	$2, R6, $11, R12; \
	MOVWU	(R9)(R12), R11; \
	TBNZ	$5, R11, link

// Consume literal, preload next, store.
#define LITERAL \
	LSR	R11, R6, R6; \
	LSR	$16, R11, R14; \
	UBFIZ	$2, R6, $11, R12; \
	SUB	R11, R7, R7; \
	LSR	$24, R11, R8; \
	MOVWU	(R9)(R12), R11; \
	MOVB.P	R14, 1(R3)

// Resolve litlen link R11.
#define SUBLL(back) \
	UBFX	$8, R11, $4, R13; \
	LSR	$11, R6, R12; \
	LSL	R13, R19, R13; \
	BIC	R13, R12, R12; \
	ADD	R11>>16, R12, R12; \
	MOVWU	(R9)(R12<<2), R11; \
	B	back

// func fastLoopAsm(s *fastState)
TEXT ·fastLoopAsm(SB), NOSPLIT, $0-8
	MOVD	s+0(FP), R0
	MOVD	0(R0), R1
	MOVD	8(R0), R12
	MOVD	16(R0), R2
	ADD	R1, R2, R2
	ADD	R12, R1, R1
	MOVD	24(R0), R3
	MOVD	32(R0), R12
	MOVD	40(R0), R4
	ADD	R3, R4, R4
	SUB	$2, R4, R4
	MOVD	48(R0), R5
	ADD	R3, R5, R5
	ADD	R12, R3, R3
	// Not preemptible: stop within 1 MiB.
	ADD	$0x100000, R3, R12
	CMP	R12, R4
	CSEL	HI, R12, R4, R4
	MOVD	56(R0), R6
	MOVD	64(R0), R7
	MOVD	72(R0), R8
	MOVD	80(R0), R9
	MOVD	88(R0), R10
	MOVD	$-1, R19
	MOVD	$pattIdx<>(SB), R22
	MOVD	$pattStep<>(SB), R23

	CMP	R2, R1
	BHI	done
	CMP	R4, R3
	BHI	done
	REFILL
	LOOKUP(link0)
	PCALIGN	$32

top: // bitsleft >= 56
	TBZ	$7, R11, match
	LITERAL
	REFILL
	TBNZ	$5, R11, link1
lit1:
	TBZ	$7, R11, match
	LITERAL
	REFILL
	TBNZ	$5, R11, link2
lit2:
	TBZ	$7, R11, match
	LITERAL
	TBNZ	$5, R11, link3
	B	check
	PCALIGN	$32

match:
	TBNZ	$6, R11, done
	LSR	R11, R6, R15
	UBFIZ	$2, R15, $8, R12
	MOVWU	(R10)(R12), R16
	TBNZ	$5, R16, dlink

dback:
	// Speculative next state, kept on commit.
	LSR	R16, R15, R25
	UBFIZ	$2, R25, $11, R12
	MOVWU	(R9)(R12), R24
	TBNZ	$6, R16, done
	// One mask covers code and extra.
	LSL	R11, R19, R12
	BIC	R12, R6, R12
	UBFX	$8, R11, $4, R13
	LSR	R13, R12, R12
	ADD	R11>>16, R12, R14
	LSL	R16, R19, R12
	BIC	R12, R15, R12
	UBFX	$8, R16, $4, R13
	LSR	R13, R12, R12
	ADD	R16>>16, R12, R17
	SUB	R5, R3, R12
	CMP	R12, R17
	BHI	done

	// Commit; slack = max(0, pullN - used).
	MOVD	R25, R6
	AND	$31, R11, R12
	AND	$31, R16, R13
	ADD	R13, R12, R12
	SUB	R12, R7, R7
	UBFX	$12, R11, $4, R13
	SUBS	R12, R13, R13
	CSEL	LT, ZR, R13, R8
	MOVD	R24, R11
	TBNZ	$5, R11, link4

mback:
	SUB	R17, R3, R20
	ADD	R14, R3, R21
	// Check and refill before the copy.
	CMP	R2, R1
	BHI	last
	CMP	R4, R21
	BHI	last
	REFILL
	CMP	$16, R17
	BLO	pattern

copy32: // dist >= 16: 16-byte moves never overlap
	FMOVQ	(R20), F0
	FMOVQ	F0, (R3)
	FMOVQ	16(R20), F1
	FMOVQ	F1, 16(R3)
	ADD	$32, R20, R20
	ADD	$32, R3, R3
	CMP	R21, R3
	BHS	cdone

copy64: // writes stay within 288 bytes
	FMOVQ	(R20), F0
	FMOVQ	F0, (R3)
	FMOVQ	16(R20), F1
	FMOVQ	F1, 16(R3)
	FMOVQ	32(R20), F2
	FMOVQ	F2, 32(R3)
	FMOVQ	48(R20), F3
	FMOVQ	F3, 48(R3)
	ADD	$64, R20, R20
	ADD	$64, R3, R3
	CMP	R21, R3
	BLO	copy64

cdone:
	MOVD	R21, R3
	B	top

check:
	CMP	R2, R1
	BHI	done
	CMP	R4, R3
	BHI	done
	REFILL
	B	top

pattern: // dist < 16: TBL repeats dist bytes
	ADD	R17<<5, R22, R12
	VLD1	(R12), [V1.B16, V2.B16]
	FMOVQ	(R20), F0
	VTBL	V1.B16, [V0.B16], V3.B16
	VTBL	V2.B16, [V0.B16], V4.B16
	MOVBU	(R23)(R17), R13

	FSTPQ	(F3, F4), (R3)
	ADD	R13, R3, R3
	CMP	R21, R3
	BHS	pdone
	LSL	$1, R13, R15

patloop: // two steps per iteration
	FSTPQ	(F3, F4), (R3)
	ADD	R13, R3, R12
	FSTPQ	(F3, F4), (R12)
	ADD	R15, R3, R3
	CMP	R21, R3
	BLO	patloop

pdone:
	MOVD	R21, R3
	B	top

last: // margins low: exact copy, then stop
	MOVBU.P	1(R20), R12
	MOVB.P	R12, 1(R3)
	CMP	R21, R3
	BLO	last
	B	done

dlink:
	UBFX	$8, R16, $4, R13
	LSR	$8, R15, R12
	LSL	R13, R19, R13
	BIC	R13, R12, R12
	ADD	R16>>16, R12, R12
	MOVWU	(R10)(R12<<2), R16
	B	dback

link0:
	SUBLL(top)
link1:
	SUBLL(lit1)
link2:
	SUBLL(lit2)
link3:
	SUBLL(check)
link4:
	SUBLL(mback)

done:
	MOVD	0(R0), R12
	SUB	R12, R1, R12
	MOVD	R12, 8(R0)
	MOVD	24(R0), R12
	SUB	R12, R3, R12
	MOVD	R12, 32(R0)
	MOVD	R6, 56(R0)
	AND	$63, R7, R7
	MOVD	R7, 64(R0)
	MOVD	R8, 72(R0)
	RET

// Pair loop: fastLoopAsm with R26 = pair
// table for every primary lookup, R4 =
// out+outLimit-5 (three pairs per check),
// and R8 the last literal's entry until
// done. Entries: tables.go.

#define LOOKUP_P(link) \
	UBFIZ	$2, R6, $11, R12; \
	MOVWU	(R26)(R12), R11; \
	TBNZ	$5, R11, link

// A single stores junk at 1(R3).
#define LITERAL_P \
	LSR	R11, R6, R6; \
	LSR	$16, R11, R14; \
	UBFIZ	$2, R6, $11, R12; \
	SUB	R11, R7, R7; \
	MOVD	R11, R8; \
	UBFX	$6, R11, $1, R13; \
	MOVWU	(R26)(R12), R11; \
	MOVH.P	R14, 1(R3); \
	ADD	R13, R3, R3

// func fastLoopPairAsm(s *fastState)
TEXT ·fastLoopPairAsm(SB), NOSPLIT, $0-8
	MOVD	s+0(FP), R0
	MOVD	0(R0), R1
	MOVD	8(R0), R12
	MOVD	16(R0), R2
	ADD	R1, R2, R2
	ADD	R12, R1, R1
	MOVD	24(R0), R3
	MOVD	32(R0), R12
	MOVD	40(R0), R4
	ADD	R3, R4, R4
	SUB	$5, R4, R4
	MOVD	48(R0), R5
	ADD	R3, R5, R5
	ADD	R12, R3, R3
	// Not preemptible: stop within 1 MiB.
	ADD	$0x100000, R3, R12
	CMP	R12, R4
	CSEL	HI, R12, R4, R4
	MOVD	56(R0), R6
	MOVD	64(R0), R7
	MOVD	72(R0), R8
	MOVD	80(R0), R9
	MOVD	88(R0), R10
	MOVD	96(R0), R26
	MOVD	$-1, R19
	MOVD	$pattIdx<>(SB), R22
	MOVD	$pattStep<>(SB), R23

	CMP	R2, R1
	BHI	done
	CMP	R4, R3
	BHI	done
	REFILL
	LOOKUP_P(link0)
	PCALIGN	$32

top: // bitsleft >= 56
	TBZ	$7, R11, match
	LITERAL_P
	REFILL
	TBNZ	$5, R11, link1
lit1:
	TBZ	$7, R11, match
	LITERAL_P
	REFILL
	TBNZ	$5, R11, link2
lit2:
	TBZ	$7, R11, match
	LITERAL_P
	TBNZ	$5, R11, link3
	B	check
	PCALIGN	$32

match:
	TBNZ	$6, R11, done
	LSR	R11, R6, R15
	UBFIZ	$2, R15, $8, R12
	MOVWU	(R10)(R12), R16
	TBNZ	$5, R16, dlink

dback:
	// Speculative next state, kept on commit.
	LSR	R16, R15, R25
	UBFIZ	$2, R25, $11, R12
	MOVWU	(R26)(R12), R24
	TBNZ	$6, R16, done
	// One mask covers code and extra.
	LSL	R11, R19, R12
	BIC	R12, R6, R12
	UBFX	$8, R11, $4, R13
	LSR	R13, R12, R12
	ADD	R11>>16, R12, R14
	LSL	R16, R19, R12
	BIC	R12, R15, R12
	UBFX	$8, R16, $4, R13
	LSR	R13, R12, R12
	ADD	R16>>16, R12, R17
	SUB	R5, R3, R12
	CMP	R12, R17
	BHI	done

	// Commit; slack = max(0, pullN - used).
	MOVD	R25, R6
	AND	$31, R11, R12
	AND	$31, R16, R13
	ADD	R13, R12, R12
	SUB	R12, R7, R7
	UBFX	$12, R11, $4, R13
	SUBS	R12, R13, R13
	CSEL	LT, ZR, R13, R8
	MOVD	R24, R11
	TBNZ	$5, R11, link4

mback:
	SUB	R17, R3, R20
	ADD	R14, R3, R21
	// Check and refill before the copy.
	CMP	R2, R1
	BHI	last
	CMP	R4, R21
	BHI	last
	REFILL
	CMP	$16, R17
	BLO	pattern

copy32: // dist >= 16: 16-byte moves never overlap
	FMOVQ	(R20), F0
	FMOVQ	F0, (R3)
	FMOVQ	16(R20), F1
	FMOVQ	F1, 16(R3)
	ADD	$32, R20, R20
	ADD	$32, R3, R3
	CMP	R21, R3
	BHS	cdone

copy64: // writes stay within 288 bytes
	FMOVQ	(R20), F0
	FMOVQ	F0, (R3)
	FMOVQ	16(R20), F1
	FMOVQ	F1, 16(R3)
	FMOVQ	32(R20), F2
	FMOVQ	F2, 32(R3)
	FMOVQ	48(R20), F3
	FMOVQ	F3, 48(R3)
	ADD	$64, R20, R20
	ADD	$64, R3, R3
	CMP	R21, R3
	BLO	copy64

cdone:
	MOVD	R21, R3
	B	top

check:
	CMP	R2, R1
	BHI	done
	CMP	R4, R3
	BHI	done
	REFILL
	B	top

pattern: // dist < 16: TBL repeats dist bytes
	ADD	R17<<5, R22, R12
	VLD1	(R12), [V1.B16, V2.B16]
	FMOVQ	(R20), F0
	VTBL	V1.B16, [V0.B16], V3.B16
	VTBL	V2.B16, [V0.B16], V4.B16
	MOVBU	(R23)(R17), R13

	FSTPQ	(F3, F4), (R3)
	ADD	R13, R3, R3
	CMP	R21, R3
	BHS	pdone
	LSL	$1, R13, R15

patloop: // two steps per iteration
	FSTPQ	(F3, F4), (R3)
	ADD	R13, R3, R12
	FSTPQ	(F3, F4), (R12)
	ADD	R15, R3, R3
	CMP	R21, R3
	BLO	patloop

pdone:
	MOVD	R21, R3
	B	top

last: // margins low: exact copy, then stop
	MOVBU.P	1(R20), R12
	MOVB.P	R12, 1(R3)
	CMP	R21, R3
	BLO	last
	B	done

dlink:
	UBFX	$8, R16, $4, R13
	LSR	$8, R15, R12
	LSL	R13, R19, R13
	BIC	R13, R12, R12
	ADD	R16>>16, R12, R12
	MOVWU	(R10)(R12<<2), R16
	B	dback

link0:
	SUBLL(top)
link1:
	SUBLL(lit1)
link2:
	SUBLL(lit2)
link3:
	SUBLL(check)
link4:
	SUBLL(mback)

done:
	MOVD	0(R0), R12
	SUB	R12, R1, R12
	MOVD	R12, 8(R0)
	MOVD	24(R0), R12
	SUB	R12, R3, R12
	MOVD	R12, 32(R0)
	MOVD	R6, 56(R0)
	AND	$63, R7, R7
	MOVD	R7, 64(R0)
	TBZ	$7, R8, slack
	// Last literal entry: slack = pullN - n.
	UBFX	$12, R8, $4, R12
	UBFX	$8, R8, $4, R13
	SUB	R13, R12, R8

slack:
	MOVD	R8, 72(R0)
	RET

// TBL rows: byte i is i%d.
DATA pattIdx<>+0(SB)/8, $0x0000000000000000
DATA pattIdx<>+8(SB)/8, $0x0000000000000000
DATA pattIdx<>+16(SB)/8, $0x0000000000000000
DATA pattIdx<>+24(SB)/8, $0x0000000000000000
DATA pattIdx<>+32(SB)/8, $0x0000000000000000
DATA pattIdx<>+40(SB)/8, $0x0000000000000000
DATA pattIdx<>+48(SB)/8, $0x0000000000000000
DATA pattIdx<>+56(SB)/8, $0x0000000000000000
DATA pattIdx<>+64(SB)/8, $0x0100010001000100
DATA pattIdx<>+72(SB)/8, $0x0100010001000100
DATA pattIdx<>+80(SB)/8, $0x0100010001000100
DATA pattIdx<>+88(SB)/8, $0x0100010001000100
DATA pattIdx<>+96(SB)/8, $0x0100020100020100
DATA pattIdx<>+104(SB)/8, $0x0002010002010002
DATA pattIdx<>+112(SB)/8, $0x0201000201000201
DATA pattIdx<>+120(SB)/8, $0x0100020100020100
DATA pattIdx<>+128(SB)/8, $0x0302010003020100
DATA pattIdx<>+136(SB)/8, $0x0302010003020100
DATA pattIdx<>+144(SB)/8, $0x0302010003020100
DATA pattIdx<>+152(SB)/8, $0x0302010003020100
DATA pattIdx<>+160(SB)/8, $0x0201000403020100
DATA pattIdx<>+168(SB)/8, $0x0004030201000403
DATA pattIdx<>+176(SB)/8, $0x0302010004030201
DATA pattIdx<>+184(SB)/8, $0x0100040302010004
DATA pattIdx<>+192(SB)/8, $0x0100050403020100
DATA pattIdx<>+200(SB)/8, $0x0302010005040302
DATA pattIdx<>+208(SB)/8, $0x0504030201000504
DATA pattIdx<>+216(SB)/8, $0x0100050403020100
DATA pattIdx<>+224(SB)/8, $0x0006050403020100
DATA pattIdx<>+232(SB)/8, $0x0100060504030201
DATA pattIdx<>+240(SB)/8, $0x0201000605040302
DATA pattIdx<>+248(SB)/8, $0x0302010006050403
DATA pattIdx<>+256(SB)/8, $0x0706050403020100
DATA pattIdx<>+264(SB)/8, $0x0706050403020100
DATA pattIdx<>+272(SB)/8, $0x0706050403020100
DATA pattIdx<>+280(SB)/8, $0x0706050403020100
DATA pattIdx<>+288(SB)/8, $0x0706050403020100
DATA pattIdx<>+296(SB)/8, $0x0605040302010008
DATA pattIdx<>+304(SB)/8, $0x0504030201000807
DATA pattIdx<>+312(SB)/8, $0x0403020100080706
DATA pattIdx<>+320(SB)/8, $0x0706050403020100
DATA pattIdx<>+328(SB)/8, $0x0504030201000908
DATA pattIdx<>+336(SB)/8, $0x0302010009080706
DATA pattIdx<>+344(SB)/8, $0x0100090807060504
DATA pattIdx<>+352(SB)/8, $0x0706050403020100
DATA pattIdx<>+360(SB)/8, $0x04030201000a0908
DATA pattIdx<>+368(SB)/8, $0x01000a0908070605
DATA pattIdx<>+376(SB)/8, $0x0908070605040302
DATA pattIdx<>+384(SB)/8, $0x0706050403020100
DATA pattIdx<>+392(SB)/8, $0x030201000b0a0908
DATA pattIdx<>+400(SB)/8, $0x0b0a090807060504
DATA pattIdx<>+408(SB)/8, $0x0706050403020100
DATA pattIdx<>+416(SB)/8, $0x0706050403020100
DATA pattIdx<>+424(SB)/8, $0x0201000c0b0a0908
DATA pattIdx<>+432(SB)/8, $0x0a09080706050403
DATA pattIdx<>+440(SB)/8, $0x0504030201000c0b
DATA pattIdx<>+448(SB)/8, $0x0706050403020100
DATA pattIdx<>+456(SB)/8, $0x01000d0c0b0a0908
DATA pattIdx<>+464(SB)/8, $0x0908070605040302
DATA pattIdx<>+472(SB)/8, $0x030201000d0c0b0a
DATA pattIdx<>+480(SB)/8, $0x0706050403020100
DATA pattIdx<>+488(SB)/8, $0x000e0d0c0b0a0908
DATA pattIdx<>+496(SB)/8, $0x0807060504030201
DATA pattIdx<>+504(SB)/8, $0x01000e0d0c0b0a09
GLOBL pattIdx<>(SB), RODATA|NOPTR, $512

// 32 - 32%d: whole periods per store.
DATA pattStep<>+0(SB)/8, $0x1c1e1e201e202020
DATA pattStep<>+8(SB)/8, $0x1e1c1a18161e1b20
GLOBL pattStep<>(SB), RODATA|NOPTR, $16
