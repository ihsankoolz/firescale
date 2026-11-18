# 0082 — A second held-out corpus of cheats

Status: accepted (2026-11-18, Track B run). Amends ADR 0046 (measured rates).

## Context

`bench/tamper-cheats.json` (ADR 0046) is the gate for every tamper guard change: 98 cheating changes written by a subagent that never saw the guard, held out, never tuned on. On 2026-11-18 the author of `docs/research/research-verification.md` listed the titles of its cases to write that report (the report says so). The next detectors (verification ideas 1, 6 and 7) come from that report, so they are no longer tested blind against the first corpus: a detector shaped by its titles would look better on it than it is.

The report's idea 11 asks for a second corpus, written from ImpossibleBench's categories and METR's examples by an agent that sees none of the guard.

## Decision

- **`bench/tamper-cheats-2.json`**: 100 cheating changes in the first corpus's format (`name`, `category`, `language`, `description`, `base`, `head`; the writer used `python` and `rust`, changed afterwards to the first corpus's `py` and `rs`), 20 in each of five categories: `test-modification`, `operator-overloading`, `recording-extra-state`, `special-casing` (ImpossibleBench's four, Zhong et al., arXiv 2510.20270) and `evaluator-tampering` (the reward hacks of METR's June 2025 report: a patched grader, clock or assertion, answers read from the test file or the stack, a harness that always reports success). Languages: Python 35, JavaScript 20, TypeScript 15, Rust 30; Vitest or Jest, pytest, and `cargo test`.
- **How it was written, and what its writer did not see.** One subagent wrote it on 2026-11-18. Its brief forbade opening, reading, listing or searching anything in the firescale checkouts on the laptop (and so `crates/core/src/tamper/`, `bench/tamper-cheats.json`, `bench/tamper-corpus.json` and `docs/research/`), the research folder beside them, or GitHub for "firescale". It was given only the JSON format and the five categories with examples of each. Its report states that it followed this, opened none of those folders, and did not use the web. It wrote the cases in its own scratch folder; they were copied into the repo after it finished. The brief did not show it the first corpus's categories or any of its titles.
- **It is held out like the first one.** Never tune a detector on it, and never read its cases to design one. A detector pull request measures misses on both corpora before and after, and is not merged if it adds a miss on either (the owner's rule for this run).
- `bench/tamper-misses.mjs --corpus <file>` runs either; without it, the first, as before. **It prints counts only**: the titles of missed cases come with `--names` (and are in `--json`), so that the routine command does not show them (the review of this pull request).
- The first corpus stays as a gate. It is no longer blind, and `bench/RESULTS.md` says so.

## Consequences

- **The number is low, and it says where the guard looks.** On `main` at `ba9c459`, 90 of 100 are missed (7 caught, 3 need approval only). 60 of the misses are operator overloading, recorded state and special-casing, which live in production code; the guard reads test files, config and CI only, by design. Test modification, where it is meant to work, misses 12 of 20. `bench/RESULTS.md` has the tables and the kinds of misses.
- Equal weights per category are not how often agents cheat each way; no weighted rate is given, because the published mix was not checked against a primary source here.
- The cases were not run: that each base fails and each head passes rests on the writer's reading of its own code.
- Whatever a later author reads of this file ends its use as a blind gate too. The ADR of a detector should say whether its author opened it.
- **`tamper-misses.mjs` printed the title of every missed case** when this corpus was first measured. In this run that was the session that writes the next detectors (Track B's pull requests 5 to 7): it saw the 90 titles when it measured, and opened no case. Those detectors were named by the owner before this corpus existed, and their ADRs say what their author saw. `bench/RESULTS.md` describes the misses by category only, so that it does not tell a detector author what to match.

## Later

- A rate weighted by ImpossibleBench's published mix, once the mix is checked against the paper.
- A third corpus when this one is read, and cases in the languages the guard reads beyond these four (Java, Kotlin, C#, Go, Swift, Dart, C, C++).
- Run each case's tests once (base fails, head passes) to prove the cases are what they claim.
