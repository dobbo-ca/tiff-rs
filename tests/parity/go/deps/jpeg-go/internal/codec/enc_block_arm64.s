//go:build !purego

#include "go_asm.h"
#include "textflag.h"

// R0 bw, R5 acc, R6 free.
// R7 out ptr, R8 out base.
// R9 AC mask: natural order, then RBIT, <<1
// for acloop (bit 63 = k1). R10 last k.
// R19 vals, R20 nb, R21 0x0101...
// R22 next k, R23 zigzag.
// PUT: R11 code, R12 size (jchuff.c).
// Clobbers R15-R16.
#define PUT(sp, dn) \
	SUBS  R12, R6, R6 \
	BLT   sp \
	LSL   R12, R5, R5 \
	ORR   R11, R5, R5 \
dn:

// Out of line, PUT falls through.
#define SPILL(sp, st, sl, dn) \
sp: \
	ADD   R6, R12, R15 \
	LSL   R15, R5, R5 \
	NEG   R6, R15 \
	LSR   R15, R11, R15 \
	ORR   R15, R5, R5 \
	MVN   R5, R15 \
	SUB   R21, R15, R16 \
	BIC   R15, R16, R16 \
	TST   $0x8080808080808080, R16 \
	BNE   st \
	REV   R5, R15 \
	MOVD.P R15, 8(R7) \
	MOVD  R11, R5 \
	ADD   $64, R6, R6 \
	B     dn \
st: \
	MOVD  $8, R16 \
sl: \
	ROR   $56, R5, R5 \
	MOVB.P R5, 1(R7) \
	MOVB  ZR, (R7) \
	AND   $0xff, R5, R15 \
	CMP   $0xff, R15 \
	CINC  EQ, R7, R7 \
	SUBS  $1, R16, R16 \
	BNE   sl \
	MOVD  R11, R5 \
	ADD   $64, R6, R6 \
	B     dn

