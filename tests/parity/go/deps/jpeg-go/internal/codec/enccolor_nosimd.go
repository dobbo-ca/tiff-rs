//go:build purego || !(amd64 || arm64)

package codec

const encSIMD = false

// RGBAToYCbCrRow and DownsampleH2V2Row are the encoder colour kernels.
var RGBAToYCbCrRow, DownsampleH2V2Row = rgbaToYCbCrRowGo, downsampleH2V2RowGo
