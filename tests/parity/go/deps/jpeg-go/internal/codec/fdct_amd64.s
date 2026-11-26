//go:build !purego

#include "textflag.h"

// fdctQuantAsm: turbo's jsimd_convsamp/fdct_islow/quantize_avx2 fused,
// 16-bit lanes, two rows per register. Output is int16.

#define KF130 fdctK<>+0(SB)
#define KF078 fdctK<>+32(SB)
#define KF060 fdctK<>+64(SB)
#define KF050 fdctK<>+96(SB)
#define KPD1 fdctK<>+128(SB)
#define KPD2 fdctK<>+160(SB)
#define KP2X fdctK<>+192(SB)
#define K1N1 fdctK<>+224(SB)
#define KM128 fdctK<>+256(SB)

DATA fdctK<>+0(SB)/8, $0x115129cf115129cf
DATA fdctK<>+8(SB)/8, $0x115129cf115129cf
DATA fdctK<>+16(SB)/8, $0x1151d6301151d630
DATA fdctK<>+24(SB)/8, $0x1151d6301151d630
DATA fdctK<>+32(SB)/8, $0x25a1e6dc25a1e6dc
DATA fdctK<>+40(SB)/8, $0x25a1e6dc25a1e6dc
DATA fdctK<>+48(SB)/8, $0x25a1192525a11925
DATA fdctK<>+56(SB)/8, $0x25a1192525a11925
DATA fdctK<>+64(SB)/8, $0xe333ecc1e333ecc1
DATA fdctK<>+72(SB)/8, $0xe333ecc1e333ecc1
DATA fdctK<>+80(SB)/8, $0xadfdefb0adfdefb0
DATA fdctK<>+88(SB)/8, $0xadfdefb0adfdefb0
DATA fdctK<>+96(SB)/8, $0xadfd1051adfd1051
DATA fdctK<>+104(SB)/8, $0xadfd1051adfd1051
DATA fdctK<>+112(SB)/8, $0xe333133ee333133e
DATA fdctK<>+120(SB)/8, $0xe333133ee333133e
DATA fdctK<>+128(SB)/8, $0x0000040000000400
DATA fdctK<>+136(SB)/8, $0x0000040000000400
DATA fdctK<>+144(SB)/8, $0x0000040000000400
DATA fdctK<>+152(SB)/8, $0x0000040000000400
DATA fdctK<>+160(SB)/8, $0x0000400000004000
DATA fdctK<>+168(SB)/8, $0x0000400000004000
DATA fdctK<>+176(SB)/8, $0x0000400000004000
DATA fdctK<>+184(SB)/8, $0x0000400000004000
DATA fdctK<>+192(SB)/8, $0x0002000200020002
DATA fdctK<>+200(SB)/8, $0x0002000200020002
DATA fdctK<>+208(SB)/8, $0x0002000200020002
DATA fdctK<>+216(SB)/8, $0x0002000200020002
DATA fdctK<>+224(SB)/8, $0x0001000100010001
DATA fdctK<>+232(SB)/8, $0x0001000100010001
DATA fdctK<>+240(SB)/8, $0xffffffffffffffff
DATA fdctK<>+248(SB)/8, $0xffffffffffffffff
DATA fdctK<>+256(SB)/8, $0xff80ff80ff80ff80
DATA fdctK<>+264(SB)/8, $0xff80ff80ff80ff80
DATA fdctK<>+272(SB)/8, $0xff80ff80ff80ff80
DATA fdctK<>+280(SB)/8, $0xff80ff80ff80ff80
GLOBL fdctK<>(SB), RODATA|NOPTR, $288

#define DOTRANSPOSE(a1, a2, a3, a4, a5, a6, a7, a8) \
	VPUNPCKLWD a2, a1, a5; \
	VPUNPCKHWD a2, a1, a6; \
	VPUNPCKLWD a4, a3, a7; \
	VPUNPCKHWD a4, a3, a8; \
	VPUNPCKLDQ a7, a5, a1; \
	VPUNPCKHDQ a7, a5, a2; \
	VPUNPCKLDQ a8, a6, a3; \
	VPUNPCKHDQ a8, a6, a4; \
	VPERMQ $0x8D, a1, a1; \
	VPERMQ $0x8D, a2, a2; \
	VPERMQ $0xD8, a3, a3; \
	VPERMQ $0xD8, a4, a4

