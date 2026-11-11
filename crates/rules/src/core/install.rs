//! How a detected package manager installs dependencies. Shared by the fixes
//! that create starter files (`core/ci-exists`, `core/readme-setup`).

/// The command a person runs to install dependencies, when the package
/// manager is one firescale knows.
pub(super) fn install_command(package_manager: &str) -> Option<&'static str> {
    match package_manager {
        "npm" => Some("npm install"),
        "pnpm" => Some("pnpm install"),
        "yarn" => Some("yarn install"),
        "bun" => Some("bun install"),
        _ => None,
    }
}

/// The install commands for a CI job, in order: they must not change the
/// lockfile. `None` when firescale cannot write them for this package manager.
pub(super) fn ci_install_commands(
    package_manager: &str,
    lockfiles: &[String],
) -> Option<Vec<&'static str>> {
    match package_manager {
        "npm" if lockfiles.iter().any(|lock| lock == "package-lock.json") => Some(vec!["npm ci"]),
        "npm" => install_command("npm").map(|command| vec![command]),
        "pnpm" => Some(vec![
            "npm install --global pnpm",
            "pnpm install --frozen-lockfile",
        ]),
        _ => None,
    }
}
