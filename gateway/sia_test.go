// sia_test.go — SiaAdapter interface-level tests.
// We deliberately DO NOT dial Sia in unit tests: the live round-trip lives
// in bench/thesis/thesis.go and will land as a `integration`-tagged
// gateway test in when the cache miss path is wired. W2's job is to
// prove the adapter honors its contract at the interface seam:
// - NewSiaAdapter validates env inputs (AppID / AppKey / IndexerURL).
// - DownloadRange delegates ctx cancellation to its caller.
// - The `SiaDownloader` interface is satisfied by the concrete struct AND
// by the fake substitute downstream plans will inject.
// Fake-driven cancellation / success tests give us CI coverage of the
// SiaDownloader seam without a live Zen testnet dependency.
package main

import (
	"bytes"
	"context"
	"errors"
	"io"
	"sync/atomic"
	"testing"
	"time"

	"go.sia.tech/core/types"
)

// fakeSia implements SiaDownloader for use in unit tests. Configurable to
// emit a fixed payload, return a fixed error, or block until ctx cancels.
// additions: `downloadCalls` counter (atomic.Int64) so cache +
// singleflight tests can assert "exactly one SDK call for N concurrent
// misses"; `sleep` duration to simulate a slow Sia round-trip while the
// coalescer builds up followers.
type fakeSia struct {
	payload []byte
	err     error
	block   bool

	// : observability for miss-coalescing tests.
	downloadCalls atomic.Int64
	sleep         time.Duration
}

func (f *fakeSia) Download(ctx context.Context, w io.Writer, id types.Hash256, offset, length uint64) error {
	return f.DownloadRange(ctx, w, id, offset, length)
}

func (f *fakeSia) DownloadRange(ctx context.Context, w io.Writer, _ types.Hash256, offset, length uint64) error {
	f.downloadCalls.Add(1)
	if f.sleep > 0 {
		select {
		case <-time.After(f.sleep):
		case <-ctx.Done():
			return ctx.Err()
		}
	}
	if f.err != nil {
		return f.err
	}
	if f.block {
		<-ctx.Done()
		return ctx.Err()
	}
	data := f.payload
	if length > 0 {
		end := offset + length
		if end > uint64(len(data)) {
			end = uint64(len(data))
		}
		if offset > uint64(len(data)) {
			offset = uint64(len(data))
		}
		data = data[offset:end]
	}
	_, err := w.Write(data)
	return err
}

// Statically assert both types satisfy the interface. Compile-time check;
// if the interface drifts, the test file fails to build.
var _ SiaDownloader = (*SiaAdapter)(nil)
var _ SiaDownloader = (*fakeSia)(nil)

func TestNewSiaAdapter_RejectsEmptyInputs(t *testing.T) {
	t.Parallel()
	cases := []struct {
		name string
		cfg  *Config
		want string
	}{
		{"nil cfg", nil, "nil Config"},
		{"empty indexer", &Config{AppID: hex64(), AppKey: hex32()}, "SiaIndexerURL is empty"},
		{"empty app id", &Config{SiaIndexerURL: "http://x", AppKey: hex32()}, "AppID is empty"},
		{"empty app key", &Config{SiaIndexerURL: "http://x", AppID: hex64()}, "AppKey is empty"},
		{"bad app id hex", &Config{SiaIndexerURL: "http://x", AppID: "not-hex", AppKey: hex32()}, "parse SIAHUB_APP_ID"},
		{"bad app key hex", &Config{SiaIndexerURL: "http://x", AppID: hex64(), AppKey: "zzz"}, "decode SIAHUB_APP_KEY"},
	}
	for _, tc := range cases {
		tc := tc
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			_, err := NewSiaAdapter(tc.cfg, nil)
			if err == nil {
				t.Fatalf("NewSiaAdapter(%q) = nil err; want %q", tc.name, tc.want)
			}
			if !errContains(err, tc.want) {
				t.Fatalf("err = %q; want substring %q", err.Error(), tc.want)
			}
		})
	}
}

func TestFakeSia_DownloadRange_SlicesPayload(t *testing.T) {
	t.Parallel()
	payload := bytes.Repeat([]byte("abcdefgh"), 128) // 1 KiB
	f := &fakeSia{payload: payload}
	var buf bytes.Buffer

	err := f.DownloadRange(context.Background(), &buf, types.Hash256{}, 16, 32)
	if err != nil {
		t.Fatalf("DownloadRange: %v", err)
	}
	if got := buf.Bytes(); !bytes.Equal(got, payload[16:48]) {
		t.Fatalf("sliced body mismatch: got %x want %x", got, payload[16:48])
	}
}

func TestFakeSia_DownloadRange_WholeObject(t *testing.T) {
	t.Parallel()
	payload := []byte("hello sia")
	f := &fakeSia{payload: payload}
	var buf bytes.Buffer

	// length=0 → whole object (matches the SiaAdapter contract — no option set).
	if err := f.DownloadRange(context.Background(), &buf, types.Hash256{}, 0, 0); err != nil {
		t.Fatalf("DownloadRange: %v", err)
	}
	if !bytes.Equal(buf.Bytes(), payload) {
		t.Fatalf("whole-object mismatch: got %q want %q", buf.String(), payload)
	}
}

func TestFakeSia_PropagatesError(t *testing.T) {
	t.Parallel()
	want := errors.New("simulated sia outage")
	f := &fakeSia{err: want}
	err := f.DownloadRange(context.Background(), io.Discard, types.Hash256{}, 0, 128)
	if !errors.Is(err, want) {
		t.Fatalf("err = %v; want %v", err, want)
	}
}

// precondition: ctx cancel halts a blocking download within 1s.
// The fake substitutes for the SDK here; the live-wire test lands in .
func TestFakeSia_CancellationAbortsDownload(t *testing.T) {
	t.Parallel()
	f := &fakeSia{block: true}
	ctx, cancel := context.WithCancel(context.Background())

	done := make(chan error, 1)
	go func() {
		done <- f.DownloadRange(ctx, io.Discard, types.Hash256{}, 0, 1<<20)
	}()

	go func() {
		time.Sleep(50 * time.Millisecond)
		cancel()
	}()

	select {
	case err := <-done:
		if !errors.Is(err, context.Canceled) {
			t.Fatalf("err = %v; want context.Canceled", err)
		}
	case <-time.After(time.Second):
		t.Fatalf("download did not cancel within 1s")
	}
}

func TestSiaObjectIDFromHex(t *testing.T) {
	t.Parallel()
	id, err := siaObjectIDFromHex(hex64())
	if err != nil {
		t.Fatalf("siaObjectIDFromHex: %v", err)
	}
	if id == (types.Hash256{}) {
		t.Fatalf("expected non-zero Hash256")
	}
	if _, err := siaObjectIDFromHex("not hex"); err == nil {
		t.Fatalf("expected error on malformed input")
	}
}

// ---- test helpers ----

// hex32 returns 64 hex chars (32 bytes) — a valid PrivateKey literal.
// Constant pattern; not cryptographically meaningful.
func hex32() string {
	return "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef" +
		"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
}

// hex64 returns 64 hex chars (32 bytes) — a valid Hash256 literal.
func hex64() string {
	return "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
}

func errContains(err error, sub string) bool {
	if err == nil {
		return false
	}
	return bytes.Contains([]byte(err.Error()), []byte(sub))
}
