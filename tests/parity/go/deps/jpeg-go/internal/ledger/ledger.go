// Package ledger is a decode call's allocation budget (Limits.MaxBytes).
// Every make() whose size depends on the input charges it first.
package ledger

import (
	"errors"
	"fmt"
	"sync/atomic"
)

// ErrLimit is returned when a call's own Limits are exceeded. Permanent:
// retrying the same call fails the same way.
var ErrLimit = errors.New("jpeg: per-call limit exceeded")

// LimitError says which per-call limit was exceeded. It wraps ErrLimit.
type LimitError struct {
	Name     string // "MaxPixels" or "MaxBytes"
	Got, Max int64
}

func (e *LimitError) Error() string {
	return fmt.Sprintf("%v: %s %d over %d", ErrLimit, e.Name, e.Got, e.Max)
}

func (e *LimitError) Unwrap() error { return ErrLimit }

// Ledger counts the bytes a call has charged against max. A nil *Ledger
// is unbounded, so call sites need no check.
type Ledger struct {
	max  int64
	used atomic.Int64
}

// New returns a ledger of max bytes, or nil (unbounded) for max <= 0.
func New(max int64) *Ledger {
	if max <= 0 {
		return nil
	}
	return &Ledger{max: max}
}

// page is Charge's rounding: the runtime hands out large buffers in whole
// 8 KiB pages, so a charge is never below what the allocation costs.
const page = 8192

// Charge books n bytes about to be allocated, rounded up to a page; it
// fails, booking nothing, when they would pass the budget.
func (l *Ledger) Charge(n int64) error {
	if l == nil {
		return nil
	}
	n = (n + page - 1) &^ (page - 1)
	if u := l.used.Add(n); u > l.max || u < 0 {
		l.used.Add(-n)
		return &LimitError{"MaxBytes", u, l.max}
	}
	return nil
}

// Used is the bytes charged so far.
func (l *Ledger) Used() int64 {
	if l == nil {
		return 0
	}
	return l.used.Load()
}
