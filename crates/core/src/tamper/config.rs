//! The config and CI detectors of the tamper guard (M25, ADR 0045): what
//! a change does to coverage thresholds, CI check steps, git hook skips and
//! linter or type-checker rules, compared with the base like the tests are.
//!
//! Each detector turns a file it knows into [`Measure`]s: named values where
//! **higher is stricter** (a coverage floor of 80 is `8000`, a lint rule at
//! `error` is `2`, a CI check that can fail counts `1`, each `--no-verify`
//! counts `-1`). The guard gathers them from every changed file on the base
//! and in the working tree and compares them **per key over the whole
//! change**, as it compares tests: a setting moved to another file (an
//! ESLint config migrated to the flat format, a coverage floor moved from
//! `setup.cfg` to `pyproject.toml`) is not a finding; a key whose value is
//! lower in the working tree than on the base is.

use std::collections::BTreeMap;

use super::{Finding, Kind};

/// How the values of one key from several places combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Agg {
    /// The strictest place wins: a threshold, a rule's level.
    Max,
    /// Every place counts: check steps that can fail (`+1` each), hook
    /// skips and ignored rules (`-1` each).
    Sum,
}

/// One named value a config file holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Measure {
    pub kind: Kind,
    /// What is compared, the same on both sides: `jest coverage lines
    /// (global)`, `eslint no-console`, `ci pnpm test`.
    pub key: String,
    /// Higher is stricter.
    pub value: i64,
    pub agg: Agg,
    /// The value of the key when no file of the change sets it, on either
    /// side: `0` for a threshold (no floor), the default level of a rule.
    pub absent: i64,
    /// 1-based line in the file.
    pub line: usize,
    /// What is shown, as the file writes it:
    /// `coverageThreshold.global.lines: 80`, `"no-console": "off"`,
    /// `pnpm test || true`.
    pub what: String,
}

impl Measure {
    /// A measure where the strictest place wins.
    pub(crate) fn max(
        kind: Kind,
        key: impl Into<String>,
        value: i64,
        absent: i64,
        line: usize,
        what: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            key: key.into(),
            value,
            agg: Agg::Max,
            absent,
            line,
            what: what.into(),
        }
    }

    /// A measure that counts (`value` is `1` or `-1`, or `0` for a place
    /// that no longer counts, such as a check that cannot fail).
    pub(crate) fn count(
        kind: Kind,
        key: impl Into<String>,
        value: i64,
        line: usize,
        what: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            key: key.into(),
            value,
            agg: Agg::Sum,
            absent: 0,
            line,
            what: what.into(),
        }
    }
}

/// A detector: which paths it reads, and how.
pub(crate) struct Detector {
    /// True for a repo-relative path (forward slashes) it reads.
    pub reads: fn(&str) -> bool,
    /// The measures a file it reads holds, or the 1-based line where it
    /// cannot be read.
    pub measures: fn(&str, &str) -> Result<Vec<Measure>, usize>,
}

/// Every detector.
pub(crate) const DETECTORS: [Detector; 4] = [
    Detector {
        reads: super::ci::reads,
        measures: super::ci::measures,
    },
    Detector {
        reads: super::noverify::reads,
        measures: super::noverify::measures,
    },
    Detector {
        reads: super::coverage::reads,
        measures: super::coverage::measures,
    },
    Detector {
        reads: super::lintconfig::reads,
        measures: super::lintconfig::measures,
    },
];

/// True when some detector reads `path`.
pub(crate) fn is_config(path: &str) -> bool {
    DETECTORS.iter().any(|detector| (detector.reads)(path))
}

/// The measures of a file, from every detector that reads it; the line of
/// the first problem when one cannot read it.
pub(crate) fn measures_of(path: &str, text: &str) -> Result<Vec<Measure>, usize> {
    let mut all = Vec::new();
    for detector in DETECTORS.iter().filter(|detector| (detector.reads)(path)) {
        all.extend((detector.measures)(path, text)?);
    }
    Ok(all)
}

/// Config files of one tool in one folder, in the order the tool picks
/// them: it reads only the first one present that holds its settings (the
/// marker, when the file is shared with other tools), and ignores the rest.
struct Precedence {
    /// The start of the tool's measure keys.
    tool: &'static str,
    files: &'static [(&'static str, Option<&'static str>)],
}

