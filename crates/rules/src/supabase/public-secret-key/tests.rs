//! Tests for `supabase/public-secret-key`: passing, failing and fixed repos, and the
//! fix run twice.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use super::*;
use crate::supabase::fixtures::project;
use crate::supabase::public_secret_key;

#[test]
fn a_secret_key_under_a_public_prefix_is_found_with_its_line() {
    let f = project(
        |_| false,
        &[
            (
                "src/client.ts",
                // Built from parts, so this repo's own core/env-example
                // does not read the fixture as code of this repo.
                concat!(
                    "import { createClient } from '@supabase/supabase-js';\nexport const admin = createClient(url, process",
                    ".env.NEXT_PUBLIC_SUPABASE_SERVICE_ROLE_KEY!);\n"
                ),
            ),
            (".env.example", "NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY=\nSUPABASE_SECRET_KEY=\n"),
        ],
    );
    let found = f
        .check(&public_secret_key::PublicSecretKey, &Options::new())
        .unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].file, "src/client.ts");
    assert_eq!(found[0].range.map(|r| r.start_line), Some(2));
    assert_eq!(found[0].fixable, Fixable::Manual);
}

#[test]
fn names_a_secret_under_a_public_prefix_only() {
    assert!(is_public_secret("NEXT_PUBLIC_SUPABASE_SERVICE_ROLE_KEY"));
    assert!(is_public_secret("VITE_SUPABASE_SECRET_KEY"));
    assert!(is_public_secret("EXPO_PUBLIC_SUPABASE_SECRET_KEY"));
    assert!(!is_public_secret("SUPABASE_SECRET_KEY"));
    assert!(!is_public_secret("NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY"));
    assert!(!is_public_secret("NEXT_PUBLIC_STRIPE_SECRET"));
    assert_eq!(
        first_public_secret(concat!(
            "a\nconst k = process",
            ".env.VITE_SUPABASE_SERVICE_ROLE_KEY;\n"
        )),
        Some((2, "VITE_SUPABASE_SERVICE_ROLE_KEY".to_owned()))
    );
}
