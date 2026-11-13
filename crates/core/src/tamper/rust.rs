//! The Rust reader of the tamper guard (M24, ADR 0044).

use super::TestFile;

pub(super) fn read(_source: &str) -> TestFile {
    TestFile::default()
}

pub(super) fn is_test_path(_path: &str) -> bool {
    false
}

pub(super) fn holds_tests(_source: &str) -> bool {
    false
}
