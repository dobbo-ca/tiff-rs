// Package codec implements the JPEG bitstream: parsing, entropy coding,
// DCT/IDCT and the encoder tables and marker writer.
package encref

// stdLumaQuant and stdChromaQuant are Annex K.1 base tables, natural
// (row-major) order (jcparam.c:75-94).
var stdLumaQuant = [64]int{
	16, 11, 10, 16, 24, 40, 51, 61,
	12, 12, 14, 19, 26, 58, 60, 55,
	14, 13, 16, 24, 40, 57, 69, 56,
	14, 17, 22, 29, 51, 87, 80, 62,
	18, 22, 37, 56, 68, 109, 103, 77,
	24, 35, 55, 64, 81, 104, 113, 92,
	49, 64, 78, 87, 103, 121, 120, 101,
	72, 92, 95, 98, 112, 100, 103, 99,
}

var stdChromaQuant = [64]int{
	17, 18, 24, 47, 99, 99, 99, 99,
	18, 21, 26, 66, 99, 99, 99, 99,
	24, 26, 56, 99, 99, 99, 99, 99,
	47, 66, 99, 99, 99, 99, 99, 99,
	99, 99, 99, 99, 99, 99, 99, 99,
	99, 99, 99, 99, 99, 99, 99, 99,
	99, 99, 99, 99, 99, 99, 99, 99,
	99, 99, 99, 99, 99, 99, 99, 99,
}

// QualityScaling maps 1..100 to libjpeg's scale factor (jcparam.c:131-154).
func QualityScaling(quality int) int {
	if quality < 1 {
		quality = 1
	}
	if quality > 100 {
		quality = 100
	}
	if quality < 50 {
		return 5000 / quality
	}
	return 200 - quality*2
}

// ScaleQuantTable scales base by quality (jcparam.c:55-63), natural order.
// force_baseline is always on, so entries clamp to 1..255.
func ScaleQuantTable(base [64]int, quality int) [64]byte {
	scale := QualityScaling(quality)
	var out [64]byte
	for i, b := range base {
		t := (b*scale + 50) / 100
		if t <= 0 {
			t = 1
		}
		if t > 255 {
			t = 255
		}
		out[i] = byte(t)
	}
	return out
}

// LumaQuantTable returns the natural-order luma quant table for quality.
func LumaQuantTable(quality int) [64]byte {
	return ScaleQuantTable(stdLumaQuant, quality)
}

// ChromaQuantTable returns the natural-order chroma quant table for quality.
func ChromaQuantTable(quality int) [64]byte {
	return ScaleQuantTable(stdChromaQuant, quality)
}

// QuantNatural widens a natural-order quant table for the FDCT quantizer.
func QuantNatural(t [64]byte) [64]int32 {
	var out [64]int32
	for i, v := range t {
		out[i] = int32(v)
	}
	return out
}