#define DODCT_A(a1, a2, a3, a4, a5, a6, a7, a8) \
	VPSUBW a4, a1, a5; \
	VPADDW a4, a1, a6; \
	VPADDW a3, a2, a7; \
	VPSUBW a3, a2, a8; \
	VPERM2I128 $0x01, a6, a6, a6; \
	VPADDW a7, a6, a1; \
	VPSUBW a7, a6, a6; \
	VPERM2I128 $0x01, a1, a1, a7; \
	VPSIGNW K1N1, a1, a1; \
	VPADDW a1, a7, a7

#define DODCT_B(a1, a2, a3, a4, a5, a6, a7, a8, kpd, n) \
	VPERM2I128 $0x01, a6, a6, a7; \
	VPUNPCKLWD a7, a6, a2; \
	VPUNPCKHWD a7, a6, a6; \
	VPMADDWD KF130, a2, a2; \
	VPMADDWD KF130, a6, a6; \
	VPADDD kpd, a2, a2; \
	VPADDD kpd, a6, a6; \
	VPSRAD $n, a2, a2; \
	VPSRAD $n, a6, a6; \
	VPACKSSDW a6, a2, a3; \
	VPADDW a5, a8, a7; \
	VPERM2I128 $0x01, a7, a7, a2; \
	VPUNPCKLWD a2, a7, a6; \
	VPUNPCKHWD a2, a7, a7; \
	VPMADDWD KF078, a6, a6; \
	VPMADDWD KF078, a7, a7; \
	VPERM2I128 $0x01, a5, a5, a4; \
	VPUNPCKLWD a4, a8, a2; \
	VPUNPCKHWD a4, a8, a4; \
	VPMADDWD KF060, a2, a2; \
	VPMADDWD KF060, a4, a4; \
	VPADDD a6, a2, a2; \
	VPADDD a7, a4, a4; \
	VPADDD kpd, a2, a2; \
	VPADDD kpd, a4, a4; \
	VPSRAD $n, a2, a2; \
	VPSRAD $n, a4, a4; \
	VPACKSSDW a4, a2, a4; \
	VPERM2I128 $0x01, a8, a8, a2; \
	VPUNPCKLWD a2, a5, a8; \
	VPUNPCKHWD a2, a5, a5; \
	VPMADDWD KF050, a8, a8; \
	VPMADDWD KF050, a5, a5; \
	VPADDD a6, a8, a8; \
	VPADDD a7, a5, a5; \
	VPADDD kpd, a8, a8; \
	VPADDD kpd, a5, a5; \
	VPSRAD $n, a8, a8; \
	VPSRAD $n, a5, a5; \
	VPACKSSDW a5, a8, a2

// QUANT quantizes rows 2k,2k+1 in x (divisor byte offset off), stores int16.
#define QUANT(x, off) \
	VPABSW       x, Y8; \
	VPADDW       128+off(DI), Y8, Y8; \
	VPMULHUW     off(DI), Y8, Y8; \
	VPMULHUW     256+off(DI), Y8, Y8; \
	VPSIGNW      x, Y8, Y8; \
	VMOVDQU      Y8, off(R8)

