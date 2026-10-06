//! Voice/audio lives in `crates/praxis-voice`; this keeps the kernel's
//! historical paths and hosts the Discord voice glue, which is channel code
//! (compiled only with the `discord` feature and its `serenity` dependency).
pub use praxis_voice::*;

#[cfg(feature = "discord")]
pub mod handler;
