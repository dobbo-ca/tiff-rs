//go:build !purego

#include "go_asm.h"
#include "textflag.h"

// R8 bw, R9 acc, R10 free.
// R11 out base, R12 out len.
// R13 mask, SI last k.
// R14 run, DI ac.
// PUT: DX code, CX size (jchuff.c).
// Clobbers AX, BX, CX.
#define PUT(sp, dn) \
	SUBQ  CX, R10 \
	JS    sp \
	SHLXQ CX, R9, R9 \
	ORQ   DX, R9 \
dn:

// SPILL: PUT's full-buffer path, stuffing 0xFF.
#define SPILL(sp, st, sl, sn, dn) \
sp: \
	ADDQ  R10, CX \
	SHLXQ CX, R9, R9 \
	MOVQ  R10, CX \
	NEGQ  CX \
	SHRXQ CX, DX, AX \
	ORQ   AX, R9 \
	MOVQ  R9, AX \
	NOTQ  AX \
	MOVQ  $0x0101010101010101, BX \
	SUBQ  AX, BX \
	NEGQ  BX \
	ANDNQ BX, AX, BX \
	MOVQ  $0x8080808080808080, AX \
	TESTQ AX, BX \
	JNZ   st \
	BSWAPQ R9 \
	MOVQ  R9, (R11)(R12*1) \
	ADDQ  $8, R12 \
	JMP   sn \
st: \
	MOVQ  $8, BX \
sl: \
	ROLQ  $8, R9 \
	MOVB  R9, (R11)(R12*1) \
	INCQ  R12 \
	MOVB  $0, (R11)(R12*1) \
	CMPB  R9, $0xff \
	JNE   2(PC) \
	INCQ  R12 \
	DECQ  BX \
	JNZ   sl \
sn: \
	MOVQ  DX, R9 \
	ADDQ  $64, R10 \
	JMP   dn

// VPERMI2B indices: zigzag word bytes.
DATA zzIdx<>+0(SB)/8, $0x2120111003020100
DATA zzIdx<>+8(SB)/8, $0x1514070605041312
DATA zzIdx<>+16(SB)/8, $0x3332414031302322
DATA zzIdx<>+24(SB)/8, $0x0b0a090817162524
DATA zzIdx<>+32(SB)/8, $0x4342353427261918
DATA zzIdx<>+40(SB)/8, $0x4544535261605150
DATA zzIdx<>+48(SB)/8, $0x0d0c1b1a29283736
DATA zzIdx<>+56(SB)/8, $0x39382b2a1d1c0f0e
DATA zzIdx<>+64(SB)/8, $0x7170636255544746
DATA zzIdx<>+72(SB)/8, $0x4948575665647372
DATA zzIdx<>+80(SB)/8, $0x2f2e1f1e2d2c3b3a
DATA zzIdx<>+88(SB)/8, $0x676659584b4a3d3c
DATA zzIdx<>+96(SB)/8, $0x5b5a696877767574
DATA zzIdx<>+104(SB)/8, $0x5d5c4f4e3f3e4d4c
DATA zzIdx<>+112(SB)/8, $0x6d6c7b7a79786b6a
DATA zzIdx<>+120(SB)/8, $0x7f7e7d7c6f6e5f5e
GLOBL zzIdx<>(SB), RODATA|NOPTR, $128

// NBITS16 low-word mask and 16s.
DATA nbConst<>+0(SB)/4, $0x0000ffff
DATA nbConst<>+4(SB)/4, $0x00100010
GLOBL nbConst<>(SB), RODATA|NOPTR, $8

// NBITS16: word bit lengths; clobbers Z6-Z7.
#define NBITS16(r) \
	VPSLLD   $16, r, Z6 \
	VPORD    Z4, Z6, Z6 \
	VPORD    Z4, r, Z7 \
	VPLZCNTD Z6, Z6 \
	VPLZCNTD Z7, Z7 \
	VPSLLD   $16, Z7, Z7 \
	VPORD    Z7, Z6, Z6 \
	VPSUBW   Z6, Z5, r

// func writeBlockAsm(bw *bitWriter, blk *[64]int16, lastDC *int32, dc, ac *[256]HuffCode)
TEXT ·writeBlockAsm(SB), NOSPLIT, $32-40
	MOVQ  bw+0(FP), R8
	MOVQ  blk+8(FP), SI
	CMPB  ·writeBlockAVX512(SB), $0
	JEQ   prepcall

	// 32-byte loads: fdct store forwarding.
	VMOVDQU  (SI), Y0
	VINSERTI64X4 $1, 32(SI), Z0, Z0
	VMOVDQU  64(SI), Y1
	VINSERTI64X4 $1, 96(SI), Z1, Z1
	VMOVDQU64 zzIdx<>+0(SB), Z2
	VMOVDQU64 zzIdx<>+64(SB), Z3
	VPERMI2B Z1, Z0, Z2
	VPERMI2B Z1, Z0, Z3
	VPTESTMW Z2, Z2, K1
	VPTESTMW Z3, Z3, K2
	KUNPCKDQ K1, K2, K3
	KMOVQ K3, R13
	TESTQ $-2, R13 // no AC: vals, nb unused
	JNZ   prep512
	VZEROUPPER
	JMP   prepped