// func fdctQuantAsm(pix *byte, stride int, div *fdctDivisors, coef *[64]int16)
TEXT ·fdctQuantAsm(SB), NOSPLIT, $0-32
	MOVQ pix+0(FP), SI
	MOVQ stride+8(FP), BX
	MOVQ div+16(FP), DI
	MOVQ coef+24(FP), R8
	LEAQ (BX)(BX*2), DX
	LEAQ (SI)(BX*4), R9
	MOVQ    (SI), X0
	VPINSRQ $1, (R9), X0, X0
	MOVQ    (SI)(BX*1), X1
	VPINSRQ $1, (R9)(BX*1), X1, X1
	MOVQ    (SI)(BX*2), X2
	VPINSRQ $1, (R9)(BX*2), X2, X2
	MOVQ    (SI)(DX*1), X3
	VPINSRQ $1, (R9)(DX*1), X3, X3
	VPMOVZXBW X0, Y0
	VPMOVZXBW X1, Y1
	VPMOVZXBW X2, Y2
	VPMOVZXBW X3, Y3
	VPADDW KM128, Y0, Y0
	VPADDW KM128, Y1, Y1
	VPADDW KM128, Y2, Y2
	VPADDW KM128, Y3, Y3

	// Y0-Y3 hold rows 0|4, 1|5, 2|6, 3|7 (jfdctint-avx2.asm layout).
	DOTRANSPOSE(Y0, Y1, Y2, Y3, Y4, Y5, Y6, Y7)
	DODCT_A(Y0, Y1, Y2, Y3, Y4, Y5, Y6, Y7)
	VPSLLW $2, Y6, Y0
	DODCT_B(Y0, Y1, Y2, Y3, Y4, Y5, Y6, Y7, KPD1, 11)

	VPERM2I128 $0x20, Y3, Y1, Y4
	VPERM2I128 $0x31, Y3, Y1, Y1
	DOTRANSPOSE(Y0, Y1, Y2, Y4, Y3, Y5, Y6, Y7)
	DODCT_A(Y0, Y1, Y2, Y4, Y3, Y5, Y6, Y7)
	VPADDW KP2X, Y6, Y6
	VPSRAW $2, Y6, Y0
	DODCT_B(Y0, Y1, Y2, Y4, Y3, Y5, Y6, Y7, KPD2, 15)

	VPERM2I128 $0x30, Y1, Y0, Y3
	VPERM2I128 $0x20, Y1, Y2, Y5
	VPERM2I128 $0x31, Y4, Y0, Y6
	VPERM2I128 $0x21, Y4, Y2, Y7

	QUANT(Y3, 0)
	QUANT(Y5, 32)
	QUANT(Y6, 64)
	QUANT(Y7, 96)
	VZEROUPPER
	RET

// fdctQuantPairAsm is fdctQuantAsm on two blocks at once: block a in
// the low and block b in the high 256 bits of each ZMM register, so
// every in-lane op matches the AVX2 kernel per block. Z0-Z15 only:
// VZEROUPPER does not clean Z16-Z31.

#define ZF130 fdctZ<>+0(SB)
#define ZF078 fdctZ<>+64(SB)
#define ZF060 fdctZ<>+128(SB)
#define ZF050 fdctZ<>+192(SB)
#define ZPD1 fdctZ<>+256(SB)
#define ZPD2 fdctZ<>+320(SB)
#define ZP2X fdctZ<>+384(SB)
#define ZM128 fdctZ<>+448(SB)

