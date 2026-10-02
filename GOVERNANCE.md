# Governance

> How ZhiWei makes decisions and how contributors can participate.

## Overview

ZhiWei is a personal open-source project maintained by hancic128. The project
operates under a **benevolent dictator** model with a clear vision and scope.

## Project Maintainer

| Role | Person | Responsibility |
| --- | --- | --- |
| BDFL / Maintainer | hancic128 | Final decision authority, security issues, releases |

## Decision Making

### What Requires Approval

| Type | Process |
| --- | --- |
| Bug fixes | Self-merge after CI passes |
| Minor features | PR + one maintainer review |
| Major features / Architecture | Discussion + RFC in issues + maintainer approval |
| Breaking changes | Maintainer-only merge |
| Security fixes | Maintainer + coordinated disclosure |

### How to Influence Decisions

1. **Open an issue first** for significant changes
2. **Read the positioning document** ([docs/POSITIONING.md](./docs/POSITIONING.md))
3. **Align with the project's goals**: single operator, multi-machine management
4. **Be patient**: this is a one-person project

## Contribution Pathway

```
1. Contributor (external)
   └─> Opens PRs, reports issues
           │
2. Recognized Contributor (invited)
   └─> Has merge rights for doc/bug fixes
           │
3. Maintainer (hancic128)
       └─> Final say on all decisions
```

## Issue Handling

Issues are closed without action if they:
- Don't align with the project's goals (single operator managing multiple machines)
- Request out-of-scope features (multi-user, HA, dashboards, etc.)
- Are duplicates or lack sufficient detail

This is not unfriendly - it's how a single-maintainer project stays focused.

## Conflict Resolution

1. Start with a respectful discussion
2. If unresolved, maintainer has final say
3. Escalation is not appropriate - this is a personal project

## Contact

- **Issues**: GitHub Issues
- **Security**: See [SECURITY.md](./SECURITY.md)
- **Email**: hancic128+conduct@proton.me (Code of Conduct matters only)

## Changes to Governance

This document may be updated as the project evolves. Significant changes
will be announced via GitHub Discussion.
