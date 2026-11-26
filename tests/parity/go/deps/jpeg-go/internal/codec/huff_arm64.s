//go:build !purego

#include "go_asm.h"
#include "textflag.h"

// R3 left-aligned bit buffer, R4 its bits.
// R0 data, R1 len, R2 pos.
// R5/R6 DC/AC table, R7 block or 0.
// R8 k, R9 natural order, R10 DC.
// R11 block, R19 component, R15 no marker.

// REFILL is FILL_BIT_BUFFER_FAST (jdhuff.c:414-417).
#define REFILL(slow, merge) \
	ADD   $8, R2, R22 \
	CMP   R1, R22 \
	BHI   slow \
	MOVD  (R0)(R2), R22 \
	REV   R22, R22 \
	MVN   R22, R23 \
	ORR   $0xffff, R23, R23 \
	SUB   R20, R23, R24 \
	BIC   R23, R24, R24 \
	AND   R21, R24, R24 \
	CBNZ  R24, slow \
	LSR   $16, R22, R22 \
	ADD   $6, R2, R2 \
merge: \
	MOVD  $16, R23 \
	SUB   R4, R23, R23 \
	LSL   R23, R22, R22 \
	ORR   R22, R3, R3 \
	ADD   $48, R4, R4

// REFILLSLOW is jdhuff.c's GET_BYTE, six times.
#define REFILLSLOW(slow, loop, plain, mk, next, merge) \
slow: \
	MOVD  $0, R22 \
	MOVD  $6, R25 \
loop: \
	LSL   $8, R22, R22 \
	ADD   $1, R2, R23 \
	CMP   R1, R23 \
	BHS   mk \
	MOVBU (R0)(R2), R23 \
	CMP   $0xff, R23 \
	BNE   plain \
	ADD   $1, R2, R24 \
	MOVBU (R0)(R24), R24 \
	CBNZ  R24, mk \
	ORR   $0xff, R22, R22 \
	ADD   $2, R2, R2 \
	B     next \
plain: \
	ORR   R23, R22, R22 \
	ADD   $1, R2, R2 \
	B     next \
mk: \
	MOVD  $0, R15 \
next: \
	SUBS  $1, R25, R25 \
	BNE   loop \
	B     merge

// VALUE reads an R25-bit value, HUFF_EXTENDed.
#define VALUE \
	NEG   R25, R23 \
	LSR   R23, R3, R24 \
	ASR   $63, R3, R22 \
	MVN   R22, R22 \
	LSR   R23, R22, R22 \
	SUB   R22, R24, R24 \
	LSL   R25, R3, R3 \
	SUB   R25, R4, R4

// SLOWHUFF is jpeg_huff_decode past the lookahead.
#define SLOWHUFF(tbl, loop, found, bad, out) \
	MOVD  $9, R25 \
loop: \
	NEG   R25, R23 \
	LSR   R23, R3, R24 \
	ADD   $derivedTable_maxcode, tbl, R23 \
	MOVW  (R23)(R25<<2), R23 \
	CMP   R23, R24 \
	BLE   found \
	ADD   $1, R25, R25 \
	B     loop \
found: \
	LSL   R25, R3, R3 \
	SUB   R25, R4, R4 \
	MOVD  $0, R22 \
	CMP   $16, R25 \
	BGT   bad \
	ADD   $derivedTable_valoffset, tbl, R23 \
	MOVW  (R23)(R25<<2), R23 \
	ADD   R24, R23, R23 \
	AND   $0xff, R23, R23 \
	MOVD  derivedTable_values(tbl), R24 \
	MOVBU (R24)(R23), R22 \
	B     out \
bad: \
	MOVD  r+0(FP), R23 \
	MOVD  $1, R24 \
	MOVB  R24, bitReader_lost(R23) \
	B     out

// func decodeMCUAsm(r *bitReader, dc, ac *[4]derivedTable, lastDC *[4]int32, blks *[maxMCUBlocks]*Block, ks *[maxMCUBlocks]int, n int, hasAC *[maxMCUBlocks]bool) bool
TEXT ·decodeMCUAsm(SB), NOSPLIT, $16-65
	MOVD  r+0(FP), R10
	MOVD  bitReader_data(R10), R0
	MOVD  (bitReader_data+8)(R10), R1
	MOVD  bitReader_pos(R10), R2
	MOVD  bitReader_bitsLeft(R10), R4
	MOVD  bitReader_buf(R10), R3
	MOVD  $64, R22
	SUB   R4, R22, R22
	LSL   R22, R3, R3
	CMP   $0, R4
	CSEL  EQ, ZR, R3, R3
	MOVD  lastDC+24(FP), R22
	MOVD  $dcs-16(SP), R10
	MOVD  (R22), R23
	MOVD  R23, (R10)
	MOVD  8(R22), R23
	MOVD  R23, 8(R10)
	MOVD  $1, R15
	MOVD  $·naturalOrderFast(SB), R9
	MOVD  $0x0101010101010101, R20
	MOVD  $0x8080808080808080, R21
	MOVD  blks+32(FP), R13
	MOVD  ks+40(FP), R14
	MOVD  $0, R11

block:
	MOVD  n+48(FP), R22
	CMP   R22, R11
	BGE   finish
	MOVD  (R14)(R11<<3), R19
	AND   $3, R19, R19
	MOVD  $derivedTable__size, R22
	MUL   R22, R19, R22
	MOVD  dc+8(FP), R5
	ADD   R22, R5, R5
	MOVD  ac+16(FP), R6
	ADD   R22, R6, R6
	MOVD  $0, R7
	CBZ   R13, dc1
	MOVD  (R13)(R11<<3), R7

