//! Tests for `core/no-secrets-in-source`. Every token is built at run time from
//! pieces so that no secret-looking literal exists in the source.

use firescale_core::rule::Options;
use firescale_core::testing::{Fixture, Track};
use serde_json::json;

use super::{find_secrets, NoSecretsInSource};

fn options(value: serde_json::Value) -> Options {
    value.as_object().cloned().unwrap_or_default()
}

fn mix(unit: &str, times: usize) -> String {
    unit.repeat(times)
}

fn aws() -> String {
    ["AK", "IA"].concat() + &mix("Q7ZK", 4)
}
fn github() -> String {
    ["gh", "p_"].concat() + &mix("a1B2", 9)
}
fn github_pat() -> String {
    ["github", "_pat_"].concat() + &mix("a1B_2c", 10)
}
fn slack() -> String {
    ["xo", "xb-"].concat() + &mix("12ab-", 5)
}
fn stripe() -> String {
    ["sk", "_live_"].concat() + &mix("a1B2", 6)
}
fn google() -> String {
    ["AI", "za"].concat() + &mix("a1-_B", 7)
}
fn npm() -> String {
    ["np", "m_"].concat() + &mix("a1B2", 9)
}
fn private_key() -> String {
    ["-----BEG", "IN RSA PRIV", "ATE KEY-----"].concat()
}

fn repo(files: &[(&str, &str)]) -> Fixture {
    Fixture::new(files, Track::All).unwrap()
}

fn run(f: &Fixture, opts: &Options) -> Vec<(String, u32, String)> {
    f.check(&NoSecretsInSource, opts)
        .unwrap()
        .into_iter()
        .map(|d| (d.file, d.range.map_or(0, |r| r.start_line), d.message))
        .collect()
}

mod passing_repo {
    use super::*;

    #[test]
    fn reports_nothing_for_a_clean_repo() {
        let f = repo(&[("src/a.ts", "export const a = 1;\n"), ("README.md", "hi\n")]);
        assert!(run(&f, &Options::new()).is_empty());
    }

    #[test]
    fn ignores_placeholders_and_the_aws_sample_key() {
        let sample = ["AKIAIOSFODNN7", "EXAMPLE"].concat();
        let repeated = ["gh", "p_"].concat() + &"a".repeat(36);
        let word = ["sk", "_live_"].concat() + "placeholder_placeholder";
        let text = format!("{sample}\n{repeated}\n{word}\n");
        let f = repo(&[("config.txt", &text)]);
        assert!(run(&f, &Options::new()).is_empty());
    }

    #[test]
    fn skips_lockfiles_binary_files_and_vendored_folders() {
        let token = format!("token={}\n", github());
        let mut binary = token.clone().into_bytes();
        binary.push(0);
        let f = repo(&[("pnpm-lock.yaml", &token), ("node_modules/x/a.js", &token)]);
        std::fs::write(f.root().join("blob.dat"), binary).unwrap();
        f.track(&["blob.dat"]).unwrap();
        assert!(run(&f, &Options::new()).is_empty());
    }

    #[test]
    fn does_not_scan_untracked_files() {
        let token = format!("{}\n", github());
        let files = [("a.txt", token.as_str()), ("b.txt", "ok\n")];
        let f = Fixture::new(&files, Track::Only(&["b.txt"])).unwrap();
        assert!(run(&f, &Options::new()).is_empty());
    }

    #[test]
    fn reports_nothing_outside_a_git_repo() {
        let token = format!("{}\n", github());
        let f = Fixture::new(&[("a.txt", token.as_str())], Track::NoGit).unwrap();
        assert!(run(&f, &Options::new()).is_empty());
    }

    #[test]
    fn requires_a_word_boundary_before_the_prefix() {
        let text = format!("x{}\n", github());
        assert!(find_secrets(&text).is_empty());
    }

    #[test]
    fn rejects_wrong_lengths() {
        assert!(find_secrets(&format!("{}Z\n", aws())).is_empty());
        assert!(find_secrets(&format!("{}\n", &github()[..30])).is_empty());
        assert!(find_secrets(&format!("{}a\n", npm())).is_empty());
    }
}

mod failing_repo {
    use super::*;

    #[test]
    fn reports_each_kind_with_its_line_and_never_the_token() {
        let cases: Vec<(String, &str)> = vec![
            (aws(), "AWS access key ID"),
            (github(), "GitHub token"),
            (github_pat(), "GitHub token"),
            (slack(), "Slack token"),
            (stripe(), "Stripe live key"),
            (google(), "Google API key"),
            (npm(), "npm token"),
        ];
        for (token, kind) in cases {
            let text = format!("line one\nvalue = \"{token}\"\n");
            let f = repo(&[("src/conf.txt", &text)]);
            let found = run(&f, &Options::new());
            assert_eq!(found.len(), 1, "{kind}");
            let (file, line, message) = &found[0];
            assert_eq!(file, "src/conf.txt");
            assert_eq!(*line, 2, "{kind}");
            assert!(message.contains(kind), "{message}");
            assert!(message.contains("Rotate"), "{message}");
            assert!(!message.contains(&token), "{message}");
            let tail = &token[token.len().saturating_sub(8)..];
            assert!(!message.contains(tail), "{message}");
        }
    }

