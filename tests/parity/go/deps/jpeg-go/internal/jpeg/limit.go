package jpeg

import (
	"context"
	"errors"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"runtime"
	"runtime/debug"
	"strconv"
	"strings"
	"sync"

	"github.com/dobbo-ca/jpeg-go/internal/codec"
	"github.com/dobbo-ca/jpeg-go/internal/ledger"
)

// ErrMemoryLimit is returned when a decode is refused because others
// hold the budget, but it would fit alone (docs/design.md section 6).
var ErrMemoryLimit = errors.New("jpeg: memory limit held by other decodes (transient)")

// ErrLimit is returned when a call's own Limits are exceeded. Permanent:
// retrying the same call fails the same way.
var ErrLimit = ledger.ErrLimit

// Limits bounds one call; zero fields mean no bound. Checked from SOF,
// before any allocation, and again by every input-sized allocation
// (internal/ledger).
type Limits struct {
	MaxPixels int64 // frame Width*Height
	MaxBytes  int64 // most this call may allocate, plus LedgerSlack of fixed state
}

// LedgerSlack is the fixed state outside the ledger (decoder struct,
// tables, readers): a decode allocates at most MaxBytes + LedgerSlack.
const LedgerSlack = 1 << 16

// LimitError says which per-call limit was exceeded. It wraps ErrLimit.
type LimitError = ledger.LimitError

// Ledger is l's allocation ledger: nil when MaxBytes is unset.
func (l Limits) Ledger() *ledger.Ledger { return ledger.New(l.MaxBytes) }

func (l Limits) checkPixels(w, h int) error {
	if n := int64(w) * int64(h); l.MaxPixels > 0 && n > l.MaxPixels {
		return &LimitError{Name: "MaxPixels", Got: n, Max: l.MaxPixels}
	}
	return nil
}

func (l Limits) checkBytes(need int64) error {
	if l.MaxBytes > 0 && need > l.MaxBytes {
		return &LimitError{Name: "MaxBytes", Got: need, Max: l.MaxBytes}
	}
	return nil
}

// maxNeed caps a single decode whatever the limit (docs/design.md section
// 6): bounds hostile SOF sizes when no limit is set, since a truncated
// stream still decodes at full size.
const maxNeed = 1 << 30

// maxRegion bounds Region coordinates: pad sizes cannot overflow.
const maxRegion = 1 << 29

// limWaiter is a decode's FIFO queue node. It lives in the Decoder (or the
// pooled Decoder for package-level Decode). wake is made on the node's
// first reservation, so waiting allocates nothing. Only the queue head
// is signalled, so one buffered size-1 channel is enough.
type limWaiter struct {
	need       int64
	next, prev *limWaiter
	queued     bool
	wake       chan struct{}
}

var budget struct {
	mu         sync.Mutex
	explicit   bool
	limit      int64 // valid when explicit
	used       int64
	head, tail *limWaiter
}

// SetMemoryLimit sets the reservation limit and returns the previous one.
// n <= 0 means none. See docs/design.md section 6.
func SetMemoryLimit(n int64) int64 {
	if n <= 0 {
		n = math.MaxInt64
	}
	budget.mu.Lock()
	prev := currentLimit()
	budget.explicit, budget.limit = true, n
	wakeAllLocked()
	budget.mu.Unlock()
	return prev
}

// maxWorkers caps DecodeOptions.Workers; more never helps.
const maxWorkers = 256

// fitsLimit reports whether need passes reserve's cap and limit checks.
func fitsLimit(need int64) bool {
	budget.mu.Lock()
	defer budget.mu.Unlock()
	return need <= maxNeed && need <= currentLimit()
}

// currentLimit is the effective limit; budget.mu held.
func currentLimit() int64 {
	if budget.explicit {
		return budget.limit
	}
	return defaultLimit("")
}

// wakeLocked wakes the queue head to recheck whether it now fits; budget.mu
// held. Only the head can ever be admitted (FIFO), so waking anyone else
// would just have them go back to sleep. The send is non-blocking: a
// pending signal already means "recheck", so a second one is a no-op.
func wakeLocked() {
	if budget.head == nil {
		return
	}
	select {
	case budget.head.wake <- struct{}{}:
	default:
	}
}

