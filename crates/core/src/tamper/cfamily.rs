//! The C and C++ reader of the tamper guard (M32, ADR 0052): GoogleTest (`TEST`, `TEST_F`, `TEST_P`, `DISABLED_`, `GTEST_SKIP`), Catch2 (`TEST_CASE`, `SECTION`, `[.]` hidden tags), Unity/CMocka/Check for C; assertions `EXPECT_*`, `ASSERT_*`, `REQUIRE`, `CHECK`, `TEST_ASSERT*`, `assert`; suppressions `// NOLINT…`, `#pragma … diagnostic ignored`, `// cppcheck-suppress`.
//!
//! STUB: filled in by M32. Until then it reads no tests and no suppressions.

use super::{Suppression, TestFile};

/// Reads the tests, skips and assertions of a C and C++ test file.
pub(super) fn read(source: &str, cpp: bool) -> TestFile {
    let _ = (source, cpp);
    TestFile::default()
}

/// True for a path that holds C and C++ tests by its name or folder alone.
pub(super) fn is_test_path(path: &str) -> bool {
    let _ = path;
    false
}

/// True for C and C++ source that holds tests although its path does not say so.
pub(super) fn holds_tests(source: &str, cpp: bool) -> bool {
    let _ = (source, cpp);
    false
}

/// The lint suppressions in a C and C++ source file.
pub(super) fn suppressions(source: &str, cpp: bool) -> Vec<Suppression> {
    let _ = (source, cpp);
    Vec::new()
}
