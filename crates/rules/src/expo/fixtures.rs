//! Fixture repos the expo rules' tests share.

use firescale_core::testing::{Fixture, Track};

use super::{app_config, entry, eslint, new_package_json, tsconfig};

/// An Expo app as `fscale init --packs core,expo` leaves it.
pub(crate) fn set_up() -> Vec<(&'static str, String)> {
    vec![
        ("package.json", new_package_json()),
        ("app.json", app_config::new_app_json("expo-app")),
        ("App.tsx", entry::APP.to_owned()),
        ("index.ts", entry::INDEX.to_owned()),
        ("tsconfig.json", tsconfig::TSCONFIG.to_owned()),
        ("eslint.config.js", eslint::CONFIG.to_owned()),
        (
            ".gitignore",
            "node_modules/\n.env\n\n# Expo\n.expo/\ndist/\n.env*.local\n*.jks\n*.p8\n*.p12\n*.key\n*.mobileprovision\n"
                .to_owned(),
        ),
    ]
}

pub(crate) fn fixture(files: &[(&str, String)]) -> Fixture {
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, c)| (*p, c.as_str())).collect();
    Fixture::new(&refs, Track::All).unwrap()
}

/// The set-up app without the files for which `drop` is true.
pub(crate) fn app_without(drop: impl Fn(&str) -> bool) -> Fixture {
    with(drop, &[])
}

/// The set-up app without the files for which `drop` is true, and with
/// `extra` files added (or replaced).
pub(crate) fn with(drop: impl Fn(&str) -> bool, extra: &[(&'static str, &str)]) -> Fixture {
    let mut files: Vec<(&str, String)> = set_up()
        .into_iter()
        .filter(|(path, _)| !drop(path) && !extra.iter().any(|(p, _)| p == path))
        .collect();
    files.extend(extra.iter().map(|(p, c)| (*p, (*c).to_owned())));
    fixture(&files)
}
