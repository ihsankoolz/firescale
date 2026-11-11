---
name: rule-id
description: One or two sentences on what this skill fixes and when to use it. Mention the firescale rule id.
---

# pack/rule-id

## What this rule enforces and why

State what must be true in the repo, in one or two sentences. Then say why it matters: what goes wrong when the rule is broken.

## Manual fix

Numbered steps a person can follow by hand.

1. First step.
2. Second step.

## Agent fix instructions

Fix only the diagnostics reported for this rule. Do not change anything else.

### DETERMINISTIC

Steps the agent applies. Each step must be safe and have one correct outcome.

1. First step.

### REPORT

Items the agent only reports. It must not act on them.

- Something that needs a human decision.

## Verify

Run this command. It must report no violations for this rule.

```
fscale check --rules pack/rule-id
```
