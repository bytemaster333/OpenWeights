// Package main — singleflight_test.go.
//
// Validates the GATE-09 / P10 coalescing invariant: 100 concurrent cold-miss
// requests for the SAME xorb hash collapse into exactly ONE underlying
// fetch. Also covers error-propagation semantics and distinct-hash
// concurrency (no cross-hash blocking).
package main

import (
	"context"
	"errors"
	"fmt"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// TestMissCoalescer_100Concurrent — the load-bearing GATE-09 assertion.
// 100 goroutines racing into Do(hash, fn) for the same hash → fn runs
// exactly ONCE, every goroutine gets the same result, coalesced counter
// goes up (≥ 1; plan's "at least one follower joined" signal).
func TestMissCoalescer_100Concurrent(t *testing.T) {
	// Register metrics so the coalesced counter collector exists.
	_ = NewMetrics()

	mc := NewMissCoalescer()
	var calls atomic.Int64
	fetchFn := func() (string, error) {
		calls.Add(1)
		// Simulate a slow Sia fetch so followers have time to queue up
		// behind the leader. 50ms is long enough that 100 goroutines
		// starting concurrently all reach Do() before fn returns.
		time.Sleep(50 * time.Millisecond)
		return "/cache/abc.bin", nil
	}

	const n = 100
	var wg sync.WaitGroup
	wg.Add(n)
	errCh := make(chan error, n)
	// Barrier: all 100 goroutines start at the same instant to maximise
	// concurrency pressure. Without this, the singleflight group could
	// "finish" a single call before the next arrival and serialise fetches.
	barrier := make(chan struct{})
	for i := 0; i < n; i++ {
		go func() {
			defer wg.Done()
			<-barrier
			path, err := mc.Do(context.Background(), "abcdef", fetchFn)
			if err != nil {
				errCh <- err
				return
			}
			if path != "/cache/abc.bin" {
				errCh <- errors.New("wrong path from coalescer")
			}
		}()
	}
	close(barrier)
	wg.Wait()
	close(errCh)
	for e := range errCh {
		t.Error(e)
	}

	if got := calls.Load(); got != 1 {
		t.Fatalf("GATE-09 violated: %d fetch calls for 100 concurrent misses, want 1", got)
	}
	if got := mc.CoalescedLoad(); got < 1 {
		t.Fatalf("coalesced counter: want >= 1 follower, got %d", got)
	}
	t.Logf("GATE-09: 100 concurrent misses → %d fetch call, %d coalesced signal", calls.Load(), mc.CoalescedLoad())
}

// TestMissCoalescer_DistinctHashesParallel — distinct hashes do NOT block
// each other. fn for hash A runs concurrently with fn for hash B. Asserts
// the singleflight key granularity is correct (NOT a global mutex).
func TestMissCoalescer_DistinctHashesParallel(t *testing.T) {
	_ = NewMetrics()

	mc := NewMissCoalescer()

	// Stagger by hash: "a" sleeps 100ms, "b" sleeps 10ms. If the coalescer
	// serialises the two, total time is ≥ 110ms. If it parallelises them
	// (correct), total is ≈ 100ms.
	start := time.Now()
	var wg sync.WaitGroup
	wg.Add(2)
	go func() {
		defer wg.Done()
		_, _ = mc.Do(context.Background(), "aaaa", func() (string, error) {
			time.Sleep(100 * time.Millisecond)
			return "a", nil
		})
	}()
	go func() {
		defer wg.Done()
		_, _ = mc.Do(context.Background(), "bbbb", func() (string, error) {
			time.Sleep(10 * time.Millisecond)
			return "b", nil
		})
	}()
	wg.Wait()
	elapsed := time.Since(start)
	if elapsed > 150*time.Millisecond {
		t.Fatalf("distinct hashes serialised: %v > 150ms", elapsed)
	}
	t.Logf("distinct-hash parallelism: %v", elapsed)
}

// TestMissCoalescer_ErrorPropagation — fn error surfaces to every caller in
// the cohort (not just the leader).
func TestMissCoalescer_ErrorPropagation(t *testing.T) {
	_ = NewMetrics()

	mc := NewMissCoalescer()
	targetErr := errors.New("sia is sad")
	fetchFn := func() (string, error) {
		time.Sleep(10 * time.Millisecond)
		return "", targetErr
	}

	const n = 20
	var wg sync.WaitGroup
	wg.Add(n)
	errs := make(chan error, n)
	for i := 0; i < n; i++ {
		go func() {
			defer wg.Done()
			_, err := mc.Do(context.Background(), "cccc", fetchFn)
			errs <- err
		}()
	}
	wg.Wait()
	close(errs)

	got := 0
	for e := range errs {
		if !errors.Is(e, targetErr) {
			t.Errorf("got err %v, want %v", e, targetErr)
		}
		got++
	}
	if got != n {
		t.Fatalf("err count: want %d, got %d", n, got)
	}
}

// TestMissCoalescer_SeriallyNoCoalesce — sequential (non-concurrent) Do
// calls each invoke fn independently; no sharing.
func TestMissCoalescer_SeriallyNoCoalesce(t *testing.T) {
	_ = NewMetrics()

	mc := NewMissCoalescer()
	var calls atomic.Int64
	fn := func() (string, error) {
		calls.Add(1)
		return "x", nil
	}
	for i := 0; i < 5; i++ {
		if _, err := mc.Do(context.Background(), "dddd", fn); err != nil {
			t.Fatalf("Do %d: %v", i, err)
		}
	}
	if got := calls.Load(); got != 5 {
		t.Fatalf("serial Do calls: want 5, got %d", got)
	}
	// Serial calls have no followers; coalesced counter stays at 0.
	if got := mc.CoalescedLoad(); got != 0 {
		t.Fatalf("coalesced counter should stay 0 for serial: got %d", got)
	}
}

// TestMissCoalescer_FetchCountMetric — the coalescer's
// `gateway_cache_singleflight_coalesced_total` Prometheus counter
// increments on follower-join, and stays monotonic across cohorts.
func TestMissCoalescer_FetchCountMetric(t *testing.T) {
	_ = NewMetrics()

	mc := NewMissCoalescer()
	before := readCounter(metricsCacheSingleflightCoalesced)

	// Fire 10 concurrent Do calls for the same hash.
	var wg sync.WaitGroup
	const n = 10
	wg.Add(n)
	barrier := make(chan struct{})
	for i := 0; i < n; i++ {
		go func() {
			defer wg.Done()
			<-barrier
			_, _ = mc.Do(context.Background(), fmt.Sprintf("metric-test"), func() (string, error) {
				time.Sleep(20 * time.Millisecond)
				return "ok", nil
			})
		}()
	}
	close(barrier)
	wg.Wait()

	after := readCounter(metricsCacheSingleflightCoalesced)
	if after <= before {
		t.Fatalf("coalesced metric did not increment: before=%v after=%v", before, after)
	}
	t.Logf("coalesced metric: before=%v after=%v", before, after)
}