// wakeAllLocked wakes every queued waiter, not just the head; budget.mu
// held. Used when the limit changes, since a waiter behind the head may
// now be permanently over it and must not wait for its turn to find out.
func wakeAllLocked() {
	for w := budget.head; w != nil; w = w.next {
		select {
		case w.wake <- struct{}{}:
		default:
		}
	}
}

func queuePush(w *limWaiter) {
	w.next, w.prev = nil, budget.tail
	if budget.tail != nil {
		budget.tail.next = w
	} else {
		budget.head = w
	}
	budget.tail = w
	w.queued = true
}

func queueRemove(w *limWaiter) {
	if !w.queued {
		return
	}
	if w.prev != nil {
		w.prev.next = w.next
	} else {
		budget.head = w.next
	}
	if w.next != nil {
		w.next.prev = w.prev
	} else {
		budget.tail = w.prev
	}
	w.next, w.prev, w.queued = nil, nil, false
}

// reserve admits need bytes for d's decode, waiting FIFO under ctx when
// others hold the budget. It never allocates on the uncontended path: w
// is a field of d, not a new node. Call d.releaseReservation to give the
// bytes back.
func (d *Decoder) reserve(ctx context.Context, need int64) error {
	if need > maxNeed {
		return fmt.Errorf("jpeg: decode needs %d bytes, over the 1 GiB cap; use Region", need)
	}
	return d.reserveWait(ctx, need)
}

// reserveWait is reserve after the per-decode cap check, which the caller
// owns (a RowReader has its own cap).
func (d *Decoder) reserveWait(ctx context.Context, need int64) error {
	w := &d.limWait
	budget.mu.Lock()
	limit := currentLimit()
	if need > limit {
		budget.mu.Unlock()
		return fmt.Errorf("jpeg: decode needs %d bytes, over the %d byte memory limit", need, limit)
	}
	if w.wake == nil {
		w.wake = make(chan struct{}, 1) // made once so waits never allocate
	}
	if budget.head == nil && budget.used+need <= limit {
		budget.used += need
		w.need = need
		budget.mu.Unlock()
		return nil
	}
	w.need = need
	queuePush(w)
	budget.mu.Unlock()

	for {
		select {
		case <-w.wake:
		case <-ctx.Done():
			budget.mu.Lock()
			queueRemove(w)
			wakeLocked() // the new head, if any, may now fit
			budget.mu.Unlock()
			w.need = 0
			return fmt.Errorf("jpeg: %w: %w", ctx.Err(), ErrMemoryLimit)
		}

		budget.mu.Lock()
		limit = currentLimit()
		if need > limit {
			// Lowered below need: fail rather than block the queue.
			queueRemove(w)
			wakeLocked()
			budget.mu.Unlock()
			w.need = 0
			return fmt.Errorf("jpeg: decode needs %d bytes, over the %d byte memory limit", need, limit)
		}
		if budget.head == w && budget.used+need <= limit {
			queueRemove(w)
			budget.used += need
			wakeLocked() // the new head, if any, may now fit
			budget.mu.Unlock()
			return nil
		}
		budget.mu.Unlock()
	}
}

// releaseReservation gives back d's reservation. A no-op if d never
// reserved (need 0), so callers can defer it unconditionally.
func (d *Decoder) releaseReservation() {
	if d.limWait.need == 0 {
		return
	}
	budget.mu.Lock()
	budget.used -= d.limWait.need
	d.limWait.need = 0
	wakeLocked()
	budget.mu.Unlock()
}

