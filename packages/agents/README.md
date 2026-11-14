# @firescale/agents

Agent adapters for `fscale fix --agent` (M28, ADR 0048): Claude Code and Codex behind the runner's protocol. `node dist/cli.js <claude|codex> --version | auth status | run <request.json>`.

This package is private: its build ships inside the `firescale` npm package as `agents/`, and the `fscale` launcher points the binary at it (`FSCALE_AGENTS`).
