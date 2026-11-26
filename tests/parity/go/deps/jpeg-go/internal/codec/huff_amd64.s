//go:build !purego

#include "go_asm.h"
#include "textflag.h"

// BX left-aligned bit buffer, R9 its bits.
// SI data, R8 len, DI pos.
// R10/R11 DC/AC table, R12 block or 0.
// R13 k, R14 natural order.
// 0(SP) block, 8(SP) component, 32(SP) DC.

DATA ones<>+0(SB)/8, $0x0101010101010101
GLOBL ones<>(SB), RODATA|NOPTR, $8
DATA highs<>+0(SB)/8, $0x8080808080808080
GLOBL highs<>(SB), RODATA|NOPTR, $8

// REFILL is FILL_BIT_BUFFER_FAST (jdhuff.c:414-417).
#define REFILL(slow, merge) \
	LEAQ  8(DI), AX \
	CMPQ  AX, R8 \
	JHI   slow \
	MOVQ  (SI)(DI*1), AX \
	BSWAPQ AX \
	MOVQ  AX, DX \
	NOTQ  DX \
	ORQ   $0xffff, DX \
	MOVQ  DX, CX \
	SUBQ  ones<>(SB), CX \
	NOTQ  DX \
	ANDQ  DX, CX \
	ANDQ  highs<>(SB), CX \
	JNZ   slow \
	SHRQ  $16, AX \
	ADDQ  $6, DI \
merge: \
	MOVQ  $16, CX \
	SUBQ  R9, CX \
	SHLXQ CX, AX, AX \
	ORQ   AX, BX \
	ADDQ  $48, R9

// REFILLSLOW is jdhuff.c's GET_BYTE, six times.
#define REFILLSLOW(slow, loop, plain, mk, next, merge) \
slow: \
	MOVQ  R13, 16(SP) \
	XORQ  AX, AX \
	MOVQ  $6, CX \
loop: \
	SHLQ  $8, AX \
	LEAQ  1(DI), DX \
	CMPQ  DX, R8 \
	JGE   mk \
	MOVBQZX (SI)(DI*1), DX \
	CMPQ  DX, $0xff \
	JNE   plain \
	MOVBQZX 1(SI)(DI*1), R13 \
	TESTQ R13, R13 \
	JNZ   mk \
	ORQ   $0xff, AX \
	ADDQ  $2, DI \
	JMP   next \
plain: \
	ORQ   DX, AX \
	INCQ  DI \
	JMP   next \
mk: \
	MOVB  $0, ret+64(FP) \
next: \
	DECQ  CX \
	JNZ   loop \
	MOVQ  16(SP), R13 \
	JMP   merge

// VALUE reads a CX-bit value, HUFF_EXTENDed.
#define VALUE(tmp) \
	MOVQ  CX, AX \
	NEGQ  AX \
	SHRXQ AX, BX, DX \
	MOVQ  BX, tmp \
	SARQ  $63, tmp \
	NOTQ  tmp \
	SHRXQ AX, tmp, tmp \
	SUBQ  tmp, DX \
	SHLXQ CX, BX, BX \
	SUBQ  CX, R9

// SLOWHUFF is jpeg_huff_decode past the lookahead.
#define SLOWHUFF(tbl, loop, found, bad, out) \
	MOVQ  $9, CX \
loop: \
	MOVQ  CX, AX \
	NEGQ  AX \
	SHRXQ AX, BX, AX \
	MOVLQSX derivedTable_maxcode(tbl)(CX*4), DX \
	CMPQ  AX, DX \
	JLE   found \
	INCQ  CX \
	JMP   loop \
found: \
	SHLXQ CX, BX, BX \
	SUBQ  CX, R9 \
	XORQ  DX, DX \
	CMPQ  CX, $16 \
	JGT   bad \
	MOVLQSX derivedTable_valoffset(tbl)(CX*4), DX \
	ADDQ  AX, DX \
	ANDQ  $0xff, DX \
	MOVQ  derivedTable_values(tbl), AX \
	MOVBQZX (AX)(DX*1), DX \
	JMP   out \
