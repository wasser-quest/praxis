//! GPU routing lives in `crates/praxis-gpu-router`: the kernel links the
//! crate but owns none of the router protocol. The provider is enabled at
//! runtime (`GPU_ROUTER_URL`) and is a safe no-op without it, so local direct
//! setups are untouched.
pub use praxis_gpu_router::*;
