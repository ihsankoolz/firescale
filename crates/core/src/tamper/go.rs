//! The Go reader of the tamper guard (M32, ADR 0052): `func TestXxx(t *testing.T)`, subtests `t.Run`, `t.Skip…`; assertions `t.Error…`/`t.Fatal…`, testify `assert.*`/`require.*`; suppressions `//nolint`, `//lint:ignore`.
//!
//! STUB: filled in by M32. Until then it reads no tests and no suppressions.

use super::{Suppression, TestFile};

/// Reads the tests, skips and assertions of a Go test file.
pub(super) fn read(source: &str) -> TestFile {
    let _ = source;
    TestFile::default()
}

/// True for a path that holds Go tests by its name or folder alone.
pub(super) fn is_test_path(path: &str) -> bool {
    let _ = path;
    false
}

/// True for Go source that holds tests although its path does not say so.
pub(super) fn holds_tests(source: &str) -> bool {
    let _ = source;
    false
}

/// The lint suppressions in a Go source file.
pub(super) fn suppressions(source: &str) -> Vec<Suppression> {
    let _ = source;
    Vec::new()
}
