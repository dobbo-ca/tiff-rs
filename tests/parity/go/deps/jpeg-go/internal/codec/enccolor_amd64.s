//go:build !purego

#include "textflag.h"

// PSHUFB masks, per pixel: (r,g), (g,b) words and r, b dwords.
DATA shufRG<>+0(SB)/8, $0x8005800480018000
DATA shufRG<>+8(SB)/8, $0x800d800c80098008
DATA shufGB<>+0(SB)/8, $0x8006800580028001
DATA shufGB<>+8(SB)/8, $0x800e800d800a8009
DATA shufR<>+0(SB)/8, $0x8080800480808000
DATA shufR<>+8(SB)/8, $0x8080800c80808008
DATA shufB<>+0(SB)/8, $0x8080800680808002
DATA shufB<>+8(SB)/8, $0x8080800e8080800a
GLOBL shufRG<>(SB), RODATA|NOPTR, $16
GLOBL shufGB<>(SB), RODATA|NOPTR, $16
GLOBL shufR<>(SB), RODATA|NOPTR, $16
GLOBL shufB<>(SB), RODATA|NOPTR, $16

#define K32(NAME, V) \
	DATA NAME<>+0(SB)/8, V \
	DATA NAME<>+8(SB)/8, V \
	DATA NAME<>+16(SB)/8, V \
	DATA NAME<>+24(SB)/8, V \
	GLOBL NAME<>(SB), RODATA|NOPTR, $32

// PMADDWD word pairs; 38470 and 32768 exceed int16, so they split.
K32(kYrg, $0x4b234c8b4b234c8b) // 19595 r + 19235 g
K32(kYgb, $0x1d2f4b231d2f4b23) // 19235 g + 7471 b
K32(kCb, $0xab33d4cdab33d4cd)  // -11059 r - 21709 g
K32(kCr, $0xeb2f94d1eb2f94d1)  // -27439 g - 5329 b

// 64-byte forms for the AVX-512 loop.
#define K64(NAME, V) \
	DATA NAME<>+0(SB)/8, V \
	DATA NAME<>+8(SB)/8, V \
	DATA NAME<>+16(SB)/8, V \
	DATA NAME<>+24(SB)/8, V \
	DATA NAME<>+32(SB)/8, V \
	DATA NAME<>+40(SB)/8, V \
	DATA NAME<>+48(SB)/8, V \
	DATA NAME<>+56(SB)/8, V \
	GLOBL NAME<>(SB), RODATA|NOPTR, $64

K64(zYrg, $0x4b234c8b4b234c8b)
K64(zYgb, $0x1d2f4b231d2f4b23)
K64(zCb, $0xab33d4cdab33d4cd)
K64(zCr, $0xeb2f94d1eb2f94d1)

DATA encK<>+0(SB)/4, $32768
DATA encK<>+4(SB)/4, $(128<<16 + 32767)
DATA encK<>+8(SB)/4, $0x01010101
DATA encK<>+12(SB)/4, $0x00020001 // h2v2 bias 1,2
GLOBL encK<>(SB), RODATA|NOPTR, $16

// 8 pixels at OFF(SI) to Y, Cb, Cr dwords. Clobbers Y0-Y3.
#define YCC8(OFF, YO, BO, RO) \
	VMOVDQU  OFF(SI), Y0 \
	VPSHUFB  Y15, Y0, Y1 \
	VPSHUFB  Y14, Y0, Y2 \
	VPMADDWD kYrg<>(SB), Y1, YO \
	VPMADDWD kYgb<>(SB), Y2, Y3 \
	VPADDD   Y3, YO, YO \
	VPADDD   Y11, YO, YO \
	VPSRLD   $16, YO, YO \
	VPMADDWD kCb<>(SB), Y1, BO \
	VPSHUFB  Y12, Y0, Y3 \
	VPSLLD   $15, Y3, Y3 \
	VPADDD   Y3, BO, BO \
	VPADDD   Y10, BO, BO \
	VPSRLD   $16, BO, BO \
	VPMADDWD kCr<>(SB), Y2, RO \
	VPSHUFB  Y13, Y0, Y3 \
	VPSLLD   $15, Y3, Y3 \
	VPADDD   Y3, RO, RO \
	VPADDD   Y10, RO, RO \
	VPSRLD   $16, RO, RO

// 16 dwords A, B to 16 bytes at DST. Clobbers A, X0.
#define PACK16(A, B, XA, DST) \
	VPACKUSDW    B, A, A \
	VEXTRACTI128 $1, A, X0 \
	VPACKUSWB    X0, XA, XA \
	VPSHUFD      $0xd8, XA, XA \
	VMOVDQU      XA, (DST)

