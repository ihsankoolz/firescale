//! Node.js bindings for the firescale core. Keep this layer thin: it converts
//! types and forwards to `firescale_core`.

use napi_derive::napi;

/// Returns the version of the Rust core.
#[napi]
pub fn core_version() -> String {
    firescale_core::version().to_owned()
}