const PRECEDENCE: [Precedence; 3] = [
    // coverage.py: `.coveragerc`, else the first of these with its settings.
    Precedence {
        tool: "coverage.py ",
        files: &[
            (".coveragerc", None),
            ("setup.cfg", Some("[coverage:")),
            ("tox.ini", Some("[coverage:")),
            ("pyproject.toml", Some("[tool.coverage")),
        ],
    },
    // Ruff: the closest of these, in this order.
    Precedence {
        tool: "ruff | ",
        files: &[
            (".ruff.toml", None),
            ("ruff.toml", None),
            ("pyproject.toml", Some("[tool.ruff")),
        ],
    },
    // ESLint 9: a flat config wins over every eslintrc form.
    Precedence {
        tool: "eslint | ",
        files: &[
            ("eslint.config.js", None),
            ("eslint.config.mjs", None),
            ("eslint.config.cjs", None),
            ("eslint.config.ts", None),
            ("eslint.config.mts", None),
            ("eslint.config.cts", None),
            (".eslintrc.js", None),
            (".eslintrc.cjs", None),
            (".eslintrc.yaml", None),
            (".eslintrc.yml", None),
            (".eslintrc.json", None),
            (".eslintrc", None),
            ("package.json", Some("\"eslintConfig\"")),
        ],
    },
];

fn split_folder(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

fn joined(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_owned()
    } else {
        format!("{folder}/{name}")
    }
}

/// The other config files of the same tools in `path`'s folder: when one of
/// them changes, all of them are read, since adding one can hide another.
pub(crate) fn siblings(path: &str) -> Vec<String> {
    let (folder, name) = split_folder(path);
    PRECEDENCE
        .iter()
        .filter(|group| group.files.iter().any(|(file, _)| *file == name))
        .flat_map(|group| group.files.iter().map(|(file, _)| joined(folder, file)))
        .filter(|sibling| sibling != path)
        .collect()
}

/// Drops the measures of files a tool does not read because another of its
/// files in the same folder comes first. `texts` holds the config files of
/// one side.
pub(crate) fn drop_shadowed(measures: &mut Vec<Located>, texts: &BTreeMap<String, String>) {
    measures.retain(|located| {
        let (folder, name) = split_folder(&located.file);
        for group in &PRECEDENCE {
            if !located.measure.key.starts_with(group.tool)
                || !group.files.iter().any(|(file, _)| *file == name)
            {
                continue;
            }
            let winner = group.files.iter().find(|(file, marker)| {
                texts
                    .get(&joined(folder, file))
                    .is_some_and(|text| marker.is_none_or(|marker| text.contains(marker)))
            });
            if let Some((file, _)) = winner {
                if *file != name {
                    return false;
                }
            }
        }
        true
    });
}

/// A measure and the file it is in.
#[derive(Debug, Clone)]
pub(crate) struct Located {
    pub file: String,
    pub measure: Measure,
}

