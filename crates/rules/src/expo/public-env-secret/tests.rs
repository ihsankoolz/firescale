//! Tests for `expo/public-env-secret`: names that ship a secret are found,
//! public client keys are not.

use firescale_core::model::Fixable;
use firescale_core::rule::Options;

use crate::expo::fixtures::with;
use crate::expo::public_env_secret::{is_public_secret, PublicEnvSecret};

#[test]
fn names_read_as_secrets_only_with_a_secret_word() {
    assert!(is_public_secret("EXPO_PUBLIC_STRIPE_SECRET_KEY"));
    assert!(is_public_secret("EXPO_PUBLIC_PRIVATE_API_KEY"));
    assert!(!is_public_secret("EXPO_PUBLIC_MAPBOX_TOKEN"));
    assert!(!is_public_secret("EXPO_PUBLIC_SUPABASE_PUBLISHABLE_KEY"));
    assert!(!is_public_secret("STRIPE_SECRET_KEY"));
}

#[test]
fn a_secret_under_the_prefix_is_found_with_its_line() {
    let f = with(
        |_| false,
        &[
            // Built from parts, so this repo's own core/env-example does
            // not read the fixture as code of this repo.
            (
                "src/pay.ts",
                concat!(
                    "const a = 1;\nconst k = process",
                    ".env.EXPO_PUBLIC_STRIPE_SECRET_KEY;\n"
                ),
            ),
            (".env.example", "EXPO_PUBLIC_API_URL=\n"),
        ],
    );
    let found = f.check(&PublicEnvSecret, &Options::new()).unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].file, "src/pay.ts");
    assert_eq!(found[0].range.map(|r| r.start_line), Some(2));
    assert_eq!(found[0].fixable, Fixable::Manual);
}
