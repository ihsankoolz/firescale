# SARIF output

`fscale check --sarif` prints the report as [SARIF 2.1.0](https://docs.oasis-open.org/sarif/sarif/v2.1.0/sarif-v2.1.0.html) on stdout. Exit codes are the same as for the other formats. `--sarif` cannot be combined with `--json`.

The document has one run. `tool.driver` is `firescale`, with one rule descriptor for each rule that ran. There is one result per diagnostic, with `ruleId`, `ruleIndex`, `level`, `message.text`, a location and `properties` (`fixable`, `status`).

## Levels

| Rule status | Severity      | SARIF level |
| ----------- | ------------- | ----------- |
| `evaluated` | any           | `note`      |
| `enforced`  | blocker, high | `error`     |
| `enforced`  | medium        | `warning`   |
| `enforced`  | low           | `note`      |

## Locations

`artifactLocation.uri` is the path relative to the repo root, with forward slashes, percent-encoded as a URI reference (spaces, non-ASCII characters, `[`, `]`, `%`, `#` and `?` are escaped), and `uriBaseId` is `%SRCROOT%`. `region` (`startLine`, `endLine`) is present only when the diagnostic has a line range.

## GitHub code scanning

Write the output to a file (`fscale check --sarif > fscale.sarif`) and upload it with GitHub's SARIF upload for code scanning; see GitHub's documentation for the current action and API.
