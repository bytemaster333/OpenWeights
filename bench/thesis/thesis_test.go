package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

const sampleProcNetDev = `Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 1234567  890    0    0    0    0     0          0        1234567  890    0    0    0    0    0       0
  eth0: 987654321 12345 0    0    0    0     0          0        10000    100    0    0    0    0    0       0
`

func TestReadInboundBytesFromReader(t *testing.T) {
	cases := []struct {
		name    string
		iface   string
		want    uint64
		wantErr bool
	}{
		{"eth0 rx bytes", "eth0", 987654321, false},
		{"lo rx bytes", "lo", 1234567, false},
		{"missing iface", "ens99", 0, true},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			n, err := readInboundBytesFromReader(strings.NewReader(sampleProcNetDev), tc.iface)
			if (err != nil) != tc.wantErr {
				t.Fatalf("err=%v wantErr=%v", err, tc.wantErr)
			}
			if n != tc.want {
				t.Fatalf("got=%d want=%d", n, tc.want)
			}
		})
	}
}

func TestComputeVerdict(t *testing.T) {
	const ceiling uint64 = 1_000_000 // 1 MiB-ish (for test math use 10^6)

	cases := []struct {
		name    string
		inbound []uint64
		wantMin uint64
		wantMed uint64
		wantMax uint64
		wantOut string
	}{
		{"all sector-scoped", []uint64{300_000, 400_000, 500_000}, 300_000, 400_000, 500_000, "PASS"},
		{"median exactly at ceiling", []uint64{100, 1_000_000, 2_000_000}, 100, 1_000_000, 2_000_000, "PASS"},
		{"median above ceiling", []uint64{100, 1_000_001, 2_000_000}, 100, 1_000_001, 2_000_000, "FAIL"},
		{"all full-object", []uint64{60_000_000, 64_000_000, 62_000_000}, 60_000_000, 62_000_000, 64_000_000, "FAIL"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			trials := make([]Trial, len(tc.inbound))
			for i, b := range tc.inbound {
				trials[i] = Trial{TrialNum: i + 1, InboundBytes: b}
			}
			mn, md, mx, v := computeVerdict(trials, ceiling)
			if mn != tc.wantMin || md != tc.wantMed || mx != tc.wantMax || v != tc.wantOut {
				t.Fatalf("got (min=%d med=%d max=%d v=%s) want (%d %d %d %s)",
					mn, md, mx, v, tc.wantMin, tc.wantMed, tc.wantMax, tc.wantOut)
			}
		})
	}
}

// TestComputeVerdictPanicsOnEvenLength — explicit invariant test.
// See measure.go doc comment: even-length trial arrays silently shift median
// from lower-middle to upper-middle, corrupting the verdict. We panic instead.
func TestComputeVerdictPanicsOnEvenLength(t *testing.T) {
	cases := []struct {
		name    string
		inbound []uint64
	}{
		{"empty", nil},
		{"two trials", []uint64{100, 200}},
		{"four trials", []uint64{100, 200, 300, 400}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			defer func() {
				r := recover()
				if r == nil {
					t.Fatalf("expected panic for len=%d trials; got no panic", len(tc.inbound))
				}
				msg, ok := r.(string)
				if !ok {
					t.Fatalf("expected panic message string; got %T: %v", r, r)
				}
				if !strings.Contains(msg, "odd number of trials") {
					t.Fatalf("panic message should mention 'odd number of trials'; got: %s", msg)
				}
			}()
			trials := make([]Trial, len(tc.inbound))
			for i, b := range tc.inbound {
				trials[i] = Trial{TrialNum: i + 1, InboundBytes: b}
			}
			_, _, _, _ = computeVerdict(trials, 1_000_000)
		})
	}
}

func TestWriteRun(t *testing.T) {
	dir := t.TempDir()
	r := Report{
		Thesis:         "test",
		ObjectSize:     64 * 1024 * 1024,
		RequestedRange: 128 * 1024,
		PassCeiling:    1024 * 1024,
		Trials: []Trial{
			{TrialNum: 1, InboundBytes: 400000, Ratio: 3.05, DurationMs: 1234},
		},
		Min: 400000, Median: 400000, Max: 400000,
		Verdict:    "PASS",
		SDKVersion: "siastorage v0.0.3",
		Timestamp:  time.Date(2026, 4, 20, 12, 0, 0, 0, time.UTC),
		RunDir:     "20260420T120000Z",
	}
	path, err := writeRun(dir, r)
	if err != nil {
		t.Fatalf("writeRun: %v", err)
	}
	if filepath.Base(path) != "report.json" {
		t.Fatalf("unexpected path: %s", path)
	}
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read back: %v", err)
	}
	if !strings.Contains(string(b), `"verdict": "PASS"`) {
		t.Fatalf("serialized JSON missing verdict: %s", b)
	}
}

// Template rendering — only runs if REPORT.md.tmpl is reachable from test CWD
// (go test runs with CWD=the package dir, and REPORT.md.tmpl is in the same dir).
func TestRenderReport(t *testing.T) {
	if _, err := os.Stat("REPORT.md.tmpl"); err != nil {
		t.Skipf("REPORT.md.tmpl not readable from test CWD: %v", err)
	}
	r := Report{
		Timestamp: time.Now(),
		Verdict:   "PASS",
		Trials:    []Trial{{TrialNum: 1, InboundBytes: 400000, Ratio: 3.05, DurationMs: 1234}},
		Min:       400000, Median: 400000, Max: 400000,
		RunDir: "20260420T120000Z",
	}
	md, err := renderReportMarkdown("REPORT.md.tmpl", r)
	if err != nil {
		t.Fatalf("render: %v", err)
	}
	if !strings.Contains(md, "PASS") {
		t.Fatalf("rendered md missing verdict: %s", md)
	}
}
