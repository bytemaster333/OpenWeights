package main

import (
	"fmt"
	"sort"
	"time"
)

// Trial captures one download measurement. Exported JSON field names mirror the report schema.
type Trial struct {
	TrialNum     int     `json:"trial_num"`
	InboundBytes uint64  `json:"inbound_bytes"`
	Ratio        float64 `json:"ratio_to_requested"` // InboundBytes / RequestedRangeLength
	DurationMs   int64   `json:"duration_ms"`
	Err          string  `json:"err,omitempty"`
}

// Report is the structured output of a thesis run. Serialized to JSON and
// rendered into REPORT.md via the template.
type Report struct {
	Thesis         string    `json:"thesis"`
	ObjectSize     uint64    `json:"object_size"`
	RequestedRange uint64    `json:"requested_range"`
	PassCeiling    uint64    `json:"pass_ceiling"`
	Trials         []Trial   `json:"trials"`
	Min            uint64    `json:"min"`
	Median         uint64    `json:"median"`
	Max            uint64    `json:"max"`
	Verdict        string    `json:"verdict"` // "PASS" | "FAIL"
	SDKVersion     string    `json:"sdk_version"`
	Timestamp      time.Time `json:"timestamp"`
	RunDir         string    `json:"run_dir"` // name of the directory under bench/thesis/runs/
}

// computeVerdict sorts trial inbound bytes and returns min/median/max +
// verdict ("PASS" if median ≤ passCeiling; "FAIL" otherwise).
//
// INVARIANT: requires an ODD number of trials. A panic on even-length is
// intentional — our per-D-01 design uses 3 trials and the "lower-middle"
// median convention silently shifts to "upper-middle" at even lengths,
// which would corrupt the verdict. If a future change legitimately moves
// to an even trial count, the author MUST decide lower- vs upper-middle
// at that moment rather than inheriting whatever bytes[len/2] happens to
// return. See CONTEXT D-01 + RESEARCH §2.
func computeVerdict(trials []Trial, passCeiling uint64) (min, median, max uint64, verdict string) {
	if len(trials) == 0 || len(trials)%2 == 0 {
		panic(fmt.Sprintf("computeVerdict: requires odd number of trials (got %d)", len(trials)))
	}
	bytes := make([]uint64, len(trials))
	for i, t := range trials {
		bytes[i] = t.InboundBytes
	}
	sort.Slice(bytes, func(i, j int) bool { return bytes[i] < bytes[j] })
	min = bytes[0]
	max = bytes[len(bytes)-1]
	// Safe because len is odd: len/2 is the exact middle index (e.g. 3 -> 1, 5 -> 2).
	median = bytes[len(bytes)/2]
	if median <= passCeiling {
		verdict = "PASS"
	} else {
		verdict = "FAIL"
	}
	return
}