// func writeBlockAsm(bw *bitWriter, blk *[64]int16, lastDC *int32, dc, ac *[256]HuffCode)
TEXT ·writeBlockAsm(SB), NOSPLIT, $0-40
	MOVD  bw+0(FP), R4
	MOVD  blk+8(FP), R0
	ADD   $bitWriter_vals, R4, R1
	ADD   $bitWriter_nb, R4, R2
	MOVD  $·huffPrepConst(SB), R3

	// Natural prep, then zigzag nb.
	VLD1.P 64(R0), [V0.H8, V1.H8, V2.H8, V3.H8]
	VLD1   (R0), [V4.H8, V5.H8, V6.H8, V7.H8]

	// No AC: skip the prep, vals and nb unused.
	VEOR   V9.B16, V9.B16, V9.B16
	VMOV   V0.B16, V8.B16
	VMOV   V9.H[0], V8.H[0]
	VORR   V3.B16, V2.B16, V10.B16
	VORR   V5.B16, V4.B16, V11.B16
	VORR   V7.B16, V6.B16, V12.B16
	VORR   V1.B16, V8.B16, V8.B16
	VORR   V12.B16, V11.B16, V11.B16
	VORR   V10.B16, V8.B16, V8.B16
	VORR   V11.B16, V8.B16, V8.B16
	VMOV   V8.D[0], R9
	VMOV   V8.D[1], R10
	ORR    R10, R9, R9
	CBZ    R9, prepped

	MOVD   $·wbZigzag(SB), R10
	VLD1   (R10), [V24.B16, V25.B16, V26.B16, V27.B16]
	ADD    $128, R3, R3
	VLD1   (R3), [V31.B16]
	VMOVI  $16, V30.B16

	// nb = 16 - clz|v|, then zigzag.
	WORD   $0x4e60b808 // ABS V8.8H, V0.8H
	WORD   $0x4e60b829 // ABS V9.8H, V1.8H
	WORD   $0x4e60b84a // ABS V10.8H, V2.8H
	WORD   $0x4e60b86b // ABS V11.8H, V3.8H
	WORD   $0x4e60b88c // ABS V12.8H, V4.8H
	WORD   $0x4e60b8ad // ABS V13.8H, V5.8H
	WORD   $0x4e60b8ce // ABS V14.8H, V6.8H
	WORD   $0x4e60b8ef // ABS V15.8H, V7.8H
	WORD   $0x6e604908 // CLZ V8.8H, V8.8H
	WORD   $0x6e604929 // CLZ V9.8H, V9.8H
	WORD   $0x6e60494a // CLZ V10.8H, V10.8H
	WORD   $0x6e60496b // CLZ V11.8H, V11.8H
	WORD   $0x6e60498c // CLZ V12.8H, V12.8H
	WORD   $0x6e6049ad // CLZ V13.8H, V13.8H
	WORD   $0x6e6049ce // CLZ V14.8H, V14.8H
	WORD   $0x6e6049ef // CLZ V15.8H, V15.8H
	VUZP1  V9.B16, V8.B16, V16.B16
	VUZP1  V11.B16, V10.B16, V17.B16
	VUZP1  V13.B16, V12.B16, V18.B16
	VUZP1  V15.B16, V14.B16, V19.B16
	VSUB   V16.B16, V30.B16, V16.B16
	VSUB   V17.B16, V30.B16, V17.B16
	VSUB   V18.B16, V30.B16, V18.B16
	VSUB   V19.B16, V30.B16, V19.B16
	VTBL   V24.B16, [V16.B16, V17.B16, V18.B16, V19.B16], V20.B16
	VTBL   V25.B16, [V16.B16, V17.B16, V18.B16, V19.B16], V21.B16
	VTBL   V26.B16, [V16.B16, V17.B16, V18.B16, V19.B16], V22.B16
	VTBL   V27.B16, [V16.B16, V17.B16, V18.B16, V19.B16], V23.B16
	VST1   [V20.B16, V21.B16, V22.B16, V23.B16], (R2)
	VCMTST V20.B16, V20.B16, V20.B16
	VCMTST V21.B16, V21.B16, V21.B16
	VCMTST V22.B16, V22.B16, V22.B16
	VCMTST V23.B16, V23.B16, V23.B16
	VAND   V31.B16, V20.B16, V20.B16
	VAND   V31.B16, V21.B16, V21.B16
	VAND   V31.B16, V22.B16, V22.B16
	VAND   V31.B16, V23.B16, V23.B16
	VADDP  V21.B16, V20.B16, V20.B16
	VADDP  V23.B16, V22.B16, V22.B16
	VADDP  V22.B16, V20.B16, V20.B16
	VADDP  V20.B16, V20.B16, V20.B16
	VMOV   V20.D[0], R9
	AND    $0xfffffffffffffffe, R9, R9

	// vals, natural order.
	VUSHR  $15, V0.H8, V8.H8
	VUSHR  $15, V1.H8, V9.H8
	VUSHR  $15, V2.H8, V10.H8
	VUSHR  $15, V3.H8, V11.H8
	VUSHR  $15, V4.H8, V12.H8
	VUSHR  $15, V5.H8, V13.H8
	VUSHR  $15, V6.H8, V14.H8
	VUSHR  $15, V7.H8, V15.H8
	VSUB   V8.H8, V0.H8, V8.H8
	VSUB   V9.H8, V1.H8, V9.H8
	VSUB   V10.H8, V2.H8, V10.H8
	VSUB   V11.H8, V3.H8, V11.H8
	VSUB   V12.H8, V4.H8, V12.H8
	VSUB   V13.H8, V5.H8, V13.H8
	VSUB   V14.H8, V6.H8, V14.H8
	VSUB   V15.H8, V7.H8, V15.H8
	VST1.P [V8.H8, V9.H8, V10.H8, V11.H8], 64(R1)
	VST1   [V12.H8, V13.H8, V14.H8, V15.H8], (R1)