DATA fdctZ<>+0(SB)/8, $0x115129cf115129cf
DATA fdctZ<>+8(SB)/8, $0x115129cf115129cf
DATA fdctZ<>+16(SB)/8, $0x1151d6301151d630
DATA fdctZ<>+24(SB)/8, $0x1151d6301151d630
DATA fdctZ<>+32(SB)/8, $0x115129cf115129cf
DATA fdctZ<>+40(SB)/8, $0x115129cf115129cf
DATA fdctZ<>+48(SB)/8, $0x1151d6301151d630
DATA fdctZ<>+56(SB)/8, $0x1151d6301151d630
DATA fdctZ<>+64(SB)/8, $0x25a1e6dc25a1e6dc
DATA fdctZ<>+72(SB)/8, $0x25a1e6dc25a1e6dc
DATA fdctZ<>+80(SB)/8, $0x25a1192525a11925
DATA fdctZ<>+88(SB)/8, $0x25a1192525a11925
DATA fdctZ<>+96(SB)/8, $0x25a1e6dc25a1e6dc
DATA fdctZ<>+104(SB)/8, $0x25a1e6dc25a1e6dc
DATA fdctZ<>+112(SB)/8, $0x25a1192525a11925
DATA fdctZ<>+120(SB)/8, $0x25a1192525a11925
DATA fdctZ<>+128(SB)/8, $0xe333ecc1e333ecc1
DATA fdctZ<>+136(SB)/8, $0xe333ecc1e333ecc1
DATA fdctZ<>+144(SB)/8, $0xadfdefb0adfdefb0
DATA fdctZ<>+152(SB)/8, $0xadfdefb0adfdefb0
DATA fdctZ<>+160(SB)/8, $0xe333ecc1e333ecc1
DATA fdctZ<>+168(SB)/8, $0xe333ecc1e333ecc1
DATA fdctZ<>+176(SB)/8, $0xadfdefb0adfdefb0
DATA fdctZ<>+184(SB)/8, $0xadfdefb0adfdefb0
DATA fdctZ<>+192(SB)/8, $0xadfd1051adfd1051
DATA fdctZ<>+200(SB)/8, $0xadfd1051adfd1051
DATA fdctZ<>+208(SB)/8, $0xe333133ee333133e
DATA fdctZ<>+216(SB)/8, $0xe333133ee333133e
DATA fdctZ<>+224(SB)/8, $0xadfd1051adfd1051
DATA fdctZ<>+232(SB)/8, $0xadfd1051adfd1051
DATA fdctZ<>+240(SB)/8, $0xe333133ee333133e
DATA fdctZ<>+248(SB)/8, $0xe333133ee333133e
DATA fdctZ<>+256(SB)/8, $0x0000040000000400
DATA fdctZ<>+264(SB)/8, $0x0000040000000400
DATA fdctZ<>+272(SB)/8, $0x0000040000000400
DATA fdctZ<>+280(SB)/8, $0x0000040000000400
DATA fdctZ<>+288(SB)/8, $0x0000040000000400
DATA fdctZ<>+296(SB)/8, $0x0000040000000400
DATA fdctZ<>+304(SB)/8, $0x0000040000000400
DATA fdctZ<>+312(SB)/8, $0x0000040000000400
DATA fdctZ<>+320(SB)/8, $0x0000400000004000
DATA fdctZ<>+328(SB)/8, $0x0000400000004000
DATA fdctZ<>+336(SB)/8, $0x0000400000004000
DATA fdctZ<>+344(SB)/8, $0x0000400000004000
DATA fdctZ<>+352(SB)/8, $0x0000400000004000
DATA fdctZ<>+360(SB)/8, $0x0000400000004000
DATA fdctZ<>+368(SB)/8, $0x0000400000004000
DATA fdctZ<>+376(SB)/8, $0x0000400000004000
DATA fdctZ<>+384(SB)/8, $0x0002000200020002
DATA fdctZ<>+392(SB)/8, $0x0002000200020002
DATA fdctZ<>+400(SB)/8, $0x0002000200020002
DATA fdctZ<>+408(SB)/8, $0x0002000200020002
DATA fdctZ<>+416(SB)/8, $0x0002000200020002
DATA fdctZ<>+424(SB)/8, $0x0002000200020002
DATA fdctZ<>+432(SB)/8, $0x0002000200020002
DATA fdctZ<>+440(SB)/8, $0x0002000200020002
DATA fdctZ<>+448(SB)/8, $0xff80ff80ff80ff80
DATA fdctZ<>+456(SB)/8, $0xff80ff80ff80ff80
DATA fdctZ<>+464(SB)/8, $0xff80ff80ff80ff80
DATA fdctZ<>+472(SB)/8, $0xff80ff80ff80ff80
DATA fdctZ<>+480(SB)/8, $0xff80ff80ff80ff80
DATA fdctZ<>+488(SB)/8, $0xff80ff80ff80ff80
DATA fdctZ<>+496(SB)/8, $0xff80ff80ff80ff80
DATA fdctZ<>+504(SB)/8, $0xff80ff80ff80ff80
GLOBL fdctZ<>(SB), RODATA|NOPTR, $512

