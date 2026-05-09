# Scenario Management Workflow

Use this workflow when asked to "manage scenarios", "add skills to a scenario", "switch scenario", or "set up a new scenario".

## What is a scenario?

A scenario is a named set of skills that gets synced to all your agent tools (Claude Code, Cursor, etc.) at once. Switching scenarios lets you load different skill sets for different workflows.

## Prerequisite

Write operations (add-skill, remove-skill) require the App to be closed.
Read operations (list, current, preview) are safe with App open.

---

## Common workflows

### Check current state
```bash
$CLI --json scenarios list      # all scenarios + skill counts
$CLI --json scenarios current   # which one is active
```

### Preview before switching
```bash
# See exactly what files will be synced before applying
$CLI --json scenarios preview <scenario-name>
```

### Switch scenario
```bash
$CLI --json scenarios apply <scenario-name>
# This syncs the scenario's skills to all detected agent tools immediately.
```

### Add a skill to a scenario (close App first)
```bash
$CLI --json scenarios add-skill <scenario-name> <skill-name>
```

### Remove a skill from a scenario (close App first)
```bash
$CLI --json scenarios remove-skill <scenario-name> <skill-name>
```

### Bulk-add multiple skills to a scenario
```bash
# Close App first
for skill in skill-a skill-b skill-c; do
  $CLI --json scenarios add-skill "My Scenario" "$skill"
done
```

### Verify scenario membership
```bash
# List all skills that belong to a scenario
$CLI --json skills list | python3 -c "
import sys, json
d = json.load(sys.stdin)
scenario = 'My Scenario'
members = [s['name'] for s in d if scenario in s.get('scenarios', [])]
print(f'{len(members)} skills in \"{scenario}\":')
for m in members: print(' -', m)
"
```

---

## Notes

- `scenarios apply` syncs via symlinks by default — switching is instant.
- Skills not in the active scenario are still in your library, just not synced to agents.
- If a skill appears in multiple scenarios, it will be present whenever any of those scenarios is active.
