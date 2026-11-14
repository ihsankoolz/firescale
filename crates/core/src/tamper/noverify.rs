//! Stub: filled in M25.

use super::config::Measure;

pub(crate) fn reads(_path: &str) -> bool {
    false
}

pub(crate) fn measures(_path: &str, _text: &str) -> Result<Vec<Measure>, usize> {
    Ok(Vec::new())
}