    #[test]
    fn reports_every_match_on_separate_lines() {
        let text = format!("{}\nok\n{}\n", aws(), npm());
        let f = repo(&[("a.env.txt", &text)]);
        let lines: Vec<u32> = run(&f, &Options::new()).iter().map(|r| r.1).collect();
        assert_eq!(lines, vec![1, 3]);
    }

    #[test]
    fn handles_crlf_line_endings() {
        let text = format!("a\r\nb\r\n{}\r\n", github());
        assert_eq!(find_secrets(&text), vec![(3, "a GitHub token")]);
    }
}

fn fake_body() -> String {
    mix("aB3+/xY9", 8)
}

fn pem(body: &str) -> String {
    let end = private_key().replace("BEGIN", "END");
    format!("{}\n{body}\n{end}\n", private_key())
}

mod private_key_evidence {
    use super::*;

    #[test]
    fn a_header_with_a_body_line_is_reported() {
        let text = format!("x\n  {}\n{}\n", private_key(), fake_body());
        assert_eq!(find_secrets(&text), vec![(2, "a private key")]);
        assert_eq!(find_secrets(&pem(&fake_body())).len(), 1);
    }

    #[test]
    fn a_header_with_a_matching_end_line_is_reported() {
        let end = private_key().replace("BEGIN", "END");
        let text = format!("{}\nshort\nshort\nshort\nshort\n{end}\n", private_key());
        assert_eq!(find_secrets(&text), vec![(1, "a private key")]);
    }

    #[test]
    fn a_header_alone_or_inside_a_string_is_not_reported() {
        let alone = format!("{}\nshort\n", private_key());
        assert!(find_secrets(&alone).is_empty());
        let code = format!("const HEADER = \"{}\";\n{}\n", private_key(), fake_body());
        assert!(find_secrets(&code).is_empty());
        let one_line = format!("v = \"{}\"\n", private_key());
        assert!(find_secrets(&one_line).is_empty());
    }
}

mod boundaries_and_placeholders {
    use super::*;

    #[test]
    fn an_exact_length_token_followed_by_more_characters_is_not_reported() {
        assert!(find_secrets(&format!("{}a\n", aws())).is_empty());
        assert!(find_secrets(&format!("{}_x\n", aws())).is_empty());
        assert_eq!(find_secrets(&format!("{}\"\n", aws())).len(), 1);
    }

    #[test]
    fn a_placeholder_does_not_hide_a_real_token_later_in_the_line() {
        let placeholder = ["gh", "p_"].concat() + &"a".repeat(36);
        let text = format!("{placeholder} {}\n", github());
        assert_eq!(find_secrets(&text), vec![(1, "a GitHub token")]);
    }
}

mod allow_marker {
    use super::*;

    #[test]
    fn a_line_with_the_marker_is_ignored() {
        let text = format!("{} # firescale:allow-secret\n{}\n", github(), github());
        let f = repo(&[("a.txt", &text)]);
        let lines: Vec<u32> = run(&f, &Options::new()).iter().map(|r| r.1).collect();
        assert_eq!(lines, vec![2]);
    }
}

mod allow_files_option {
    use super::*;

    #[test]
    fn skips_listed_files_only() {
        let text = format!("{}\n", github());
        let f = repo(&[("docs/a.txt", &text), ("docs/b.txt", &text)]);
        let found = run(&f, &options(json!({ "allowFiles": ["docs/a.txt"] })));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "docs/b.txt");
    }

    #[test]
    fn rejects_invalid_values_with_a_helpful_error() {
        let f = repo(&[("a.txt", "ok\n")]);
        for bad in [
            json!("a.txt"),
            json!([1]),
            json!([""]),
            json!(["../a.txt"]),
            json!(["/a.txt"]),
            json!(["docs\\a.txt"]),
        ] {
            let error = f
                .check(&NoSecretsInSource, &options(json!({ "allowFiles": bad })))
                .unwrap_err();
            assert!(error.message().contains("allowFiles"));
            assert!(error.message().contains("Fix it"));
        }
    }
}

mod self_scan {
    use super::*;

    #[test]
    fn this_rules_own_sources_have_no_matches() {
        for text in [
            include_str!("mod.rs"),
            include_str!("tests.rs"),
            include_str!("SKILL.md"),
        ] {
            assert!(find_secrets(text).is_empty());
        }
    }
}