// VPERMI2Q/VPERMT2Q qword indices for VPERM2I128 $0x20/$0x31/$0x30/$0x21
// done in each 256-bit half.
DATA pidx20<>+0(SB)/8, $0
DATA pidx20<>+8(SB)/8, $1
DATA pidx20<>+16(SB)/8, $8
DATA pidx20<>+24(SB)/8, $9
DATA pidx20<>+32(SB)/8, $4
DATA pidx20<>+40(SB)/8, $5
DATA pidx20<>+48(SB)/8, $12
DATA pidx20<>+56(SB)/8, $13
GLOBL pidx20<>(SB), RODATA|NOPTR, $64
DATA pidx31<>+0(SB)/8, $2
DATA pidx31<>+8(SB)/8, $3
DATA pidx31<>+16(SB)/8, $10
DATA pidx31<>+24(SB)/8, $11
DATA pidx31<>+32(SB)/8, $6
DATA pidx31<>+40(SB)/8, $7
DATA pidx31<>+48(SB)/8, $14
DATA pidx31<>+56(SB)/8, $15
GLOBL pidx31<>(SB), RODATA|NOPTR, $64
DATA pidx30<>+0(SB)/8, $0
DATA pidx30<>+8(SB)/8, $1
DATA pidx30<>+16(SB)/8, $10
DATA pidx30<>+24(SB)/8, $11
DATA pidx30<>+32(SB)/8, $4
DATA pidx30<>+40(SB)/8, $5
DATA pidx30<>+48(SB)/8, $14
DATA pidx30<>+56(SB)/8, $15
GLOBL pidx30<>(SB), RODATA|NOPTR, $64
DATA pidx21<>+0(SB)/8, $2
DATA pidx21<>+8(SB)/8, $3
DATA pidx21<>+16(SB)/8, $8
DATA pidx21<>+24(SB)/8, $9
DATA pidx21<>+32(SB)/8, $6
DATA pidx21<>+40(SB)/8, $7
DATA pidx21<>+48(SB)/8, $12
DATA pidx21<>+56(SB)/8, $13
GLOBL pidx21<>(SB), RODATA|NOPTR, $64

// ZDODCT_A is DODCT_A: VPERM2I128 $0x01 is VPERMQ $0x4E, and VPSIGNW
// K1N1 is a negate under K1 (words 8-15 of each half).
#define ZDODCT_A(a1, a2, a3, a4, a5, a6, a7, a8) \
	VPSUBW a4, a1, a5; \
	VPADDW a4, a1, a6; \
	VPADDW a3, a2, a7; \
	VPSUBW a3, a2, a8; \
	VPERMQ $0x4E, a6, a6; \
	VPADDW a7, a6, a1; \
	VPSUBW a7, a6, a6; \
	VPERMQ $0x4E, a1, a7; \
	VPSUBW a1, Z11, K1, a1; \
	VPADDW a1, a7, a7

#define ZDODCT_B(a1, a2, a3, a4, a5, a6, a7, a8, kpd, n) \
	VPERMQ $0x4E, a6, a7; \
	VPUNPCKLWD a7, a6, a2; \
	VPUNPCKHWD a7, a6, a6; \
	VPMADDWD ZF130, a2, a2; \
	VPMADDWD ZF130, a6, a6; \
	VPADDD kpd, a2, a2; \
	VPADDD kpd, a6, a6; \
	VPSRAD $n, a2, a2; \
	VPSRAD $n, a6, a6; \
	VPACKSSDW a6, a2, a3; \
	VPADDW a5, a8, a7; \
	VPERMQ $0x4E, a7, a2; \
	VPUNPCKLWD a2, a7, a6; \
	VPUNPCKHWD a2, a7, a7; \
	VPMADDWD ZF078, a6, a6; \
	VPMADDWD ZF078, a7, a7; \
	VPERMQ $0x4E, a5, a4; \
	VPUNPCKLWD a4, a8, a2; \
	VPUNPCKHWD a4, a8, a4; \
	VPMADDWD ZF060, a2, a2; \
	VPMADDWD ZF060, a4, a4; \
	VPADDD a6, a2, a2; \
	VPADDD a7, a4, a4; \
	VPADDD kpd, a2, a2; \
	VPADDD kpd, a4, a4; \
	VPSRAD $n, a2, a2; \
	VPSRAD $n, a4, a4; \
	VPACKSSDW a4, a2, a4; \
	VPERMQ $0x4E, a8, a2; \
	VPUNPCKLWD a2, a5, a8; \
	VPUNPCKHWD a2, a5, a5; \
	VPMADDWD ZF050, a8, a8; \
	VPMADDWD ZF050, a5, a5; \
	VPADDD a6, a8, a8; \
	VPADDD a7, a5, a5; \
	VPADDD kpd, a8, a8; \
	VPADDD kpd, a5, a5; \
	VPSRAD $n, a8, a8; \
	VPSRAD $n, a5, a5; \
	VPACKSSDW a5, a8, a2

