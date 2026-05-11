# Auto-Tagging Workflow

Use this workflow when asked to "tag all skills", "auto-classify skills", or "organize skills by category".

## Prerequisite

Close the Skills Manager App before starting — write operations require exclusive DB access.

## Steps

### 1. List all skills
```bash
$CLI --json skills list
```
Returns: array of `{ name, description, tags, enabled }` for all skills.

### 2. Analyze and decide tags

For each skill, read `name` + `description` and assign one or more tags from the categories below.
A skill can have multiple tags (e.g. `["dev", "ai"]`).

| Tag | Assign when the skill is about... |
|-----|----------------------------------|
| `writing` | articles, copywriting, scripts, presentations, docs |
| `dev` | coding, debugging, architecture, CI/CD, git |
| `research` | information gathering, analysis, competitive intel |
| `data` | data processing, visualization, scraping, analytics |
| `media` | video, image, audio generation or processing |
| `infrastructure` | system maintenance, config, backup, deployment |
| `productivity` | task management, scheduling, efficiency tools |
| `ai` | AI models, prompt engineering, fine-tuning, LLMs |
| `integration` | third-party APIs, webhooks, platform connectors |
| `social` | social media, content distribution, community |

### 3. Apply tags in bulk (close App first)
```bash
# One skill at a time:
$CLI --json skills set-tags <ref> "tag1,tag2"

# Or loop through your mapping:
for skill in skill-a skill-b skill-c; do
  $CLI --json skills set-tags "$skill" "dev,ai"
done
```

### 4. Verify
```bash
$CLI --json skills list | python3 -c "
import sys, json
d = json.load(sys.stdin)
tagged = [s for s in d if s['tags']]
print(f'{len(tagged)}/{len(d)} skills tagged')
"
```

### 5. Commit to Git backup
```bash
$CLI git commit -m "chore: auto-tag all skills by category"
$CLI git push
```
