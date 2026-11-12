//! Fixture repos the python rules' tests share.

use firescale_core::testing::{Fixture, Track};

use super::pytest_config::PYTEST_INI;
use super::requirements::{dev_requirements, REQUIREMENTS};
use super::ruff_config::RUFF_TOML;
use super::tests_present::{APP_INIT, APP_MAIN, TEST_MAIN};

/// A service of the course project's shape: its own requirements files,
/// pytest.ini and code, and no pyproject.toml.
pub(crate) fn service(folder: &str) -> Vec<(String, String)> {
    vec![
        (
            format!("{folder}/requirements.txt"),
            "fastapi==0.120.0\n".to_owned(),
        ),
        (format!("{folder}/requirements-dev.txt"), dev_requirements()),
        (format!("{folder}/pytest.ini"), PYTEST_INI.to_owned()),
        (format!("{folder}/ruff.toml"), RUFF_TOML.to_owned()),
        (format!("{folder}/app/__init__.py"), APP_INIT.to_owned()),
        (format!("{folder}/app/main.py"), APP_MAIN.to_owned()),
        (format!("{folder}/tests/test_main.py"), TEST_MAIN.to_owned()),
    ]
}

pub(crate) const GITIGNORE: &str = "__pycache__/\n.venv/\nvenv/\n.pytest_cache/\n.ruff_cache/\n";

/// Two services, each set up, minus the files `drop` names.
pub(crate) fn two_services(drop: &[&str]) -> Fixture {
    let mut files = service("backend/orders");
    files.extend(service("backend/payments"));
    files.push((".gitignore".to_owned(), GITIGNORE.to_owned()));
    files.retain(|(path, _)| !drop.contains(&path.as_str()));
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    Fixture::new(&refs, Track::All).unwrap()
}

/// A single app at the root, set up.
pub(crate) fn root_app() -> Fixture {
    let files = [
        ("requirements.txt", REQUIREMENTS.to_owned()),
        ("requirements-dev.txt", dev_requirements()),
        ("pytest.ini", PYTEST_INI.to_owned()),
        ("ruff.toml", RUFF_TOML.to_owned()),
        ("app/__init__.py", APP_INIT.to_owned()),
        ("app/main.py", APP_MAIN.to_owned()),
        ("tests/test_main.py", TEST_MAIN.to_owned()),
        (".gitignore", GITIGNORE.to_owned()),
    ];
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, c)| (*p, c.as_str())).collect();
    Fixture::new(&refs, Track::All).unwrap()
}
