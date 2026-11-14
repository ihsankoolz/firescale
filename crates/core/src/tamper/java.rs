//! The Java reader of the tamper guard (M32, ADR 0052): JUnit 4 and 5 (`@Test`, `@ParameterizedTest`, `@Disabled`, `@Ignore`), TestNG; assertions `assert*`, AssertJ `assertThat`, Hamcrest; suppressions `@SuppressWarnings`, `// NOPMD`, `// CHECKSTYLE:OFF`, `//NOSONAR`.
//!
//! STUB: filled in by M32. Until then it reads no tests and no suppressions.

use super::{Suppression, TestFile};

/// Reads the tests, skips and assertions of a Java test file.
pub(super) fn read(source: &str) -> TestFile {
    let _ = source;
    TestFile::default()
}

/// True for a path that holds Java tests by its name or folder alone.
pub(super) fn is_test_path(path: &str) -> bool {
    let _ = path;
    false
}

/// True for Java source that holds tests although its path does not say so.
pub(super) fn holds_tests(source: &str) -> bool {
    let _ = source;
    false
}

/// The lint suppressions in a Java source file.
pub(super) fn suppressions(source: &str) -> Vec<Suppression> {
    let _ = source;
    Vec::new()
}
