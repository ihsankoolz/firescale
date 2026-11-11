# `fscale check --json`

`fscale check --json` prints one JSON document to stdout. It is the stable interface for CI and coding agents. The human output of `fscale check` may change at any time; this format only changes with a new `schemaVersion`.

- JSON Schema: [`packages/cli/schemas/check-output.schema.json`](../packages/cli/schemas/check-output.schema.json). It ships in the npm package as `firescale/schemas/check-output.schema.json`.
- TypeScript type: `CheckReport`, exported from `firescale`. The document itself is produced by the Rust binary (`crates/core/src/report.rs`).
- A test validates real output against the schema, and a snapshot test pins the exact output (`packages/cli/src/check.test.ts`).

## Example

```json
{
  "schemaVersion": 1,
  "version": "0.1.0",
  "ok": false,
  "summary": {
    "rules": 1,
    "violations": 1,
    "enforcedViolations": 1
  },
  "rules": [
    {
      "id": "core/no-committed-env",
      "status": "enforced",
      "severity": "blocker",
      "category": "security",
      "description": "No .env files are tracked by git, except example files such as .env.example.",
      "violations": 1
    }
  ],
  "diagnostics": [
    {
      "ruleId": "core/no-committed-env",
      "message": "\".env\" is tracked by git. Env files usually hold secrets and must not be committed.",
      "file": ".env",
      "fixable": "deterministic",
      "status": "enforced",
      "severity": "blocker"
    }
  ]
}
```

## Fields

| Field                        | Type    | Meaning                                                                                 |
| ---------------------------- | ------- | --------------------------------------------------------------------------------------- |
| `schemaVersion`              | `1`     | Version of this format. Bumped only on a breaking change.                               |
| `version`                    | string  | The firescale version that produced the report.                                         |
| `ok`                         | boolean | `false` when at least one enforced rule has a violation.                                |
| `summary.rules`              | integer | Number of rules that ran.                                                               |
| `summary.violations`         | integer | Total number of diagnostics.                                                            |
| `summary.enforcedViolations` | integer | Diagnostics from rules whose status is `enforced`.                                      |
| `rules[]`                    | array   | Every rule that ran, sorted by id. Rules that are `off` are not listed.                 |
| `rules[].status`             | string  | `evaluated` (report only) or `enforced` (fails the run).                                |
| `rules[].severity`           | string  | `blocker`, `high`, `medium` or `low`.                                                   |
| `rules[].category`           | string  | `security`, `reliability`, `consistency` or `maintainability`.                          |
| `rules[].violations`         | integer | Number of diagnostics the rule produced.                                                |
| `diagnostics[]`              | array   | Every violation, grouped by rule in the order of `rules`, then sorted by file and line. |
| `diagnostics[].ruleId`       | string  | `<pack>/<rule>`.                                                                        |
| `diagnostics[].message`      | string  | What is wrong.                                                                          |
| `diagnostics[].file`         | string  | Path relative to the repo root. Always forward slashes, on every OS.                    |
| `diagnostics[].range`        | object  | Optional. `{ startLine, endLine }`, 1-based and inclusive.                              |
| `diagnostics[].project`      | string  | Optional. The project the file belongs to, when known.                                  |
| `diagnostics[].fixable`      | string  | `deterministic` (the rule's fix), `agent` (follow the SKILL.md) or `manual`.            |
| `diagnostics[].status`       | string  | Status of the rule that produced it.                                                    |
| `diagnostics[].severity`     | string  | Severity of the rule that produced it.                                                  |

No field holds an absolute path, a timestamp or a duration, so the output for an unchanged repo is byte-for-byte identical on every run and every OS.

## Exit codes

| Code | Meaning                                                                    |
| ---- | -------------------------------------------------------------------------- |
| 0    | No enforced rule failed. `ok` is `true`.                                   |
| 1    | An enforced rule has violations. `ok` is `false`.                          |
| 2    | Bad usage, bad config, or an internal error. Nothing is printed to stdout. |

With exit code 2 the error is on stderr as plain text, and stdout is empty, so a JSON parser never sees a partial document.

Notes for the user (for example that `--rules` ran a rule that is `off`) and anything a custom rule logs also go to stderr. Stdout only ever holds the JSON document.
