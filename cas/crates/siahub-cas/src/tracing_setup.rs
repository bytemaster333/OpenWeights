use tracing_subscriber::{EnvFilter, fmt};

/// JSON stdout logger, field names matching Phase 1 Go bench/: level, ts, msg, err,
/// request_id, api_key_id, xorb_hash, bytes, duration_ms, status.
pub fn init() {
    fmt()
        .json()
        .with_current_span(false)
        .with_span_list(false)
        .with_target(false)
        .flatten_event(true)
        .with_writer(std::io::stdout)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
}
