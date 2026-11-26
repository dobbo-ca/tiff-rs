//go:build !purego

#include "textflag.h"

// Per block of n bytes in B-byte steps:
//   s2 += n*s1 + B*sum(s1 before each step) + sum((B-j)*p[j])
//   s1 += sum(p)
// n*B fits NMAX, so no 32-bit lane overflows.
// R8 s1, R9 s2, SI p, BX bytes left, R10 block end.

// PMADDUBSW weights 64..1.
DATA weights<>+0x00(SB)/8, $0x393a3b3c3d3e3f40
DATA weights<>+0x08(SB)/8, $0x3132333435363738
DATA weights<>+0x10(SB)/8, $0x292a2b2c2d2e2f30
DATA weights<>+0x18(SB)/8, $0x2122232425262728
DATA weights<>+0x20(SB)/8, $0x191a1b1c1d1e1f20
DATA weights<>+0x28(SB)/8, $0x1112131415161718
DATA weights<>+0x30(SB)/8, $0x090a0b0c0d0e0f10
DATA weights<>+0x38(SB)/8, $0x0102030405060708
GLOBL weights<>(SB), RODATA|NOPTR, $64

DATA ones<>+0x00(SB)/8, $0x0001000100010001
DATA ones<>+0x08(SB)/8, $0x0001000100010001
DATA ones<>+0x10(SB)/8, $0x0001000100010001
DATA ones<>+0x18(SB)/8, $0x0001000100010001
GLOBL ones<>(SB), RODATA|NOPTR, $32

#define LOAD_ARGS \
	MOVL  adler+0(FP), R8; \
	MOVQ  p_base+8(FP), SI; \
	MOVQ  p_len+16(FP), BX; \
	MOVL  R8, R9; \
	ANDL  $0xffff, R8; \
	SHRL  $16, R9; \
	MOVL  $65521, R11

// CX = block size; adds n*s1 to s2.
#define BLOCK_START(max) \
	MOVQ    $max, CX; \
	CMPQ    BX, CX; \
	CMOVQCS BX, CX; \
	SUBQ    CX, BX; \
	MOVQ    CX, AX; \
	IMULQ   R8, AX; \
	ADDQ    AX, R9; \
	LEAQ    (SI)(CX*1), R10

#define REDUCE \
	MOVQ R8, AX; \
	XORL DX, DX; \
	DIVQ R11; \
	MOVQ DX, R8; \
	MOVQ R9, AX; \
	XORL DX, DX; \
	DIVQ R11; \
	MOVQ DX, R9

// func updateSSSE3(adler uint32, p []byte) uint32
TEXT ·updateSSSE3(SB), NOSPLIT, $0-36
	LOAD_ARGS
	MOVOU weights<>+32(SB), X12
	MOVOU weights<>+48(SB), X13
	MOVOU ones<>(SB), X14
	PXOR  X7, X7

ssse3Block:
	BLOCK_START(5536) // 173*32
	PXOR X0, X0       // s1 sums
	PXOR X1, X1       // prefix s1 sums
	PXOR X2, X2       // weighted sums

ssse3Loop:
	MOVOU     (SI), X3
	MOVOU     16(SI), X4
	PADDL     X0, X1
	MOVO      X3, X5
	MOVO      X4, X6
	PSADBW    X7, X3
	PSADBW    X7, X4
	PADDL     X4, X3
	PADDL     X3, X0
	PMADDUBSW X12, X5
	PMADDUBSW X13, X6
	PMADDWL   X14, X5
	PMADDWL   X14, X6
	PADDL     X6, X5
	PADDL     X5, X2
	ADDQ      $32, SI
	CMPQ      SI, R10
	JNE       ssse3Loop

	PSLLL  $5, X1
	PADDL  X1, X2
	PSHUFD $0x4e, X0, X3
	PADDL  X3, X0        // PSADBW left lanes 1, 3 zero
	MOVQ   X0, AX
	ADDQ   AX, R8
	PSHUFD $0x4e, X2, X3
	PADDL  X3, X2
	PSHUFD $0xb1, X2, X3
	PADDL  X3, X2        // every lane holds the sum
	MOVQ   X2, AX
	SHRQ   $32, AX
	ADDQ   AX, R9
	REDUCE
	TESTQ  BX, BX
	JNZ    ssse3Block
	SHLL   $16, R9
	ORL    R9, R8
	MOVL   R8, ret+32(FP)
	RET

// func updateAVX2(adler uint32, p []byte) uint32
TEXT ·updateAVX2(SB), NOSPLIT, $0-36
	LOAD_ARGS
	VMOVDQU weights<>+0(SB), Y12
	VMOVDQU weights<>+32(SB), Y13
	VMOVDQU ones<>(SB), Y14
	VPXOR   Y7, Y7, Y7

avx2Block:
	BLOCK_START(5504) // 86*64
	VPXOR Y0, Y0, Y0
	VPXOR Y1, Y1, Y1
	VPXOR Y2, Y2, Y2

