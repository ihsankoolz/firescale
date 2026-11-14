//! Fixture repos the supabase rules' tests share.

use firescale_core::testing::{Fixture, Track};

use super::{config, db_tests, gitignore, new_package_json};

/// A project set up as `fscale init --packs core,supabase` leaves it, with
/// one migration whose table has row level security.
pub(crate) fn set_up() -> Vec<(&'static str, String)> {
    vec![
        ("package.json", new_package_json()),
        ("supabase/config.toml", config::new_config("supabase-app")),
        ("supabase/.gitignore", gitignore::TEMPLATE.to_owned()),
        (db_tests::FILE, db_tests::TEST.to_owned()),
        (
            "supabase/migrations/20261114000000_notes.sql",
            "create table public.notes (id bigint primary key, body text);\nalter table public.notes enable row level security;\n".to_owned(),
        ),
        (".gitignore", "node_modules/\n.env\n".to_owned()),
    ]
}

pub(crate) fn fixture(files: &[(&str, String)]) -> Fixture {
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, c)| (*p, c.as_str())).collect();
    Fixture::new(&refs, Track::All).unwrap()
}

/// The set-up project without the files for which `drop` is true, and with
/// `extra` files added (or replaced).
pub(crate) fn project(drop: impl Fn(&str) -> bool, extra: &[(&'static str, &str)]) -> Fixture {
    let mut files: Vec<(&str, String)> = set_up()
        .into_iter()
        .filter(|(path, _)| !drop(path) && !extra.iter().any(|(p, _)| p == path))
        .collect();
    files.extend(extra.iter().map(|(p, c)| (*p, (*c).to_owned())));
    fixture(&files)
}