bad: \
	MOVQ  r+0(FP), AX \
	MOVB  $1, bitReader_lost(AX) \
	JMP   out

// func decodeMCUAsm(r *bitReader, dc, ac *[4]derivedTable, lastDC *[4]int32, blks *[maxMCUBlocks]*Block, ks *[maxMCUBlocks]int, n int, hasAC *[maxMCUBlocks]bool) bool
TEXT ·decodeMCUAsm(SB), NOSPLIT, $48-65
	MOVQ  r+0(FP), AX
	MOVQ  bitReader_data(AX), SI
	MOVQ  (bitReader_data+8)(AX), R8
	MOVQ  bitReader_pos(AX), DI
	MOVQ  bitReader_bitsLeft(AX), R9
	MOVQ  bitReader_buf(AX), BX
	MOVQ  $64, CX
	SUBQ  R9, CX
	SHLXQ CX, BX, BX
	TESTQ R9, R9
	JNZ   2(PC)
	XORQ  BX, BX
	MOVQ  lastDC+24(FP), AX
	MOVQ  (AX), CX
	MOVQ  CX, 32(SP)
	MOVQ  8(AX), CX
	MOVQ  CX, 40(SP)
	MOVB  $1, ret+64(FP)
	LEAQ  ·naturalOrderFast(SB), R14
	MOVQ  $0, 0(SP)

block:
	MOVQ  0(SP), AX
	CMPQ  AX, n+48(FP)
	JGE   finish
	MOVQ  ks+40(FP), DX
	MOVQ  (DX)(AX*8), DX
	ANDQ  $3, DX
	MOVQ  DX, 8(SP)
	IMUL3Q $derivedTable__size, DX, CX
	MOVQ  dc+8(FP), R10
	ADDQ  CX, R10
	MOVQ  ac+16(FP), R11
	ADDQ  CX, R11
	XORQ  R12, R12
	MOVQ  blks+32(FP), CX
	TESTQ CX, CX
	JZ    dc1
	MOVQ  (CX)(AX*8), R12

dc1:
	CMPQ  R9, $16
	JGT   dc2
	REFILL(dcs1, dcm1)
dc2:
	MOVQ  BX, AX
	SHRQ  $56, AX
	MOVL  derivedTable_combined(R10)(AX*4), DX
	TESTL DX, DX
	JZ    dclookup
	MOVL  DX, CX
	SHRL  $8, CX
	ANDL  $15, CX
	MOVQ  R9, AX
	SUBQ  CX, AX
	SHLXQ DX, BX, BX
	MOVL  DX, CX
	ANDL  $63, CX
	SUBQ  CX, R9
	SARL  $16, DX
	CMPQ  AX, $16
	JGT   dchave
	MOVQ  DX, 24(SP)
	REFILL(dcs2, dcm2)
	MOVQ  24(SP), DX
	JMP   dchave

dclookup:
	MOVWQZX derivedTable_lookup(R10)(AX*2), DX
	MOVQ  DX, CX
	SHRQ  $8, CX
	JZ    dcslow
	SHLXQ CX, BX, BX
	SUBQ  CX, R9
	ANDQ  $0xff, DX
dcsym:
	TESTQ DX, DX
	JZ    dchave
	MOVQ  DX, CX
	CMPQ  R9, $16
	JGT   dcval
	MOVQ  DX, 24(SP)
	REFILL(dcs3, dcm3)
	MOVQ  24(SP), CX
dcval:
	VALUE(R13)

dchave:
	MOVQ  8(SP), CX
	ADDL  32(SP)(CX*4), DX
	MOVL  DX, 32(SP)(CX*4)
	TESTQ R12, R12
	JZ    2(PC)
	MOVW  DX, (R12)
	MOVQ  $1, R13

acloop:
	CMPQ  R9, $16
	JGT   ac2
	REFILL(acs1, acm1)
