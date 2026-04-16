package main

// readInboundBytes returns the RX byte counter for an interface
// (/proc/net/dev). PLAN 06 fleshes out the real reader per RESEARCH §2.
func readInboundBytes(iface string) (uint64, error) {
	return 0, nil // stub
}
