//! The Swift reader of the tamper guard (M32, ADR 0052): XCTest (`func test…` in an `XCTestCase`, `XCTSkip`) and Swift Testing (`@Test`, `@Suite`, `.disabled`); assertions `XCTAssert…`, `#expect`, `#require`; suppressions `// swiftlint:disable…`.
//!
//! STUB: filled in by M32. Until then it reads no tests and no suppressions.

use super::{Suppression, TestFile};

/// Reads the tests, skips and assertions of a Swift test file.
pub(super) fn read(source: &str) -> TestFile {
    let _ = source;
    TestFile::default()
}

/// True for a path that holds Swift tests by its name or folder alone.
pub(super) fn is_test_path(path: &str) -> bool {
    let _ = path;
    false
}

/// True for Swift source that holds tests although its path does not say so.
pub(super) fn holds_tests(source: &str) -> bool {
    let _ = source;
    false
}

/// The lint suppressions in a Swift source file.
pub(super) fn suppressions(source: &str) -> Vec<Suppression> {
    let _ = source;
    Vec::new()
}