// func rgbaToYCbCrAsm(src, y, cb, cr *byte, n int)
TEXT ·rgbaToYCbCrAsm(SB), NOSPLIT, $0-40
	MOVQ src+0(FP), SI
	MOVQ y+8(FP), DI
	MOVQ cb+16(FP), R8
	MOVQ cr+24(FP), R9
	MOVQ n+32(FP), CX
	VBROADCASTI128 shufRG<>(SB), Y15
	VBROADCASTI128 shufGB<>(SB), Y14
	VBROADCASTI128 shufR<>(SB), Y13
	VBROADCASTI128 shufB<>(SB), Y12
	VPBROADCASTD   encK<>+0(SB), Y11
	VPBROADCASTD   encK<>+4(SB), Y10
	CMPB ·encColorAVX512(SB), $0
	JNE  wide

loop:
	YCC8(0, Y4, Y5, Y6)
	YCC8(32, Y7, Y8, Y9)
	PACK16(Y4, Y7, X4, DI)
	PACK16(Y5, Y8, X5, R8)
	PACK16(Y6, Y9, X6, R9)
	ADDQ $64, SI
	ADDQ $16, DI
	ADDQ $16, R8
	ADDQ $16, R9
	SUBQ $16, CX
	JNZ  loop
	VZEROUPPER
	RET

	// AVX-512: 16 pixels per ZMM, 4 per 128-bit lane as above; every
	// output dword is 0-255, so VPMOVDB's truncation is exact.
wide:
	VBROADCASTI32X4 shufRG<>(SB), Z15
	VBROADCASTI32X4 shufGB<>(SB), Z14
	VBROADCASTI32X4 shufR<>(SB), Z13
	VBROADCASTI32X4 shufB<>(SB), Z12
	VPBROADCASTD    encK<>+0(SB), Z11
	VPBROADCASTD    encK<>+4(SB), Z10

wloop:
	VMOVDQU64 (SI), Z0
	VPSHUFB   Z15, Z0, Z1
	VPSHUFB   Z14, Z0, Z2
	VPMADDWD  zYrg<>(SB), Z1, Z4
	VPMADDWD  zYgb<>(SB), Z2, Z3
	VPADDD    Z3, Z4, Z4
	VPADDD    Z11, Z4, Z4
	VPSRLD    $16, Z4, Z4
	VPMADDWD  zCb<>(SB), Z1, Z5
	VPSHUFB   Z12, Z0, Z3
	VPSLLD    $15, Z3, Z3
	VPADDD    Z3, Z5, Z5
	VPADDD    Z10, Z5, Z5
	VPSRLD    $16, Z5, Z5
	VPMADDWD  zCr<>(SB), Z2, Z6
	VPSHUFB   Z13, Z0, Z3
	VPSLLD    $15, Z3, Z3
	VPADDD    Z3, Z6, Z6
	VPADDD    Z10, Z6, Z6
	VPSRLD    $16, Z6, Z6
	VPMOVDB   Z4, (DI)
	VPMOVDB   Z5, (R8)
	VPMOVDB   Z6, (R9)
	ADDQ      $64, SI
	ADDQ      $16, DI
	ADDQ      $16, R8
	ADDQ      $16, R9
	SUBQ      $16, CX
	JNZ       wloop
	VZEROUPPER
	RET

// func downsampleH2V2Asm(dst, r0, r1 *byte, n int)
TEXT ·downsampleH2V2Asm(SB), NOSPLIT, $0-32
	MOVQ dst+0(FP), DI
	MOVQ r0+8(FP), SI
	MOVQ r1+16(FP), DX
	MOVQ n+24(FP), CX
	VPBROADCASTD encK<>+8(SB), Y6
	VPBROADCASTD encK<>+12(SB), Y7

dloop:
	// Pixels are the unsigned operand.
	VMOVDQU    (SI), Y0
	VMOVDQU    32(SI), Y1
	VMOVDQU    (DX), Y2
	VMOVDQU    32(DX), Y3
	VPMADDUBSW Y6, Y0, Y0
	VPMADDUBSW Y6, Y1, Y1
	VPMADDUBSW Y6, Y2, Y2
	VPMADDUBSW Y6, Y3, Y3
	VPADDW     Y2, Y0, Y0
	VPADDW     Y3, Y1, Y1
	VPADDW     Y7, Y0, Y0
	VPADDW     Y7, Y1, Y1
	VPSRLW     $2, Y0, Y0
	VPSRLW     $2, Y1, Y1
	VPACKUSWB  Y1, Y0, Y0
	VPERMQ     $0xd8, Y0, Y0
	VMOVDQU    Y0, (DI)
	ADDQ       $64, SI
	ADDQ       $64, DX
	ADDQ       $32, DI
	SUBQ       $32, CX
	JNZ        dloop
	VZEROUPPER
	RET