ac2:
	MOVQ  BX, AX
	SHRQ  $56, AX
	MOVL  derivedTable_combined(R11)(AX*4), DX
	TESTL DX, DX
	JZ    aclookup
	SHLXQ DX, BX, BX
	MOVL  DX, CX
	SHRL  $8, CX
	ANDL  $15, CX
	MOVQ  R9, AX
	SUBQ  CX, AX
	MOVL  DX, CX
	ANDL  $63, CX
	SUBQ  CX, R9
	MOVL  DX, CX
	SHRL  $12, CX
	ANDL  $15, CX
	ADDQ  CX, R13
	TESTQ R12, R12
	JZ    acnostore
	SARL  $16, DX
	MOVBQZX (R14)(R13*1), CX
	MOVW  DX, (R12)(CX*2)
acnostore:
	CMPQ  AX, $16
	JLE   acfill
acnext:
	INCQ  R13
	CMPQ  R13, $64
	JLT   acloop
	JMP   blockdone

acfill:
	REFILL(acs2, acm2)
	JMP   acnext

aclookup:
	MOVWQZX derivedTable_lookup(R11)(AX*2), DX
	MOVQ  DX, CX
	SHRQ  $8, CX
	JZ    acslow
	SHLXQ CX, BX, BX
	SUBQ  CX, R9
	ANDQ  $0xff, DX
acrs:
	MOVQ  DX, CX
	ANDQ  $15, CX
	JZ    acsize0
	SHRQ  $4, DX
	ADDQ  DX, R13
	CMPQ  R9, $16
	JGT   acval
	MOVQ  CX, 24(SP)
	REFILL(acs3, acm3)
	MOVQ  24(SP), CX
acval:
	TESTQ R12, R12
	JZ    acdrop
	VALUE(R10)
	MOVBQZX (R14)(R13*1), CX
	MOVW  DX, (R12)(CX*2)
	JMP   acnext
acdrop:
	SHLXQ CX, BX, BX
	SUBQ  CX, R9
	JMP   acnext
acsize0:
	SHRQ  $4, DX
	CMPQ  DX, $15
	JNE   blockdone
	ADDQ  $15, R13
	JMP   acnext

blockdone:
	TESTQ R12, R12
	JZ    blocknext
	MOVQ  hasAC+56(FP), AX
	MOVQ  0(SP), CX
	CMPQ  R13, $1
	SETNE (AX)(CX*1)
blocknext:
	INCQ  0(SP)
	JMP   block

finish:
	CMPB  ret+64(FP), $0
	JEQ   done
	MOVQ  r+0(FP), AX
	MOVQ  DI, DX
	SUBQ  bitReader_pos(AX), DX
	ADDQ  DX, bitReader_decoded(AX)
	MOVQ  DI, bitReader_pos(AX)
	MOVQ  R9, bitReader_bitsLeft(AX)
	MOVQ  $64, CX
	SUBQ  R9, CX
	SHRXQ CX, BX, BX
	TESTQ R9, R9
	JNZ   2(PC)
	XORQ  BX, BX
	MOVQ  BX, bitReader_buf(AX)
	MOVQ  lastDC+24(FP), AX
	MOVQ  32(SP), CX
	MOVQ  CX, (AX)
	MOVQ  40(SP), CX
	MOVQ  CX, 8(AX)
done:
	RET

dcslow:
	SLOWHUFF(R10, dcsl, dcsf, dcsb, dcsym)
acslow:
	SLOWHUFF(R11, acsl, acsf, acsb, acrs)

	REFILLSLOW(dcs1, dcl1, dcp1, dck1, dcn1, dcm1)
	REFILLSLOW(dcs2, dcl2, dcp2, dck2, dcn2, dcm2)
	REFILLSLOW(dcs3, dcl3, dcp3, dck3, dcn3, dcm3)
	REFILLSLOW(acs1, acl1, acp1, ack1, acn1, acm1)
	REFILLSLOW(acs2, acl2, acp2, ack2, acn2, acm2)
	REFILLSLOW(acs3, acl3, acp3, ack3, acn3, acm3)
