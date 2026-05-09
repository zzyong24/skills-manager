---
name: skills-manager-cli
description: Use skills-manager-cli to let agents read and write Skills Manager data — tag skills, manage scenarios, and sync backups without opening the GUI.
---

# Skills Manager CLI Skill

Use `skills-manager-cli` to let agents read and write Skills Manager data directly — tag skills, manage scenarios, and sync backups without opening the GUI.

## What it can do

| Operation | Method |
|-----------|--------|
| List all skills (tags / enabled / scenario membership) | `skills list --json` |
| View skill detail + SKILL.md content | `skills show` |
| Add / remove / replace tags on a skill | `skills tag` / `untag` / `set-tags` |
| Enable or disable a skill | `skills enable` / `disable` |
| List / switch scenarios | `scenarios list` / `apply` |
| Add / remove a skill from a scenario | `scenarios add-skill` / `remove-skill` |
| Git backup status / commit / push | `git status` / `commit` / `push` |
| View repo status | `repo status` |

## Typical use cases

```
"Auto-tag all my skills by category"
→ sm_list_skills → analyze name+description → sm_batch_tag

"Switch to the dev scenario"
→ sm_apply_scenario "dev"

"Back up current skill state"
→ sm_git_sync "chore: snapshot"
```

## Requirements

- Skills Manager app installed: https://github.com/xingkongliang/skills-manager
- CLI binary available (bundled with the app or installed separately — see guidance)