avx2Loop:
	VMOVDQU    (SI), Y3
	VMOVDQU    32(SI), Y4
	VPADDD     Y0, Y1, Y1
	VPSADBW    Y7, Y3, Y5
	VPSADBW    Y7, Y4, Y6
	VPADDD     Y5, Y6, Y5
	VPADDD     Y5, Y0, Y0
	VPMADDUBSW Y12, Y3, Y3
	VPMADDUBSW Y13, Y4, Y4
	VPMADDWD   Y14, Y3, Y3
	VPMADDWD   Y14, Y4, Y4
	VPADDD     Y3, Y4, Y3
	VPADDD     Y3, Y2, Y2
	ADDQ       $64, SI
	CMPQ       SI, R10
	JNE        avx2Loop

	VPSLLD       $6, Y1, Y1
	VPADDD       Y1, Y2, Y2
	VEXTRACTI128 $1, Y0, X3
	VPADDD       X3, X0, X0
	VPSHUFD      $0x4e, X0, X3
	VPADDD       X3, X0, X0
	VMOVD        X0, AX
	ADDQ         AX, R8
	VEXTRACTI128 $1, Y2, X3
	VPADDD       X3, X2, X2
	VPSHUFD      $0x4e, X2, X3
	VPADDD       X3, X2, X2
	VPSHUFD      $0xb1, X2, X3
	VPADDD       X3, X2, X2
	VMOVD        X2, AX
	ADDQ         AX, R9
	REDUCE
	TESTQ        BX, BX
	JNZ          avx2Block
	VZEROUPPER
	SHLL         $16, R9
	ORL          R9, R8
	MOVL         R8, ret+32(FP)
	RET

// VNNI-512 loop. Rounds of G = 128 bytes, after a 64-byte head round
// when n/64 is odd; s1 before each round is summed into P:
//   s2 += n*s1 + sum((G-1-g)*p[g]) + sum(p) + 128*P
//   s1 += sum(p)
// Weights 127..0 fit VPDPBUSD's int8. For n <= 64 KiB the weighted
// sum fits 32 bits; s1 and P are VPSADBW qwords.

DATA vnniWeights<>+0x00(SB)/8, $0x78797a7b7c7d7e7f
DATA vnniWeights<>+0x08(SB)/8, $0x7071727374757677
DATA vnniWeights<>+0x10(SB)/8, $0x68696a6b6c6d6e6f
DATA vnniWeights<>+0x18(SB)/8, $0x6061626364656667
DATA vnniWeights<>+0x20(SB)/8, $0x58595a5b5c5d5e5f
DATA vnniWeights<>+0x28(SB)/8, $0x5051525354555657
DATA vnniWeights<>+0x30(SB)/8, $0x48494a4b4c4d4e4f
DATA vnniWeights<>+0x38(SB)/8, $0x4041424344454647
DATA vnniWeights<>+0x40(SB)/8, $0x38393a3b3c3d3e3f
DATA vnniWeights<>+0x48(SB)/8, $0x3031323334353637
DATA vnniWeights<>+0x50(SB)/8, $0x28292a2b2c2d2e2f
DATA vnniWeights<>+0x58(SB)/8, $0x2021222324252627
DATA vnniWeights<>+0x60(SB)/8, $0x18191a1b1c1d1e1f
DATA vnniWeights<>+0x68(SB)/8, $0x1011121314151617
DATA vnniWeights<>+0x70(SB)/8, $0x08090a0b0c0d0e0f
DATA vnniWeights<>+0x78(SB)/8, $0x0001020304050607
GLOBL vnniWeights<>(SB), RODATA|NOPTR, $128

// r %= 65521 for r < 2^63: q = hi64(r*ceil(2^79/65521)) >> 15.
#define MOD_MUL(r) \
	MOVQ  $0x800780708697e2e7, AX; \
	MULQ  r; \
	SHRQ  $15, DX; \
	IMULQ R11, DX; \
	SUBQ  DX, r

// Y4 weighted dwords, Y8 s1 qwords, Y9 P qwords into R8, R9; reduce.
#define VNNI_FINISH \
	VPSLLQ       $7, Y9, Y9; \
	VPADDQ       Y8, Y9, Y9; \
	VEXTRACTI128 $1, Y8, X0; \
	VPADDQ       X0, X8, X8; \
	VPSHUFD      $0x4e, X8, X0; \
	VPADDQ       X0, X8, X8; \
	VMOVQ        X8, AX; \
	ADDQ         AX, R8; \
	VEXTRACTI128 $1, Y9, X0; \
	VPADDQ       X0, X9, X9; \
	VPSHUFD      $0x4e, X9, X0; \
	VPADDQ       X0, X9, X9; \
	VMOVQ        X9, AX; \
	ADDQ         AX, R9; \
	VEXTRACTI128 $1, Y4, X0; \
	VPADDD       X0, X4, X4; \
	VPSHUFD      $0x4e, X4, X0; \
	VPADDD       X0, X4, X4; \
	VPSHUFD      $0xb1, X4, X0; \
	VPADDD       X0, X4, X4; \
	VMOVD        X4, AX; \
	ADDQ         AX, R9; \
	MOD_MUL(R8); \
	MOD_MUL(R9)

