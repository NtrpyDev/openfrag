//! Pure, deterministic policy for openfrag v1.

#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::return_self_not_must_use
)]

mod baseline;
mod exact;
mod highlight;
mod identity;
mod rating;

pub use baseline::*;
pub use exact::*;
pub use highlight::*;
pub use identity::*;
pub use rating::*;
