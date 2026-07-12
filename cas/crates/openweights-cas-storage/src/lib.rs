//! Sia-storage boundary for openweights-cas.
//! All code that writes to / reads from Sia goes through the [`SiaAdapter`]
//! trait. The real binary wires in [`sia::RustSdkAdapter`] (thin wrapper around
//! `sia_storage::Sdk`); the conformance crate wires in [`mock::MockSiaAdapter`]
//! via the `sia-mock` feature so `cargo test` does not need a live `indexd`
//! (CONTEXT ).
//! The trait is object-safe so `AppState::sia: Arc<dyn SiaAdapter>` works
//! without generics bleeding into every handler.

pub mod reconciler;
pub mod sia;

pub use sia::{SiaAdapter, SiaAdapterError};

#[cfg(any(test, feature = "sia-mock"))]
pub mod mock;
