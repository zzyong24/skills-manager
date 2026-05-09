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

---

## Important: write operations require the App to be closed

The CLI and App share the same SQLite DB (path shown in `repo status → db_path`,
default `~/.skills-manager/skills-manager.db`).

- **Read operations** (`list`, `show`, `repo status`, `git status`): safe while App is open.
- **Write operations** (`tag`, `untag`, `enable`, `disable`, `scenarios add-skill`, etc.): **close the App first** — concurrent writes from the App will overwrite CLI changes.

---

## Full command reference

Add `--json` to any command for machine-readable output.
Use `--skills-root <path>` as a global flag to point at a non-default skills directory.

### repo
```bash
$CLI --json repo status          # path, skill count, scenario count
$CLI repo set-path <path>        # change skills root directory
$CLI repo reset-path             # restore default (~/.skills-manager)
```

### tools
```bash
$CLI --json tools list           # list all detected agent tools with install status
```

### skills — read
```bash
$CLI --json skills list                             # all skills with tags/enabled/scenarios
$CLI --json skills show <ref>                       # detail + full SKILL.md content
$CLI --json skills export <ref> --dest <path>       # export skill to directory
```

### skills — write (close App first)
```bash
$CLI --json skills tag <ref> "tag1,tag2"            # append tags (comma-separated, deduped)
$CLI --json skills untag <ref> <tag>                # remove one tag
$CLI --json skills set-tags <ref> "tag1,tag2"       # replace all tags (empty = clear all)
$CLI --json skills enable <ref>                     # enable a skill
$CLI --json skills disable <ref>                    # disable a skill
```

`<ref>` accepts: skill name, id, or folder name.

### scenarios
```bash
$CLI --json scenarios list                            # list all scenarios
$CLI --json scenarios current                         # active scenario
$CLI --json scenarios preview <ref>                   # preview what will be synced before applying
$CLI --json scenarios apply <ref>                     # switch and apply to all agents
$CLI --json scenarios add-skill <scenario> <skill>    # add skill to scenario (close App first)
$CLI --json scenarios remove-skill <scenario> <skill> # remove skill from scenario
```

### git
```bash
$CLI --json git status             # backup status (remote, branch, health)
$CLI git init                      # init git repo in skills directory
$CLI git clone <url>               # clone remote into skills directory (first-time setup)
$CLI git set-remote <url>          # set remote URL
$CLI git pull                      # pull latest from remote
$CLI git push                      # push to remote
$CLI git commit -m "message"       # commit all changes + create snapshot tag
$CLI --json git versions           # list all snapshot versions
$CLI git restore <tag>             # restore a snapshot version
```

### tools
```bash
$CLI --json tools list    # list all detected agent tools (Claude Code, Cursor, etc.)
```

---

## Typical workflow: auto-tag all skills by category

```
1. sm_list_skills      → get all skills with name + description
2. analyze each skill  → decide category tags
3. sm_batch_tag        → close App, bulk-write tags
4. sm_list_skills      → verify results
5. sm_git_sync         → commit and push to remote
```

## Suggested tag categories

| Tag | Use for |
|-----|---------|
| `writing` | articles, copywriting, scripts, presentations |
| `dev` | coding, debugging, architecture, CI |
| `research` | information gathering, analysis, competitive research |
| `data` | data processing, visualization, scraping |
| `media` | video, image, audio |
| `infrastructure` | system maintenance, config, backup |
| `productivity` | efficiency tools, task management |
| `ai` | AI models, prompt engineering |
