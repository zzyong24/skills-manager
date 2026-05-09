# Preset Management Workflow

Use this workflow when asked to "manage presets", "add skills to a preset", "switch preset", or "set up a new preset".

## What is a preset?

A preset is a named set of skills that gets synced to all your agent tools (Claude Code, Cursor, etc.) at once. Switching presets lets you load different skill sets for different workflows.

## Prerequisite

Write operations (add-skill, remove-skill) require the App to be closed.
Read operations (list, current, preview) are safe with App open.

---

## Common workflows

### Check current state
```bash
$CLI --json presets list      # all presets + skill counts
$CLI --json presets current   # which one is active
```

### Preview before switching
```bash
# See exactly what files will be synced before applying
$CLI --json presets preview <preset-name>
```

### Switch preset
```bash
$CLI --json presets apply <preset-name>
# This syncs the preset's skills to all detected agent tools immediately.
```

### Add a skill to a preset (close App first)
```bash
$CLI --json presets add-skill <preset-name> <skill-name>
```

### Remove a skill from a preset (close App first)
```bash
$CLI --json presets remove-skill <preset-name> <skill-name>
```

### Bulk-add multiple skills to a preset
```bash
# Close App first
for skill in skill-a skill-b skill-c; do
  $CLI --json presets add-skill "My Preset" "$skill"
done
```

### Verify preset membership
```bash
# List all skills that belong to a preset
$CLI --json skills list | python3 -c "
import sys, json
d = json.load(sys.stdin)
preset = 'My Preset'
members = [s['name'] for s in d if preset in s.get('scenarios', [])]
print(f'{len(members)} skills in \"{preset}\":')
for m in members: print(' -', m)
"
```

---

## Notes

- `presets apply` syncs via symlinks by default — switching is instant.
- Skills not in the active preset are still in your library, just not synced to agents.
- If a skill appears in multiple presets, it will be present whenever any of those presets is active.
