//! Placeholder binary. The conformance crate's "product" is its test target;
//! this binary exists only so `cargo check --bin` + CI `cargo test --no-run`
//! remain cleanly addressable without a missing-target warning. Prints
//! nothing useful and exits 0.
fn main() {
    println!("openweights-conformance: run `cargo test -p openweights-conformance`");
}