// func updateVNNI512(adler uint32, p []byte) uint32
TEXT ·updateVNNI512(SB), NOSPLIT, $0-36
	LOAD_ARGS
	VMOVDQU64 vnniWeights<>+0(SB), Z12
	VMOVDQU64 vnniWeights<>+64(SB), Z13
	VPXORQ    Z14, Z14, Z14

v512Block:
	BLOCK_START(65536)
	VPXORQ Z4, Z4, Z4 // Z4-Z7 weighted sums
	VPXORQ Z5, Z5, Z5
	VPXORQ Z6, Z6, Z6
	VPXORQ Z7, Z7, Z7
	VPXORQ Z8, Z8, Z8 // s1
	VPXORQ Z9, Z9, Z9 // P
	TESTQ  $64, CX
	JZ     v512Pair
	VMOVDQU64 (SI), Z0
	VPDPBUSD  Z13, Z0, Z4
	VPSADBW   Z14, Z0, Z8
	ADDQ      $64, SI

v512Pair:
	TESTQ     $128, CX
	JZ        v512Next
	VMOVDQU64 (SI), Z0
	VMOVDQU64 64(SI), Z1
	VPADDQ    Z8, Z9, Z9
	VPDPBUSD  Z12, Z0, Z5
	VPDPBUSD  Z13, Z1, Z6
	VPSADBW   Z14, Z0, Z0
	VPSADBW   Z14, Z1, Z1
	VPADDQ    Z0, Z1, Z0
	VPADDQ    Z0, Z8, Z8
	ADDQ      $128, SI
	JMP       v512Next

v512Loop:
	VMOVDQU64 (SI), Z0
	VMOVDQU64 64(SI), Z1
	VMOVDQU64 128(SI), Z2
	VMOVDQU64 192(SI), Z3
	VPADDQ    Z8, Z9, Z9
	VPDPBUSD  Z12, Z0, Z4
	VPDPBUSD  Z13, Z1, Z5
	VPDPBUSD  Z12, Z2, Z6
	VPDPBUSD  Z13, Z3, Z7
	VPSADBW   Z14, Z0, Z0
	VPSADBW   Z14, Z1, Z1
	VPSADBW   Z14, Z2, Z2
	VPSADBW   Z14, Z3, Z3
	VPADDQ    Z0, Z1, Z0
	VPADDQ    Z2, Z3, Z2
	VPADDQ    Z0, Z8, Z8
	VPADDQ    Z8, Z9, Z9
	VPADDQ    Z2, Z8, Z8
	ADDQ      $256, SI

v512Next:
	CMPQ SI, R10
	JNE  v512Loop

	VPADDD        Z5, Z4, Z4
	VPADDD        Z7, Z6, Z6
	VPADDD        Z6, Z4, Z4
	VEXTRACTI64X4 $1, Z4, Y0
	VPADDD        Y0, Y4, Y4
	VEXTRACTI64X4 $1, Z8, Y0
	VPADDQ        Y0, Y8, Y8
	VEXTRACTI64X4 $1, Z9, Y0
	VPADDQ        Y0, Y9, Y9
	VNNI_FINISH
	TESTQ         BX, BX
	JNZ           v512Block
	VZEROUPPER
	SHLL          $16, R9
	ORL           R9, R8
	MOVL          R8, ret+32(FP)
	RET

// func modMul(r uint64) uint64
TEXT ·modMul(SB), NOSPLIT, $0-16
	MOVQ r+0(FP), CX
	MOVL $65521, R11
	MOD_MUL(CX)
	MOVQ CX, ret+8(FP)
	RET

// func cpuFeatures() (ssse3, avx2, vnni bool)
TEXT ·cpuFeatures(SB), NOSPLIT, $0-3
	MOVB  $0, avx2+1(FP)
	MOVB  $0, vnni+2(FP)
	XORL  AX, AX
	XORL  CX, CX
	CPUID
	MOVL  AX, R8
	MOVL  $1, AX
	XORL  CX, CX
	CPUID
	MOVL  CX, R9
	SHRL  $9, CX
	ANDL  $1, CX
	MOVB  CX, ssse3+0(FP)
	CMPL  R8, $7
	JCS   done
	ANDL  $0x18000000, R9 // OSXSAVE, AVX
	CMPL  R9, $0x18000000
	JNE   done
	XORL  CX, CX
	XGETBV
	ANDL  $6, AX          // OS saves XMM and YMM
	CMPL  AX, $6
	JNE   done
	MOVL  $7, AX
	XORL  CX, CX
	CPUID
	MOVL  BX, R10
	SHRL  $5, BX
	ANDL  $1, BX
	MOVB  BX, avx2+1(FP)
	ANDL  $0xc0010000, R10 // AVX512F, BW, VL
	CMPL  R10, $0xc0010000
	JNE   done
	BTL   $11, CX          // AVX512_VNNI
	JCC   done
	XORL  CX, CX
	XGETBV
	ANDL  $0xe6, AX        // and opmask, ZMM state
	CMPL  AX, $0xe6
	JNE   done
	MOVB  $1, vnni+2(FP)

done:
	RET