prep512:
	VPSRAW   $15, Z2, Z0
	VPADDW   Z0, Z2, Z0
	VMOVDQU64 Z0, bitWriter_vals(R8)
	VPSRAW   $15, Z3, Z1
	VPADDW   Z1, Z3, Z1
	VMOVDQU64 Z1, (bitWriter_vals+64)(R8)
	VPBROADCASTD nbConst<>+0(SB), Z4
	VPBROADCASTD nbConst<>+4(SB), Z5
	VPABSW   Z2, Z2
	VPABSW   Z3, Z3
	NBITS16(Z2)
	NBITS16(Z3)
	VPMOVWB  Z2, bitWriter_nb(R8)
	VPMOVWB  Z3, (bitWriter_nb+32)(R8)
	VZEROUPPER
	JMP   prepped

prepcall:
	VMOVDQU (SI), X0
	VPXOR   X1, X1, X1
	VPBLENDW $1, X1, X0, X0 // drop DC
	VPOR    16(SI), X0, X0
	VPOR    32(SI), X0, X0
	VPOR    48(SI), X0, X0
	VPOR    64(SI), X0, X0
	VPOR    80(SI), X0, X0
	VPOR    96(SI), X0, X0
	VPOR    112(SI), X0, X0
	VPTEST  X0, X0
	JNZ     callprep
	XORQ    R13, R13
	JMP     prepped

callprep:
	MOVQ  SI, 0(SP)
	LEAQ  bitWriter_vals(R8), AX
	MOVQ  AX, 8(SP)
	LEAQ  bitWriter_nb(R8), AX
	MOVQ  AX, 16(SP)
	CALL  ·huffPrepAsm(SB)
	MOVQ  24(SP), R13
	MOVQ  bw+0(FP), R8
	MOVQ  blk+8(FP), SI

prepped:
	BTRQ  $0, R13
	MOVQ  bitWriter_acc(R8), R9
	MOVQ  bitWriter_free(R8), R10
	MOVQ  bitWriter_out(R8), R11
	MOVQ  (bitWriter_out+8)(R8), R12

	// DC: diff against lastDC, category s, then s value bits.
	MOVWLSX (SI), AX
	MOVQ  lastDC+16(FP), DI
	MOVL  (DI), BX
	MOVL  AX, (DI)
	SUBL  BX, AX
	MOVL  AX, BX
	SARL  $31, BX
	MOVL  AX, CX
	XORL  BX, CX
	SUBL  BX, CX
	LEAL  1(CX)(CX*1), CX
	BSRL  CX, CX
	ADDL  BX, AX
	BZHIQ CX, AX, AX
	MOVQ  dc+24(FP), DI
	MOVWQZX (DI)(CX*4), DX
	MOVBQZX 2(DI)(CX*4), BX
	SHLXQ CX, DX, DX
	ORQ   AX, DX
	ADDQ  BX, CX
	PUT(dcsp, dcdn)

	MOVQ  ac+32(FP), DI
	XORQ  SI, SI
	TESTQ R13, R13
	JZ    acend

acloop:
	TZCNTQ R13, AX
	BLSRQ R13, R13
	MOVQ  AX, R14
	SUBQ  SI, R14
	DECQ  R14
	MOVQ  AX, SI
	CMPQ  R14, $16
	JGE   zrl

acsym:
	MOVBQZX bitWriter_nb(R8)(SI*1), CX
	SHLQ  $4, R14
	ORQ   CX, R14
	MOVWQZX bitWriter_vals(R8)(SI*2), AX
	BZHIQ CX, AX, AX
	MOVWQZX (DI)(R14*4), DX
	MOVBQZX 2(DI)(R14*4), BX
	SHLXQ CX, DX, DX
	ORQ   AX, DX
	ADDQ  BX, CX
	PUT(asp, adn)
	TESTQ R13, R13
	JNZ   acloop

acend:
	CMPQ  SI, $63
	JEQ   done
	MOVWQZX (DI), DX
	MOVBQZX 2(DI), CX
	PUT(esp, edn)

done:
	MOVQ  R9, bitWriter_acc(R8)
	MOVQ  R10, bitWriter_free(R8)
	MOVQ  R12, (bitWriter_out+8)(R8)
	RET

zrl:
	MOVWQZX 960(DI), DX
	MOVBQZX 962(DI), CX
	PUT(zsp, zdn)
	SUBQ  $16, R14
	CMPQ  R14, $16
	JGE   zrl
	JMP   acsym

	SPILL(dcsp, dcst, dcsl, dcsn, dcdn)
	SPILL(asp, ast, asl, asn, adn)
	SPILL(esp, est, esl, esn, edn)
	SPILL(zsp, zst, zsl, zsn, zdn)
