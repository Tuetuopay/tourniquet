//! Integrations of tourniquet with other crates from the Rust ecosystem

#[cfg(feature = "celery")]
pub mod celery;
#[cfg(feature = "restson")]
pub mod restson;
