---
name: loop-fixture
description: Stands in for the SKILL.md of every sample rule in the loop-pack test fixtures. Use only in firescale's own tests of the fix loop.
---

# loop fixtures

## What this rule enforces and why

Nothing real. These rules exist to exercise the fix loop: ordering by `after`, convergence, and the errors when fixes do not settle.

## Manual fix

Not applicable.

## Agent fix instructions

### DETERMINISTIC

None.

### REPORT

None.

## Verify

```
fscale check
```
