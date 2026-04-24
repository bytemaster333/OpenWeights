package main

import (
	"bufio"
	"fmt"
	"io"
	"os"
	"strconv"
	"strings"
)

// readInboundBytes returns the RX byte counter for a given interface.
// Reads /proc/net/dev directly — see RESEARCH §2.
// Header columns (Linux kernel):
//	Inter-| Receive | Transmit
//	 face |bytes packets errs drop fifo frame compressed multicast|bytes...
func readInboundBytes(iface string) (uint64, error) {
	f, err := os.Open("/proc/net/dev")
	if err != nil {
		return 0, fmt.Errorf("open /proc/net/dev: %w", err)
	}
	defer f.Close()
	return readInboundBytesFromReader(f, iface)
}

// readInboundBytesFromReader — injectable form for unit tests.
func readInboundBytesFromReader(r io.Reader, iface string) (uint64, error) {
	sc := bufio.NewScanner(r)
	for sc.Scan() {
		line := strings.TrimSpace(sc.Text())
		prefix := iface + ":"
		if !strings.HasPrefix(line, prefix) {
			continue
		}
		fields := strings.Fields(line[len(prefix):])
		if len(fields) < 1 {
			return 0, fmt.Errorf("malformed /proc/net/dev row: %q", line)
		}
		n, err := strconv.ParseUint(fields[0], 10, 64)
		if err != nil {
			return 0, fmt.Errorf("parse rx_bytes from %q: %w", line, err)
		}
		return n, nil
	}
	if err := sc.Err(); err != nil {
		return 0, fmt.Errorf("scan: %w", err)
	}
	return 0, fmt.Errorf("iface %q not found in /proc/net/dev", iface)
}
