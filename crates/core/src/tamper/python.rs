//! The Python reader of the tamper guard (M24, ADR 0044): pytest and
//! unittest.

use super::TestFile;

pub(super) fn read(_source: &str) -> TestFile {
    TestFile::default()
}

pub(super) fn is_test_path(_path: &str) -> bool {
    false
}

pub(super) fn resolve(_from: &str, _spec: &str, _exists: &dyn Fn(&str) -> bool) -> Option<String> {
    None
}
