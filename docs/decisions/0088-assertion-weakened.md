# 0088 — An assertion made conditional or a tautology (`assertion-weakened`)

Status: accepted (2026-11-25, Track B run). Amends ADR 0046 (kinds that need a person's approval) and ADR 0044 (assertions that cannot fail). Verification idea 6, items ii and iii only.

## Context

The guard counts assertions net per test file. An assertion moved under an `if`, wrapped in a `try` that swallows its failure, or replaced by one that always holds (`len(x) >= 0`) leaves the count unchanged, so nothing is found. Only an assertion that is all literals (`assert True`, `expect(true).toBe(true)`) or a Python tuple counts as none today (ADR 0044). The research report's idea 6 names these as items ii and iii, which need no table of matchers.

## Decision

- **A new kind, `assertion-weakened`, that needs a person's approval**, as `snapshot-changed` and `expected-changed` do (`Kind::needs_approval`): where the change runs (a laptop, the stop hook, the pull request's own job) it is reported and never blocks; where approvals are read (CI's `Tamper guard`, `--approvals`) and the base enforces the guard, it needs an approving review of the pull request's last commit. The owner asked for "report-only, needing approval as the snapshot kinds do"; that is what the snapshot kinds do. `verify.tamper.allow` takes it like any kind.
- **What an assertion is** (`tamper/weakened.rs`, asked by the Python, JavaScript/TypeScript and Rust readers of each assertion they count):
  - _Conditional or swallowed_: inside its test (up to the nearest function or closure), it is under an `if` or `if let` (not in its condition), a ternary, or the right side of `&&`/`||`/`and`/`or`; in the body of a `try` whose handler catches its failure (Python: `except AssertionError`, `Exception`, `BaseException` or bare; JavaScript and TypeScript: any `catch`); under `with suppress(AssertionError)`; or in a closure given to `catch_unwind`.
  - _A tautology_: a value compared with itself (`x == x`, `toEqual(a)` on `a`, `assert_eq!(a, a)`, `assertEqual(x, x)`), a length, count or size `>= 0` (`len(x) >= 0`, `toBeGreaterThanOrEqual(0)` on `.length`, `assert!(v.len() >= 0)`), or a condition ending in `or True` / `|| true`.
  - An assertion in a `catch` or `except` handler is not one: that is how a test checks an error.
- **When it is a finding**: per test whose id is on both sides, each assertion that is weakened now and was not on the base (by how and what it checks), as long as the test has fewer assertions that always check than on the base; at most that many. So wrapping an assertion in an `if`, or replacing it by a tautology, is found; a new assertion under an `if` beside the others is not, nor is an `if` already there on the base.
- **Named once**: an assertion that became a tautology also changed its value; it is `assertion-weakened`, and the `expected-changed` finding on the same line is left out. Both need an approval, so nothing gets easier to pass.
- Weakened assertions still count as assertions, so `assertion-removed` is unchanged.

## Consequences

- **Measured** (`bench/RESULTS.md`, "assertion-weakened (ADR 0088)"), before on `main` at `4252be3` and after: first held-out corpus 40 to 37 missed of 98 (3 to approval only); second 89 to 89 of 100. No case caught before is missed after. Labelled corpus: 0 false findings before and after (0 of 196 not-a-finding cases with a finding), 9 of 9 new caught cases.
- `bench/tamper-misses.mjs` counts the new kind as "needs approval only".
- **What the author saw.** This session measured both held-out corpora with counts only and opened no case; the titles of the second corpus's misses were seen by an earlier session (ADR 0082, 0086). The patterns are the research report's items ii and iii, written before the second corpus existed.
- A renamed test is not followed: its assertions are not compared.

## Not done

- Other languages (Java, Kotlin, Swift, Dart, C, C++, C#, Go): `weakened` is `None` there.
- A condition that is always true but not by its shape (`if True:`, `if (1)`), a loop that never runs, and `expect.assertions` are not read.
- Items i, iv, v and vi of idea 6 (matcher classes, tolerances, a dropped `await`, a new `should_panic`): Later.

## Later

- Follow renamed tests (ADR 0063) for this kind.
- Measure the false-finding rate on real history (`tamper-bench.yml`) before it could ever block on its own.

## Tests

- `bench/tamper-corpus.json`: 14 new `weak:` cases (9 caught, 5 not a finding), run by `tamper.test.ts` and counted by `bench/tamper-rates.mjs` ("Weakened assertions"). The 9 caught cases were seen failing on `main`; the 5 not-a-finding cases pass before and after.
- `tamper.test.ts`, "reports an assertion made conditional as needing approval, which does not block where the change runs (ADR 0088)".
- `tamper::weakened::tests`: the tautologies by their text, and the guards in Python, TypeScript and Rust.
