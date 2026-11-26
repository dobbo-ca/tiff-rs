// Package jpeg is a pure Go JPEG codec matching libjpeg-turbo 3.2.0 bit
// for bit. See docs/design.md for the full API. The code lives in
// internal/jpeg; this file re-exports it.
package jpeg

import (
	"context"
	"image"
	"io"

	"github.com/dobbo-ca/jpeg-go/internal/codec"
	ijpeg "github.com/dobbo-ca/jpeg-go/internal/jpeg"
)

var (
	// ErrUnsupported is returned for streams or options out of scope; see
	// docs/design.md section 1.
	ErrUnsupported = ijpeg.ErrUnsupported
	// ErrTruncated wraps a decode error from entropy data that ran out,
	// held a Huffman code no table matches, or whose restart markers were
	// out of sequence (turbo's warnings). The image still comes back.
	ErrTruncated = ijpeg.ErrTruncated
	// ErrAfterScan wraps a fatal marker after a single-scan frame's last
	// row. Decode still returns the image, as libjpeg-turbo has filled
	// every row before jpeg_finish_decompress fails. The error also wraps
	// its cause (ErrUnsupported for a SOFn, say) and ErrTruncated if the
	// scan lost data, so check ErrAfterScan first to keep the image.
	ErrAfterScan = ijpeg.ErrAfterScan
	// ErrMemoryLimit is returned when a decode is refused because others
	// hold the budget, but it would fit alone (docs/design.md section 6).
	ErrMemoryLimit = ijpeg.ErrMemoryLimit
	// ErrLimit is returned when a call's own Limits are exceeded; permanent.
	ErrLimit = ijpeg.ErrLimit
)

type (
	// Decoder keeps its buffers between calls; the zero value is ready.
	Decoder = ijpeg.Decoder
	// DecodeOptions selects output format, scale, region and tables.
	DecodeOptions = ijpeg.DecodeOptions
	// Limits bounds one call (DecodeOptions.Limits, DecodeConfigLimits).
	Limits = ijpeg.Limits
	// LimitError names the Limits field exceeded and its values; it wraps ErrLimit.
	LimitError = ijpeg.LimitError
	// Format selects Decoder's output type; see docs/design.md section 2.
	Format = ijpeg.Format
	// Recon replaces the IDCT and colour conversion (Scale 1, no Region).
	Recon = ijpeg.Recon
	// Config is a JPEG frame's shape and colour space, from DecodeConfig.
	Config = ijpeg.Config
	// Coefficients is a frame's quantized DCT blocks, from DecodeCoefficients.
	Coefficients = ijpeg.Coefficients
	// ComponentCoefficients is one component's blocks and quant table.
	ComponentCoefficients = ijpeg.ComponentCoefficients
	// RowReader hands out a frame's RGBA rows one MCU row at a time over its
	// coefficients; see NewRowReader.
	RowReader = ijpeg.RowReader
	// Encoder keeps its buffers between calls; the zero value is ready.
	Encoder = ijpeg.Encoder
	// EncodeOptions sets quality and chroma sampling.
	EncodeOptions = ijpeg.EncodeOptions
)

const (
	FormatYCbCr = ijpeg.FormatYCbCr // *image.YCbCr at the stream's sampling; *image.Gray for 1 component
	FormatRGBA  = ijpeg.FormatRGBA  // *image.RGBA, alpha 255
	FormatGray  = ijpeg.FormatGray  // *image.Gray: Y for YCbCr streams, turbo's rgb_gray for RGB streams
)

// Decode is Decoder.Decode on pooled scratch; the returned image is a copy
// the caller owns, since the pooled Decoder is reused by the next call.
// With Owned and FormatRGBA it is the decode's own image, not a copy;
// with Dst it is Dst.
func Decode(ctx context.Context, src []byte, o *DecodeOptions) (image.Image, error) {
	return ijpeg.Decode(ctx, src, o)
}

// DecodeConfig parses src (an abbreviated tables stream, when non-nil,
// supplies DQT/DHT) up to SOS and returns the frame's shape and colour
// space. It allocates nothing.
func DecodeConfig(src, tables []byte) (Config, error) { return ijpeg.DecodeConfig(src, tables) }

// DecodeConfigLimits is DecodeConfig that refuses, with ErrLimit, a frame
// over l.MaxPixels.
func DecodeConfigLimits(src, tables []byte, l Limits) (Config, error) {
	return ijpeg.DecodeConfigLimits(src, tables, l)
}

// NewRowReader is for frames Decode would hold as coefficients, planes and
// RGBA at once (progressive, multi-scan): it keeps the coefficients only
// and converts one MCU row per Next, so its peak is the coefficients plus
// one band (about 6 B/px at 4:4:4). Its RowReaderNeed bytes are reserved
// from the process limiter until Close and are capped at 1100 MiB, which
// Limits.MaxBytes can only lower; Decode's 1 GiB cap is unchanged. Rows
// equal Decode's FormatRGBA output byte for byte. See ijpeg.NewRowReader.
func NewRowReader(ctx context.Context, src, tables []byte, l Limits) (*RowReader, error) {
	return ijpeg.NewRowReader(ctx, src, tables, l)
}

// RowReaderNeed is the bytes NewRowReader reserves for src's frame, from its
// headers alone.
func RowReaderNeed(src, tables []byte) (int64, error) { return ijpeg.RowReaderNeed(src, tables) }

// MaxRowReaderNeed is the most RowReaderNeed can be for any frame of at
// most maxPixels pixels.
func MaxRowReaderNeed(maxPixels int64) int64 { return ijpeg.MaxRowReaderNeed(maxPixels) }

// Encode is Encoder.Encode on pooled scratch.
func Encode(ctx context.Context, w io.Writer, m image.Image, o *EncodeOptions) error {
	return ijpeg.Encode(ctx, w, m, o)
}

// SetMemoryLimit sets the reservation limit and returns the previous one.
// n <= 0 means none. See docs/design.md section 6.
func SetMemoryLimit(n int64) int64 { return ijpeg.SetMemoryLimit(n) }

// LedgerSlack is the fixed state outside Limits.MaxBytes: a bounded call
// allocates at most MaxBytes + LedgerSlack.
const LedgerSlack = ijpeg.LedgerSlack

// H2V1FancyRow applies libjpeg-turbo's h2v1 fancy upsample formula
// (jdsample.c h2v1_fancy_upsample) to one chroma row of n = len(row)
// samples. n must be >= 2: n == 1 panics, so callers handle it.
// len(dst) must be 2n or 2n-1: a shorter dst panics, possibly after a
// partial write, and a longer one is filled only to 2n. The output equals
// libjpeg-turbo's for n > 2; at n == 2 libjpeg-turbo and this package's
// decoder use box upsampling, while H2V1FancyRow still applies the fancy
// formula. It runs AVX2 on amd64 CPUs with AVX2 and NEON on arm64; it
// runs Go on amd64 without AVX2, on other arches, under purego and with
// JPEG_NOSIMD set.
func H2V1FancyRow(dst, row []byte) { codec.H2V1FancyRow(dst, row) }

// H2V2FancyRow is the h2v2 fancy upsample formula (h2v2_fancy_upsample)
// for one output row, from its chroma row cur and the nearer neighbour
// row other; len(other) >= len(cur) = n. The n, len(dst), n == 2 and
// dispatch rules of H2V1FancyRow apply.
func H2V2FancyRow(dst, cur, other []byte) { codec.H2V2FancyRow(dst, cur, other) }