dc1:
	CMP   $16, R4
	BGT   dc2
	REFILL(dcs1, dcm1)
dc2:
	LSR   $(64-const_combBits), R3, R22
	ADD   $derivedTable_combined, R5, R23
	MOVWU (R23)(R22<<2), R24
	CBZ   R24, dclookup
	UBFX  $8, R24, $4, R23
	SUB   R23, R4, R23
	LSL   R24, R3, R3
	AND   $63, R24, R25
	SUB   R25, R4, R4
	ASRW  $16, R24, R26
	TBNZ  $6, R24, dchave
	CMP   $16, R23
	BGT   dchave
	REFILL(dcs2, dcm2)
	B     dchave

dclookup:
	LSR   $56, R3, R22
	ADD   $derivedTable_lookup, R5, R23
	MOVHU (R23)(R22<<1), R23
	LSR   $8, R23, R25
	CBZ   R25, dcslow
	LSL   R25, R3, R3
	SUB   R25, R4, R4
	AND   $0xff, R23, R22
dcsym:
	MOVD  $0, R26
	CBZ   R22, dchave
	MOVD  R22, R25
	CMP   $16, R4
	BGT   dcval
	MOVD  R22, R26
	REFILL(dcs3, dcm3)
	MOVD  R26, R25
dcval:
	VALUE
	MOVD  R24, R26

dchave:
	MOVW  (R10)(R19<<2), R23
	ADDW  R26, R23, R23
	MOVW  R23, (R10)(R19<<2)
	CBZ   R7, 2(PC)
	MOVH  R23, (R7)
	ADD   $derivedTable_combined, R6, R26
	MOVD  $1, R8

acloop:
	CMP   $16, R4
	BGT   ac2
	REFILL(acs1, acm1)
ac2:
	LSR   $(64-const_combBits), R3, R22
	MOVWU (R26)(R22<<2), R24
	CBZ   R24, aclookup
	LSL   R24, R3, R3
	UBFX  $8, R24, $4, R23
	SUB   R23, R4, R23
	AND   $63, R24, R25
	SUB   R25, R4, R4
	UBFX  $12, R24, $4, R25
	TBNZ  $6, R24, aczero
	ADD   R25, R8, R8
	CBZ   R7, acnostore
	ASRW  $16, R24, R24
	MOVBU (R9)(R8), R25
	MOVH  R24, (R7)(R25<<1)
acnostore:
	CMP   $16, R23
	BLE   acfill
acnext:
	ADD   $1, R8, R8
	CMP   $64, R8
	BLT   acloop
	B     blockdone

acfill:
	REFILL(acs2, acm2)
	B     acnext

aczero:
	CMP   $15, R25
	BNE   blockdone
	ADD   $15, R8, R8
	B     acnext

aclookup:
	LSR   $56, R3, R22
	ADD   $derivedTable_lookup, R6, R23
	MOVHU (R23)(R22<<1), R23
	LSR   $8, R23, R25
	CBZ   R25, acslow
	LSL   R25, R3, R3
	SUB   R25, R4, R4
	AND   $0xff, R23, R22
acrs:
	AND   $15, R22, R25
	CBZ   R25, acsize0
	LSR   $4, R22, R22
	ADD   R22, R8, R8
	CMP   $16, R4
	BGT   acval
	MOVD  R25, R12
	REFILL(acs3, acm3)
	MOVD  R12, R25
acval:
	CBZ   R7, acdrop
	VALUE
	MOVBU (R9)(R8), R25
	MOVH  R24, (R7)(R25<<1)
	B     acnext
acdrop:
	LSL   R25, R3, R3
	SUB   R25, R4, R4
	B     acnext
acsize0:
	LSR   $4, R22, R22
	CMP   $15, R22
	BNE   blockdone
	ADD   $15, R8, R8
	B     acnext

blockdone:
	CBZ   R7, blocknext
	MOVD  hasAC+56(FP), R22
	CMP   $1, R8
	CSET  NE, R23
	MOVB  R23, (R22)(R11)
blocknext:
	ADD   $1, R11, R11
	B     block

finish:
	MOVB  R15, ret+64(FP)
	CBZ   R15, done
	MOVD  r+0(FP), R22
	MOVD  bitReader_pos(R22), R23
	SUB   R23, R2, R23
	MOVD  bitReader_decoded(R22), R24
	ADD   R23, R24, R24
	MOVD  R24, bitReader_decoded(R22)
	MOVD  R2, bitReader_pos(R22)
	MOVD  R4, bitReader_bitsLeft(R22)
	MOVD  $64, R23
	SUB   R4, R23, R23
	LSR   R23, R3, R3
	CMP   $0, R4
	CSEL  EQ, ZR, R3, R3
	MOVD  R3, bitReader_buf(R22)
	MOVD  lastDC+24(FP), R22
	MOVD  (R10), R23
	MOVD  R23, (R22)
	MOVD  8(R10), R23
	MOVD  R23, 8(R22)
done:
	RET

dcslow:
	SLOWHUFF(R5, dcsl, dcsf, dcsb, dcsym)
acslow:
	SLOWHUFF(R6, acsl, acsf, acsb, acrs)

	REFILLSLOW(dcs1, dcl1, dcp1, dck1, dcn1, dcm1)
	REFILLSLOW(dcs2, dcl2, dcp2, dck2, dcn2, dcm2)
	REFILLSLOW(dcs3, dcl3, dcp3, dck3, dcn3, dcm3)
	REFILLSLOW(acs1, acl1, acp1, ack1, acn1, acm1)
	REFILLSLOW(acs2, acl2, acp2, ack2, acn2, acm2)
	REFILLSLOW(acs3, acl3, acp3, ack3, acn3, acm3)
