# Skills Manager CLI — Agent Guide

## Step 1: Locate the CLI binary

Check in priority order — use the first one found:

```bash
# 1. Bundled with the App (recommended — no extra install needed)
#    macOS
/Applications/skills-manager.app/Contents/MacOS/skills-manager-cli
#    Windows
%LOCALAPPDATA%\skills-manager\skills-manager-cli.exe
#    Linux
~/.local/bin/skills-manager-cli

# 2. Installed via cargo
~/.cargo/bin/skills-manager-cli

# 3. Manually downloaded from GitHub Releases and placed on PATH
skills-manager-cli

# 4. Built from source (developers only)
# Run: npm run cli:install  (inside the skills-manager repo)
# or:  cargo install --path src-tauri --bin skills-manager-cli
```

Detection script (macOS / Linux / Git Bash on Windows):

```bash
find_cli() {
  for candidate in \
    "/Applications/skills-manager.app/Contents/MacOS/skills-manager-cli" \
    "$HOME/.local/bin/skills-manager-cli" \
    "/usr/local/bin/skills-manager-cli" \
    "$HOME/.cargo/bin/skills-manager-cli" \
    "$(which skills-manager-cli 2>/dev/null)"; do
    [ -x "$candidate" ] && echo "$candidate" && return 0
  done
  echo "ERROR: skills-manager-cli not found." >&2
  echo "Download from: https://github.com/xingkongliang/skills-manager/releases" >&2
  return 1
}
CLI=$(find_cli) || exit 1
```

> **npm install coming soon** — a global `npm install -g @skills-manager/cli` option is planned for CI/CD environments.

---

## Important: write operations require the App to be closed

The CLI and App share the same SQLite DB (path shown in `repo status → db_path`,
default `~/.skills-manager/skills-manager.db`).

- **Read** (`list`, `show`, `repo status`, `git status`): safe while App is open.
- **Write** (`tag`, `untag`, `enable`, `disable`, `presets add-skill`, etc.): **close the App first**.

---

## Command reference

Add `--json` to any command for machine-readable output.
Use `--skills-root <path>` as a global flag to point at a non-default skills directory.

### repo
```bash
$CLI --json repo status          # path, skill count, preset count
$CLI repo set-path <path>        # change skills root directory
$CLI repo reset-path             # restore default (~/.skills-manager)
```

### tools
```bash
$CLI --json tools list           # list all detected agent tools with install status
```

### skills — read
```bash
$CLI --json skills list                          # all skills with tags/enabled/presets
$CLI --json skills show <ref>                    # detail + full SKILL.md content
$CLI --json skills export <ref> --dest <path>    # export skill to directory
```

### skills — write (close App first)
```bash
$CLI --json skills tag <ref> "tag1,tag2"         # append tags (comma-separated, deduped)
$CLI --json skills untag <ref> <tag>             # remove one tag
$CLI --json skills set-tags <ref> "tag1,tag2"    # replace all tags (empty = clear all)
$CLI --json skills enable <ref>                  # enable a skill
$CLI --json skills disable <ref>                 # disable a skill
```

`<ref>` accepts: skill name, id, or folder name.

### presets
```bash
$CLI --json presets list                            # list all presets
$CLI --json presets current                         # active preset
$CLI --json presets preview <ref>                   # preview what will be synced
$CLI --json presets apply <ref>                     # switch and apply to all agents
$CLI --json presets add-skill <preset> <skill>    # add skill to preset (close App first)
$CLI --json presets remove-skill <preset> <skill> # remove skill from preset
```

### git
```bash
$CLI --json git status             # backup status (remote, branch, health)
$CLI git init                      # init git repo in skills directory
$CLI git clone <url>               # clone remote (first-time setup)
$CLI git set-remote <url>          # set remote URL
$CLI git pull / push               # sync with remote
$CLI git commit -m "message"       # commit + create snapshot tag
$CLI --json git versions           # list snapshot versions
$CLI git restore <tag>             # restore a snapshot
```

---

## Detailed workflows

For step-by-step guides, read the relevant reference before acting:

| Task | Reference |
|------|-----------|
| Auto-tag all skills by category | `references/tagging-workflow.md` |
| Manage preset membership | `references/presets-workflow.md` |
