package main

import (
	"bufio"
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/dobbo-ca/jpeg-go"
	tiff "github.com/dobbo-ca/tiff-go"
)

// The real-file comparison (docs/parity.md, "Real files (manual)"). examples/quality.rs does the
// same with the Rust crate, and the two output trees must be equal.
const (
	// cell is the side of the fixed grid the page is cut into, whatever the page's tiling.
	cell = 256
	// maxRing bounds the memory of the one cell row that is kept (cell rows of the page).
	maxRing = 1 << 30
	// qMemLimit is jpeg-go's process limit here; the Rust example gives max_alloc the same value.
	qMemLimit = 2 << 30
)

// qLimits fits a 51200 x 38144 page; the other caps are the defaults.
var qLimits = tiff.Limits{MaxPixels: 1 << 32}

// qtiles writes, for every page of every slide, at most max cells of the grid as PPM or PGM files
// in out, and one line per page that it skips or that fails to logPath (stderr when empty).
func qtiles(out, logPath string, max int, slides []string) error {
	jpeg.SetMemoryLimit(qMemLimit)
	if err := os.MkdirAll(out, 0o755); err != nil {
		return err
	}
	lf := os.Stderr
	if logPath != "" {
		f, err := os.Create(logPath)
		if err != nil {
			return err
		}
		defer f.Close()
		lf = f
	}
	lw := bufio.NewWriter(lf)
	for _, s := range slides {
		if err := qslide(out, s, max, lw); err != nil {
			return err
		}
	}
	return lw.Flush()
}

func qslide(out, slide string, max int, lw *bufio.Writer) error {
	f, err := os.Open(slide)
	if err != nil {
		return err
	}
	defer f.Close()
	st, err := f.Stat()
	if err != nil {
		return err
	}
	tf, err := tiff.Open(f, st.Size(), qLimits)
	if err != nil {
		fmt.Fprintf(lw, "%s open %s\n", slide, class(err))
		return nil
	}
	stem := strings.NewReplacer(".", "_", "/", "_").Replace(filepath.Base(slide))
	for k := 0; k < tf.NumPages(); k++ {
		p, err := tf.Page(k)
		if err != nil {
			fmt.Fprintf(lw, "%s ifd%d %s\n", slide, k, class(err))
			continue
		}
		if reason, err := qpage(filepath.Join(out, fmt.Sprintf("%s.ifd%d", stem, k)), max, p); err != nil {
			return err
		} else if reason != "" {
			fmt.Fprintf(lw, "%s ifd%d %s\n", slide, k, reason)
		}
	}
	return nil
}

// qpage decodes page p and writes its sampled cells to <base>.<cell:06>.ppm or .pgm. It returns the
// class of the reason when the page is skipped or its decode fails (the cells written before the
// failure stay), and an error only for a failed write.
func qpage(base string, max int, p *tiff.Page) (string, error) {
	in := p.Info()
	w, h, spp := in.Width, in.Height, in.SamplesPerPixel
	if in.BitsPerSample != 8 || (spp != 1 && spp != 3) || len(in.ExtraSamples) != 0 ||
		in.Photometric == tiff.Palette || in.RowBytes != int64(w*spp) {
		return "layout", nil
	}
	cx, cy := (w+cell-1)/cell, (h+cell-1)/cell
	n := cx * cy
	m := min(n, max)
	if m <= 0 {
		return "", nil
	}
	// sel[r] lists the grid columns sampled in cell row r: cell floor(j*n/m) for j < m.
	sel := make([][]int, cy)
	for j := 0; j < m; j++ {
		c := j * n / m
		sel[c/cx] = append(sel[c/cx], c%cx)
	}
	last := (m - 1) * n / m / cx
	rb := w * spp
	if int64(rb)*int64(min(h, cell)) > maxRing {
		return "limit:alloc", nil
	}
	ring := make([]byte, rb*min(h, cell))
	magic := "P6"
	if spp == 1 {
		magic = "P5"
	}
	ext := ".ppm"
	if spp == 1 {
		ext = ".pgm"
	}
	var werr error
	done := false
	// flush writes the sampled cells of cell row r from the ring.
	flush := func(r int) error {
		ch := min(h-r*cell, cell)
		for _, c := range sel[r] {
			x0 := c * cell
			cw := min(w-x0, cell)
			f, err := os.Create(fmt.Sprintf("%s.%06d%s", base, r*cx+c, ext))
			if err != nil {
				return err
			}
			bw := bufio.NewWriter(f)
			fmt.Fprintf(bw, "%s\n%d %d\n255\n", magic, cw, ch)
			for i := 0; i < ch; i++ {
				bw.Write(ring[i*rb+x0*spp : i*rb+(x0+cw)*spp])
			}
			if err := bw.Flush(); err != nil {
				f.Close()
				return err
			}
			if err := f.Close(); err != nil {
				return err
			}
		}
		return nil
	}
	err := p.Rows(context.Background(), func(y int, band []byte) error {
		for i := 0; i+rb <= len(band); i += rb {
			row := y + i/rb
			if row >= h {
				break
			}
			r := row / cell
			if len(sel[r]) == 0 {
				continue
			}
			copy(ring[(row%cell)*rb:], band[i:i+rb])
			if row == min(h, (r+1)*cell)-1 {
				if werr = flush(r); werr != nil {
					return errStop
				}
				if r == last {
					done = true
					return errStop
				}
			}
		}
		return nil
	})
	if werr != nil {
		return "", werr
	}
	if err != nil && !done {
		return class(err), nil
	}
	return "", nil
}