// ZQUANT is QUANT on both halves (fdctPairDivisors offsets r, c, s);
// VPSIGNW is a negate under the sign mask of x.
#define ZQUANT(x, r, c, s, o) \
	VPABSW        x, Z8; \
	VPADDW        c(DI), Z8, Z8; \
	VPMULHUW      r(DI), Z8, Z8; \
	VPMULHUW      s(DI), Z8, Z8; \
	VPMOVW2M      x, K2; \
	VPSUBW        Z8, Z11, K2, Z8; \
	VMOVDQU       Y8, o(R8); \
	VEXTRACTI64X4 $1, Z8, o(R14)

// ZROWS loads rows r, r+4 of block a then block b into x (words).
#define ZROWS(x, y, z, ra, rb) \
	VMOVQ       ra(SI), x; \
	VPINSRQ     $1, ra(R9), x, x; \
	VMOVQ       rb(R10), X12; \
	VPINSRQ     $1, rb(R13), X12, X12; \
	VINSERTI128 $1, X12, y, y; \
	VPMOVZXBW   y, z; \
	VPADDW      ZM128, z, z

// func fdctQuantPairAsm(a, b *byte, sa, sb int, div *fdctPairDivisors, ca, cb *[64]int16)
TEXT ·fdctQuantPairAsm(SB), NOSPLIT, $0-56
	MOVQ a+0(FP), SI
	MOVQ b+8(FP), R10
	MOVQ sa+16(FP), BX
	MOVQ sb+24(FP), R11
	MOVQ div+32(FP), DI
	MOVQ ca+40(FP), R8
	MOVQ cb+48(FP), R14
	LEAQ (SI)(BX*4), R9
	LEAQ (R10)(R11*4), R13
	ZROWS(X0, Y0, Z0, 0, 0)
	ADDQ BX, SI
	ADDQ BX, R9
	ADDQ R11, R10
	ADDQ R11, R13
	ZROWS(X1, Y1, Z1, 0, 0)
	ADDQ BX, SI
	ADDQ BX, R9
	ADDQ R11, R10
	ADDQ R11, R13
	ZROWS(X2, Y2, Z2, 0, 0)
	ADDQ BX, SI
	ADDQ BX, R9
	ADDQ R11, R10
	ADDQ R11, R13
	ZROWS(X3, Y3, Z3, 0, 0)
	MOVL   $0xff00ff00, AX
	KMOVD  AX, K1
	VPXORD Z11, Z11, Z11

	DOTRANSPOSE(Z0, Z1, Z2, Z3, Z4, Z5, Z6, Z7)
	ZDODCT_A(Z0, Z1, Z2, Z3, Z4, Z5, Z6, Z7)
	VPSLLW $2, Z6, Z0
	ZDODCT_B(Z0, Z1, Z2, Z3, Z4, Z5, Z6, Z7, ZPD1, 11)

	VMOVDQU64 pidx20<>(SB), Z4
	VPERMI2Q  Z3, Z1, Z4
	VMOVDQU64 pidx31<>(SB), Z9
	VPERMT2Q  Z3, Z9, Z1
	DOTRANSPOSE(Z0, Z1, Z2, Z4, Z3, Z5, Z6, Z7)
	ZDODCT_A(Z0, Z1, Z2, Z4, Z3, Z5, Z6, Z7)
	VPADDW ZP2X, Z6, Z6
	VPSRAW $2, Z6, Z0
	ZDODCT_B(Z0, Z1, Z2, Z4, Z3, Z5, Z6, Z7, ZPD2, 15)

	VMOVDQU64 pidx30<>(SB), Z3
	VPERMI2Q  Z1, Z0, Z3
	VMOVDQU64 pidx20<>(SB), Z5
	VPERMI2Q  Z1, Z2, Z5
	VMOVDQU64 pidx31<>(SB), Z6
	VPERMI2Q  Z4, Z0, Z6
	VMOVDQU64 pidx21<>(SB), Z7
	VPERMI2Q  Z4, Z2, Z7

	ZQUANT(Z3, 0, 256, 512, 0)
	ZQUANT(Z5, 64, 320, 576, 32)
	ZQUANT(Z6, 128, 384, 640, 64)
	ZQUANT(Z7, 192, 448, 704, 96)
	VZEROUPPER
	RET
