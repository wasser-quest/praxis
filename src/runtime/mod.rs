//! Shared runtime services available without feature plugins or frontends.
pub mod dashboard_package;
pub mod events;
pub mod features;
pub mod process_service;
pub mod retention;
pub mod services;
pub mod shell;
pub mod templates;
pub mod vm;
pub mod web;
pub mod web_proxy;

#[cfg(test)]
mod vm_tests;

#[cfg(test)]
mod vm_process_tests;

#[cfg(test)]
mod shell_process_tests;

#[cfg(test)]
mod process_service_tests;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod feature_tests;
