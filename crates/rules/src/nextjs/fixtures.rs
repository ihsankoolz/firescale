//! Fixture repos the nextjs rules' tests share.

use firescale_core::testing::{Fixture, Track};

use super::{app_router, eslint, new_package_json, next_config, tsconfig};

/// A Next.js app as create-next-app leaves it.
pub(crate) fn set_up() -> Vec<(&'static str, String)> {
    vec![
        ("package.json", new_package_json()),
        ("app/layout.tsx", app_router::LAYOUT.to_owned()),
        ("app/page.tsx", app_router::PAGE.to_owned()),
        ("next.config.ts", next_config::CONFIG.to_owned()),
        ("tsconfig.json", tsconfig::TSCONFIG.to_owned()),
        ("eslint.config.mjs", eslint::CONFIG.to_owned()),
        (
            ".gitignore",
            "node_modules/\n.next/\nnext-env.d.ts\n".to_owned(),
        ),
    ]
}

/// The set-up app without the files for which `drop` is true.
pub(crate) fn app_without(drop: impl Fn(&str) -> bool) -> Fixture {
    let files: Vec<(&str, String)> = set_up()
        .into_iter()
        .filter(|(path, _)| !drop(path))
        .collect();
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, c)| (*p, c.as_str())).collect();
    Fixture::new(&refs, Track::All).unwrap()
}