/// Compares the measures of the base and of the working tree, key by key,
/// and names each place where the working tree is less strict.
pub(crate) fn compare(base: &[Located], head: &[Located]) -> Vec<Finding> {
    type Sides<'a> = (Vec<&'a Located>, Vec<&'a Located>);
    let mut keys: BTreeMap<(Kind, &str), Sides<'_>> = BTreeMap::new();
    for located in base {
        keys.entry((located.measure.kind, located.measure.key.as_str()))
            .or_default()
            .0
            .push(located);
    }
    for located in head {
        keys.entry((located.measure.kind, located.measure.key.as_str()))
            .or_default()
            .1
            .push(located);
    }
    let mut findings = Vec::new();
    for ((kind, _), (on_base, now)) in keys {
        let sample = on_base.first().or(now.first()).map(|l| &l.measure);
        let Some(sample) = sample else { continue };
        match sample.agg {
            Agg::Max => {
                fn best<'a>(side: &[&'a Located]) -> Option<&'a Located> {
                    side.iter().copied().max_by_key(|l| l.measure.value)
                }
                let base_best = best(&on_base);
                let head_best = best(&now);
                let base_value = base_best.map_or(sample.absent, |l| l.measure.value);
                let head_value = head_best.map_or(sample.absent, |l| l.measure.value);
                if head_value >= base_value {
                    continue;
                }
                match (head_best, base_best) {
                    (Some(head), Some(base)) => findings.push(Finding {
                        kind,
                        file: head.file.clone(),
                        line: head.measure.line,
                        on_base: false,
                        what: format!(
                            "{} (on the base: {}, {}:{})",
                            head.measure.what, base.measure.what, base.file, base.measure.line
                        ),
                    }),
                    (Some(head), None) => findings.push(Finding {
                        kind,
                        file: head.file.clone(),
                        line: head.measure.line,
                        on_base: false,
                        what: format!("{} (not set on the base)", head.measure.what),
                    }),
                    (None, Some(base)) => findings.push(Finding {
                        kind,
                        file: base.file.clone(),
                        line: base.measure.line,
                        on_base: true,
                        what: format!("{} is gone", base.measure.what),
                    }),
                    (None, None) => {}
                }
            }
            Agg::Sum => {
                let sum = |side: &[&Located]| side.iter().map(|l| l.measure.value).sum::<i64>();
                let base_sum = sum(&on_base);
                let head_sum = sum(&now);
                if head_sum >= base_sum {
                    continue;
                }
                let mut shortfall = base_sum - head_sum;
                // New places that do not count, or count against: by their
                // text, so one moved to another file is not new.
                let mut had: BTreeMap<(&str, i64), usize> = BTreeMap::new();
                for located in &on_base {
                    *had.entry((located.measure.what.as_str(), located.measure.value))
                        .or_default() += 1;
                }
                let mut kept: BTreeMap<(&str, i64), usize> = BTreeMap::new();
                for located in &now {
                    *kept
                        .entry((located.measure.what.as_str(), located.measure.value))
                        .or_default() += 1;
                }
                for located in now.iter().filter(|l| l.measure.value <= 0) {
                    if shortfall <= 0 {
                        break;
                    }
                    let entry = had
                        .entry((located.measure.what.as_str(), located.measure.value))
                        .or_default();
                    if *entry > 0 {
                        *entry -= 1;
                        continue;
                    }
                    shortfall -= located.measure.value.abs().max(1);
                    findings.push(Finding {
                        kind,
                        file: located.file.clone(),
                        line: located.measure.line,
                        on_base: false,
                        what: located.measure.what.clone(),
                    });
                }
                // Places that counted and are gone.
                for located in on_base.iter().filter(|l| l.measure.value > 0) {
                    if shortfall <= 0 {
                        break;
                    }
                    let entry = kept
                        .entry((located.measure.what.as_str(), located.measure.value))
                        .or_default();
                    if *entry > 0 {
                        *entry -= 1;
                        continue;
                    }
                    shortfall -= located.measure.value;
                    findings.push(Finding {
                        kind,
                        file: located.file.clone(),
                        line: located.measure.line,
                        on_base: true,
                        what: format!("{} is gone", located.measure.what),
                    });
                }
            }
        }
    }
    // One finding per place: a check counted for several events is named
    // once.
    let mut seen = std::collections::BTreeSet::new();
    findings
        .retain(|f| f.kind != Kind::CiWeakened || seen.insert((f.file.clone(), f.line, f.on_base)));
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(file: &str, measure: Measure) -> Located {
        Located {
            file: file.to_owned(),
            measure,
        }
    }

    #[test]
    fn a_lower_threshold_is_named_where_it_now_is() {
        let base = [at(
            "jest.config.js",
            Measure::max(Kind::CoverageLowered, "jest lines", 8000, 0, 3, "lines: 80"),
        )];
        let head = [at(
            "jest.config.js",
            Measure::max(Kind::CoverageLowered, "jest lines", 6000, 0, 3, "lines: 60"),
        )];
        let findings = compare(&base, &head);
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].what,
            "lines: 60 (on the base: lines: 80, jest.config.js:3)"
        );
        assert!(!findings[0].on_base);
        assert!(compare(&head, &base).is_empty());
    }

    #[test]
    fn a_threshold_gone_is_named_on_the_base_and_a_move_is_not_a_finding() {
        let base = [at(
            ".coveragerc",
            Measure::max(
                Kind::CoverageLowered,
                "coverage.py fail_under",
                9000,
                0,
                2,
                "fail_under = 90",
            ),
        )];
        let moved = [at(
            "pyproject.toml",
            Measure::max(
                Kind::CoverageLowered,
                "coverage.py fail_under",
                9000,
                0,
                7,
                "fail_under = 90",
            ),
        )];
        assert!(compare(&base, &moved).is_empty());
        let findings = compare(&base, &[]);
        assert_eq!(findings[0].what, "fail_under = 90 is gone");
        assert!(findings[0].on_base);
    }

    #[test]
    fn counts_name_new_bad_places_and_gone_good_ones() {
        let base = [
            at(
                "ci.yml",
                Measure::count(Kind::CiWeakened, "pnpm test", 1, 10, "pnpm test"),
            ),
            at(
                "package.json",
                Measure::count(Kind::NoVerify, "no-verify", -1, 4, "git commit --no-verify"),
            ),
        ];
        let head = [
            at(
                "ci.yml",
                Measure::count(Kind::CiWeakened, "pnpm test", 0, 10, "pnpm test || true"),
            ),
            at(
                "other.json",
                Measure::count(Kind::NoVerify, "no-verify", -1, 2, "git commit --no-verify"),
            ),
            at(
                "package.json",
                Measure::count(Kind::NoVerify, "no-verify", -1, 5, "git push --no-verify"),
            ),
        ];
        let findings = compare(&base, &head);
        let whats: Vec<&str> = findings.iter().map(|f| f.what.as_str()).collect();
        assert_eq!(whats, ["pnpm test || true", "git push --no-verify"]);
        let gone = compare(&base[..1], &[]);
        assert_eq!(gone[0].what, "pnpm test is gone");
    }
}