prepped:
	MOVD  bw+0(FP), R0
	MOVD  blk+8(FP), R1
	MOVD  lastDC+16(FP), R2
	MOVD  dc+24(FP), R3
	MOVD  ac+32(FP), R4
	MOVD  bitWriter_acc(R0), R5
	MOVD  bitWriter_free(R0), R6
	MOVD  bitWriter_out(R0), R8
	MOVD  (bitWriter_out+8)(R0), R7
	ADD   R8, R7, R7
	ADD   $bitWriter_vals, R0, R19
	ADD   $bitWriter_nb, R0, R20
	MOVD  $0x0101010101010101, R21
	MOVD  $·wbZigzag(SB), R23

	// DC: diff, category, value bits.
	MOVH  (R1), R13
	MOVW  (R2), R14
	MOVW  R13, (R2)
	SUBW  R14, R13, R13
	ASRW  $31, R13, R14
	EORW  R14, R13, R15
	SUBW  R14, R15, R15
	CLZW  R15, R15
	MOVD  $32, R16
	SUB   R15, R16, R15
	ADDW  R14, R13, R13
	NEG   R15, R16
	LSL   R16, R13, R13
	LSR   R16, R13, R13
	ADD   R15<<2, R3, R16
	MOVHU (R16), R11
	MOVBU 2(R16), R12
	LSL   R15, R11, R11
	ORR   R13, R11, R11
	ADD   R15, R12, R12
	PUT(dcsp, dcdn)

	MOVD  ZR, R10
	RBIT  R9, R9
	LSL   $1, R9, R9
	MOVD  $1, R22
	CBZ   R9, acend

acloop:
	CLZ   R9, R14
	LSL   $1, R9, R13
	ADD   R14, R22, R10
	LSL   R14, R13, R9
	ADD   $1, R10, R22
	CMP   $16, R14
	BGE   zrl

acsym:
	MOVBU (R20)(R10), R15
	MOVBU (R23)(R10), R13
	MOVHU (R19)(R13<<1), R17
	ORR   R14<<4, R15, R16
	ADD   R16<<2, R4, R16
	MOVHU (R16), R11
	MOVBU 2(R16), R12
	NEG   R15, R16
	LSL   R16, R17, R17
	LSR   R16, R17, R17
	LSL   R15, R11, R11
	ORR   R17, R11, R11
	ADD   R15, R12, R12
	PUT(asp, adn)
	CBNZ  R9, acloop

acend:
	CMP   $63, R10
	BEQ   done
	MOVHU (R4), R11
	MOVBU 2(R4), R12
	PUT(esp, edn)

done:
	MOVD  R5, bitWriter_acc(R0)
	MOVD  R6, bitWriter_free(R0)
	SUB   R8, R7, R7
	MOVD  R7, (bitWriter_out+8)(R0)
	RET

zrl:
	MOVHU 960(R4), R11
	MOVBU 962(R4), R12
	PUT(zsp, zdn)
	SUB   $16, R14, R14
	CMP   $16, R14
	BGE   zrl
	B     acsym

	SPILL(dcsp, dcst, dcsl, dcdn)
	SPILL(asp, ast, asl, adn)
	SPILL(esp, est, esl, edn)
	SPILL(zsp, zst, zsl, zdn)

// Natural index of each zigzag position.
DATA ·wbZigzag+0(SB)/8, $0x0a03020910080100
DATA ·wbZigzag+8(SB)/8, $0x05040b1219201811
DATA ·wbZigzag+16(SB)/8, $0x22293028211a130c
DATA ·wbZigzag+24(SB)/8, $0x1c150e07060d141b
DATA ·wbZigzag+32(SB)/8, $0x242b323938312a23
DATA ·wbZigzag+40(SB)/8, $0x332c251e170f161d
DATA ·wbZigzag+48(SB)/8, $0x2e271f262d343b3a
DATA ·wbZigzag+56(SB)/8, $0x3f3e372f363d3c35
GLOBL ·wbZigzag(SB), RODATA|NOPTR, $64
