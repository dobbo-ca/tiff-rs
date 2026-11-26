package codec

// H2V1FancyRow is h2v1FancyRow over a whole row: dsw = len(row) >= 2,
// w = len(dst), 2*len(row) or one less.
func H2V1FancyRow(dst, row []byte) { h2v1FancyRow(dst, row, len(row), len(dst)) }

// H2V2FancyRow is h2v2FancyRow over a whole row, cur and other as
// rowCur and rowOther; len(other) >= len(cur) >= 2.
func H2V2FancyRow(dst, cur, other []byte) { h2v2FancyRow(dst, cur, other, len(cur), len(dst)) }
