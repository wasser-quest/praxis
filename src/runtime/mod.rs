//! Shared runtime services available without feature plugins or frontends.
pub mod events;
pub mod features;
pub mod retention;
pub mod services;
pub mod templates;
pub mod vm;

#[cfg(test)]
mod vm_tests;

#[cfg(test)]
mod vm_process_tests;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod feature_tests;