// decodeNeed is the byte size Decode's planes and RGBA buffer will use for
// [first,last] (docs/design.md section 6), mirroring decodeScan's own
// plane-sizing formula (internal/codec/decode.go) so the estimate covers
// what Decode actually touches, computed before any allocation. out is the
// bytes of the returned image's buffers (what package Decode copies). need
// includes ext, the page rounding that only Limits.MaxBytes counts: the
// process limiter and the 1 GiB cap use need-ext, as before Limits.
func decodeNeed(f *codec.Frame, hmax, vmax, minSize int, raw, grayOnly bool, first, last int, wantRGBA bool, rgbaW, rgbaH int) (need, out, ext int64) {
	const fixed = 1 << 16 // tables, frame state, scratch (docs/design.md section 6)
	need = int64(fixed)
	for ci := 0; ci < f.NComp; ci++ {
		c := f.Comp[ci]
		if f.Progressive && grayOnly && ci > 0 {
			// Refine scans read skipped planes.
			need += int64(ceilDiv(f.Width, hmax*8)*int(c.H)) * int64((last-first+1)*int(c.V)) * 128
		}
		if grayOnly && ci > 0 {
			continue
		}
		bs := minSize
		if !raw {
			bs = codec.ComponentBlockSize(int(c.H), int(c.V), hmax, vmax, minSize)
		}
		w := ceilDiv(f.Width, hmax*8) * int(c.H)
		h := (last - first + 1) * int(c.V)
		ext += 2 * 8192 // the plane and its coefficients each round up to whole 8 KiB pages
		need += 2 * 8192
		out += int64(w) * int64(h) * int64(bs) * int64(bs)
		need += int64(w) * int64(h) * int64(bs) * int64(bs)
		if f.ScanN < f.NComp || f.Progressive {
			need += int64(w) * int64(h) * 128 // coefficients kept across scans
		}
	}
	if wantRGBA {
		out = int64(rgbaW) * int64(rgbaH) * 4
		need += out
	}
	return need, out, ext
}

// padNeed is Decoder.pad's own buffer size, mirroring its three branches
// exactly (T09 fix: pad allocates a Region-sized buffer whenever a Region
// runs past the image edge, and decodeNeed must count it or a hostile
// Region bypasses both the limit and the 1 GiB cap).
func padNeed(o *DecodeOptions, f *codec.Frame, grayOnly bool) int64 {
	r := o.Region
	w, h := int64(r.Dx()), int64(r.Dy())
	switch {
	case o.Format == FormatRGBA:
		return 4 * w * h
	case grayOnly, f.RGB:
		return w * h
	default:
		cw, ch := chromaRect(subsampleRatio(f.Comp[0].H, f.Comp[0].V), r)
		return w*h + 2*int64(cw)*int64(ch)
	}
}

// cgroupMax is the cgroup memory limit, read once: a file read per decode
// costs more than a small decode.
var cgroupMax = sync.OnceValue(func() int64 { return cgroupMaxFrom(cgroupRoot(), "/sys/fs/cgroup") })

// cgroupMaxFrom falls back on limitless roots.
func cgroupMaxFrom(root, fallback string) int64 {
	if l := cgroupLimit(root); l > 0 {
		return l
	}
	return cgroupLimit(fallback)
}

// cgroupRoot avoids shared-namespace host limits.
func cgroupRoot() string {
	if b, err := os.ReadFile("/proc/self/cgroup"); err == nil {
		if p, ok := cgroupV2Path(b); ok {
			return filepath.Join("/sys/fs/cgroup", p)
		}
	}
	return "/sys/fs/cgroup"
}

// cgroupV2Path reads the "0::" line's path.
func cgroupV2Path(procSelfCgroup []byte) (string, bool) {
	for _, line := range strings.Split(string(procSelfCgroup), "\n") {
		if p, ok := strings.CutPrefix(line, "0::"); ok {
			return p, true
		}
	}
	return "", false
}

// defaultLimit is the limit SetMemoryLimit's doc gives, with the cgroup
// files under cgroupRoot, or cgroupMax's when it is "" (copied from
// jp2k-go/internal/decoder/budget.go:213-275, docs/design.md section 6).
func defaultLimit(cgroupRoot string) int64 {
	if l := debug.SetMemoryLimit(-1); l < math.MaxInt64 {
		return max(1, l/2)
	}
	if runtime.GOOS == "linux" {
		l := cgroupMax()
		if cgroupRoot != "" {
			l = cgroupLimit(cgroupRoot)
		}
		if l > 0 {
			return max(1, l/2)
		}
	}
	return math.MaxInt64
}

// cgroupLimit reads the cgroup v2 or v1 memory limit under root, or 0.
func cgroupLimit(root string) int64 {
	for _, f := range []string{"memory.max", "memory/memory.limit_in_bytes"} {
		b, err := os.ReadFile(filepath.Join(root, f))
		if err != nil {
			continue
		}
		// v2 says "max" for none, v1 a number near 2^63.
		n, err := strconv.ParseInt(strings.TrimSpace(string(b)), 10, 64)
		if err == nil && n > 0 && n < 1<<50 {
			return n
		}
	}
	return 0
}
