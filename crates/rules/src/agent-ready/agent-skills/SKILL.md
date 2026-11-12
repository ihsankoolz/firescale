---
name: agent-skills
description: Fixes violations of the firescale rule agent-ready/agent-skills by installing the SKILL.md of every enabled firescale rule where Claude Code, Codex and Cursor look for skills. Use when fscale check reports agent-ready/agent-skills.
---

# agent-ready/agent-skills

## What this rule enforces and why

The SKILL.md of every built-in rule that is not `off` is installed in each agent's skills folder:

- Claude Code: `.claude/skills/firescale-<pack>-<rule>/SKILL.md`
- Codex: `.agents/skills/firescale-<pack>-<rule>/SKILL.md`
- Cursor: `.cursor/skills/firescale-<pack>-<rule>/SKILL.md`

The skill's `name` is the folder name, and a line after its frontmatter says firescale generated it. With the skills in place, an agent that sees a finding knows how to fix it without being told where to look.

Only files with that generated line are ever rewritten. A file of your own at the same path is left alone and reported.

## Manual fix

1. Run `fscale sync`. It writes the missing skills and rewrites generated ones that are out of date, for example after a firescale upgrade.
2. For a skill folder of your own that has the same name, rename your folder or turn the rule off.

## Agent fix instructions

### DETERMINISTIC

1. Run `fscale sync`. Do not edit the generated skill files: `fscale sync` replaces them.

### REPORT

- Report each manual finding: a file at a skill path that firescale did not write.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules agent-ready/agent-skills
```
