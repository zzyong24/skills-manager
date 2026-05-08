use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::{
    error::AppError,
    skill_store::{ProjectRecord, ScenarioRecord, SkillStore, SkillTargetRecord},
    sync_engine, tool_adapters,
    tool_service,
};

/// Returns true if `child` lives under `parent`.
///
/// Important: we canonicalize `parent` (to absorb symlinks / `..` / trailing
/// slashes in the project root), but we do NOT canonicalize `child`. If `child`
/// is itself a symlink — which is exactly what Skills Manager writes into
/// project directories — `std::fs::canonicalize` would follow it to the
/// central repo target (e.g. `~/.skills-manager/skills/html-ppt`) and make the
/// test falsely return `false`. That would prevent unsync from ever cleaning
/// up subscription-owned symlinks.
///
/// Instead, we canonicalize `child`'s parent directory (same logic, without
/// following the leaf symlink) and append the leaf file name back. This keeps
/// the comparison semantically "is this path lexically inside the project?"
/// while still handling symlinked parent dirs correctly.
pub(crate) fn path_is_under(child: &Path, parent: &Path) -> bool {
    let canonical_parent = std::fs::canonicalize(parent).ok();

    // Canonicalize child's PARENT (without following the leaf), then re-append
    // the leaf name. std::fs::canonicalize() on a symlink follows it; we don't
    // want that for the leaf, because subscription symlinks legitimately point
    // OUT of the project tree into the central repo.
    let canonical_child = match (child.parent(), child.file_name()) {
        (Some(cp), Some(name)) => std::fs::canonicalize(cp)
            .ok()
            .map(|p| p.join(name)),
        _ => None,
    };

    if let (Some(c), Some(p)) = (canonical_child.as_ref(), canonical_parent.as_ref()) {
        return c != p && c.starts_with(p);
    }

    // Lexical fallback: PathBuf::starts_with compares path components,
    // so /a/proj does NOT match /a/proj-legacy.
    let c = PathBuf::from(child);
    let p = PathBuf::from(parent);
    c != p && c.starts_with(&p)
}


/// Returns None if the tool is not actually used in this project
/// (i.e. its detect directory doesn't exist under the project root).
pub(crate) fn resolve_project_skill_target(
    project: &ProjectRecord,
    adapter: &tool_adapters::ToolAdapter,
    skill_name: &str,
) -> Option<PathBuf> {
    if project.workspace_type == "linked" {
        let project_path = PathBuf::from(&project.path);
        if !project_path.exists() {
            log::warn!("Linked workspace path does not exist: {}", project_path.display());
            return None;
        }
        return Some(project_path.join(skill_name));
    }
    // Check if this tool is actually being used in this project.
    // e.g. .claude/ or .cursor/ must exist under the project root.
    let detect_dir = PathBuf::from(&project.path).join(&adapter.relative_detect_dir);
    if !detect_dir.exists() {
        return None;
    }
    Some(
        PathBuf::from(&project.path)
            .join(&adapter.relative_skills_dir)
            .join(skill_name),
    )
}

#[derive(Debug, Clone)]
pub struct ScenarioSyncTarget {
    pub skill_id: String,
    pub skill_name: String,
    pub tool: String,
    pub source: PathBuf,
    pub target: PathBuf,
    pub mode: sync_engine::SyncMode,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncPreviewTarget {
    pub skill_id: String,
    pub skill_name: String,
    pub tool: String,
    pub target_path: String,
    pub mode: String,
}

pub fn ensure_scenario_exists(store: &SkillStore, scenario_id: &str) -> Result<(), AppError> {
    let exists = store
        .get_all_scenarios()
        .map_err(AppError::db)?
        .iter()
        .any(|s| s.id == scenario_id);
    if !exists {
        return Err(AppError::not_found("Scenario not found"));
    }
    Ok(())
}

pub fn enabled_installed_adapters_for_scenario_skill(
    store: &SkillStore,
    scenario_id: &str,
    skill_id: &str,
) -> Result<Vec<tool_adapters::ToolAdapter>, AppError> {
    let adapters = tool_adapters::enabled_installed_adapters(store);
    let adapter_keys: Vec<String> = adapters.iter().map(|a| a.key.clone()).collect();

    store
        .ensure_scenario_skill_tool_defaults(scenario_id, skill_id, &adapter_keys)
        .map_err(AppError::db)?;

    let enabled = store
        .get_enabled_tools_for_scenario_skill(scenario_id, skill_id)
        .map_err(AppError::db)?;
    let enabled_set: HashSet<String> = enabled.into_iter().collect();

    Ok(adapters
        .into_iter()
        .filter(|adapter| enabled_set.contains(&adapter.key))
        .collect())
}

pub fn collect_scenario_sync_targets(
    store: &SkillStore,
    scenario_id: &str,
) -> Result<Vec<ScenarioSyncTarget>, AppError> {
    let skills = store
        .get_skills_for_scenario(scenario_id)
        .map_err(AppError::db)?;
    let configured_mode = store.get_setting("sync_mode").map_err(AppError::db)?;
    let mut targets = Vec::new();

    for skill in &skills {
        let source = PathBuf::from(&skill.central_path);
        let target_name = sync_engine::target_dir_name(&source, &skill.name);
        let adapters = enabled_installed_adapters_for_scenario_skill(store, scenario_id, &skill.id)?;
        for adapter in &adapters {
            let target = adapter.skills_dir().join(&target_name);
            let mode = sync_engine::sync_mode_for_tool(&adapter.key, configured_mode.as_deref());
            targets.push(ScenarioSyncTarget {
                skill_id: skill.id.clone(),
                skill_name: skill.name.clone(),
                tool: adapter.key.clone(),
                source: source.clone(),
                target,
                mode,
            });
        }
    }

    Ok(targets)
}

pub fn preview_scenario_sync(
    store: &SkillStore,
    scenario_id: &str,
) -> Result<Vec<SyncPreviewTarget>, AppError> {
    collect_scenario_sync_targets(store, scenario_id).map(|targets| {
        targets
            .into_iter()
            .map(|target| SyncPreviewTarget {
                skill_id: target.skill_id,
                skill_name: target.skill_name,
                tool: target.tool,
                target_path: target.target.to_string_lossy().to_string(),
                mode: target.mode.as_str().to_string(),
            })
            .collect()
    })
}

pub fn sync_desired_targets(
    store: &SkillStore,
    desired_targets: &[ScenarioSyncTarget],
) -> Result<(), AppError> {
    let existing_targets: HashMap<(String, String), SkillTargetRecord> = store
        .get_all_targets()
        .map_err(AppError::db)?
        .into_iter()
        .map(|target| ((target.skill_id.clone(), target.tool.clone()), target))
        .collect();

    for desired in desired_targets {
        let key = (desired.skill_id.clone(), desired.tool.clone());
        if let Some(existing) = existing_targets.get(&key) {
            let target_path = PathBuf::from(&existing.target_path);
            if target_path != desired.target {
                if let Err(e) = sync_engine::remove_target(&target_path) {
                    log::warn!(
                        "Failed to remove stale target {}: {e}",
                        target_path.display()
                    );
                }
                if let Err(e) = store.delete_target(&desired.skill_id, &desired.tool) {
                    log::warn!(
                        "Failed to delete stale target record for skill {}, tool {}: {e}",
                        desired.skill_id,
                        desired.tool
                    );
                }
            } else if existing.mode == desired.mode.as_str()
                && existing.status == "ok"
                && sync_engine::is_target_current(&desired.source, &desired.target, desired.mode)
            {
                continue;
            }
        }

        match sync_engine::sync_skill(&desired.source, &desired.target, desired.mode) {
            Ok(actual_mode) => {
                let now = chrono::Utc::now().timestamp_millis();
                let target_record = SkillTargetRecord {
                    id: uuid::Uuid::new_v4().to_string(),
                    skill_id: desired.skill_id.clone(),
                    tool: desired.tool.clone(),
                    target_path: desired.target.to_string_lossy().to_string(),
                    mode: actual_mode.as_str().to_string(),
                    status: "ok".to_string(),
                    synced_at: Some(now),
                    last_error: None,
                };
                if let Err(e) = store.insert_target(&target_record) {
                    log::warn!(
                        "Failed to insert sync target for skill {}: {e}",
                        desired.skill_id
                    );
                }
            }
            Err(e) => {
                log::warn!(
                    "Failed to sync skill {} to {}: {e}",
                    desired.skill_id,
                    desired.target.display()
                );
            }
        }
    }

    Ok(())
}

pub fn unsync_obsolete_scenario_targets(
    store: &SkillStore,
    old_scenario_id: &str,
    desired_targets: &[ScenarioSyncTarget],
) -> Result<(), AppError> {
    let desired_paths: HashMap<(String, String), PathBuf> = desired_targets
        .iter()
        .map(|target| {
            (
                (target.skill_id.clone(), target.tool.clone()),
                target.target.clone(),
            )
        })
        .collect();

    // P0-5: project-level targets are owned by the project_scenarios subscription
    // lifecycle, NOT by the active-scenario sweep. If we don't exclude them here,
    // switching the active scenario (or app startup picking a different default)
    // wipes every subscribed project's symlinks, even though the subscriptions
    // themselves remain in the DB. Result: users see "bound" scenes in the UI
    // but missing skill files on disk until they manually re-bind.
    let project_paths = collect_project_paths(store);

    let old_skill_ids = store
        .get_skill_ids_for_scenario(old_scenario_id)
        .map_err(AppError::db)?;
    for skill_id in &old_skill_ids {
        let targets = store.get_targets_for_skill(skill_id).unwrap_or_default();
        for target in &targets {
            let path = PathBuf::from(&target.target_path);
            let key = (skill_id.clone(), target.tool.clone());
            if desired_paths.get(&key) == Some(&path) {
                continue;
            }
            if is_project_owned_target(&path, &project_paths) {
                continue;
            }

            if let Err(e) = sync_engine::remove_target(&path) {
                log::warn!("Failed to remove sync target {}: {e}", path.display());
            }
            if let Err(e) = store.delete_target(skill_id, &target.tool) {
                log::warn!(
                    "Failed to delete target record for skill {skill_id}, tool {}: {e}",
                    target.tool
                );
            }
        }
    }

    Ok(())
}

pub fn unsync_scenario_skills(store: &SkillStore, scenario_id: &str) -> Result<(), AppError> {
    let skill_ids = store
        .get_skill_ids_for_scenario(scenario_id)
        .map_err(AppError::db)?;

    // P0-5: same reasoning as unsync_obsolete_scenario_targets — never touch
    // targets that live inside a project directory. They belong to the
    // subscription system (bind/unbind/delete-scenario/remove-project), not
    // to active-scenario unsync sweeps that fire on app startup, scenario
    // creation, scenario deletion, and tray-menu scenario switches.
    let project_paths = collect_project_paths(store);

    for skill_id in &skill_ids {
        let targets = store.get_targets_for_skill(skill_id).unwrap_or_default();
        for target in &targets {
            let path = PathBuf::from(&target.target_path);
            if is_project_owned_target(&path, &project_paths) {
                continue;
            }
            if let Err(e) = sync_engine::remove_target(&path) {
                log::warn!("Failed to remove sync target {}: {e}", path.display());
            }
            if let Err(e) = store.delete_target(skill_id, &target.tool) {
                log::warn!(
                    "Failed to delete target record for skill {skill_id}, tool {}: {e}",
                    target.tool
                );
            }
        }
    }

    Ok(())
}

/// Collect canonical-friendly project root paths once per call, so the
/// per-target check below is cheap. canonicalize() is attempted lazily by
/// path_is_under, so we just hand it the raw stored paths.
fn collect_project_paths(store: &SkillStore) -> Vec<PathBuf> {
    store
        .get_all_projects()
        .unwrap_or_default()
        .into_iter()
        .map(|p| PathBuf::from(p.path))
        .collect()
}

/// Returns true if `path` lives inside any known project directory.
/// Used to fence off subscription-owned targets from active-scenario sweeps.
fn is_project_owned_target(path: &Path, project_paths: &[PathBuf]) -> bool {
    project_paths
        .iter()
        .any(|proj| path_is_under(path, proj))
}

pub fn sync_scenario_skills(store: &SkillStore, scenario_id: &str) -> Result<(), AppError> {
    let desired_targets = collect_scenario_sync_targets(store, scenario_id)?;
    sync_desired_targets(store, &desired_targets)
}

pub fn apply_scenario_to_default(store: &SkillStore, scenario_id: &str) -> Result<(), AppError> {
    ensure_scenario_exists(store, scenario_id)?;
    let desired_targets = collect_scenario_sync_targets(store, scenario_id)?;

    if let Ok(Some(old_id)) = store.get_active_scenario_id() {
        if old_id != scenario_id {
            unsync_obsolete_scenario_targets(store, &old_id, &desired_targets)?;
        }
    }

    store.set_active_scenario(scenario_id).map_err(AppError::db)?;
    sync_desired_targets(store, &desired_targets)
}

pub fn sync_skill_to_active_scenario(
    store: &SkillStore,
    scenario_id: &str,
    skill_id: &str,
) -> Result<(), AppError> {
    if let Ok(Some(active_id)) = store.get_active_scenario_id() {
        if active_id == scenario_id {
            let adapters = enabled_installed_adapters_for_scenario_skill(store, scenario_id, skill_id)?;
            let configured_mode = store.get_setting("sync_mode").map_err(AppError::db)?;
            let Ok(Some(skill)) = store.get_skill_by_id(skill_id) else {
                return Ok(());
            };
            let source = PathBuf::from(&skill.central_path);
            let target_name = sync_engine::target_dir_name(&source, &skill.name);
            let old_targets = store.get_targets_for_skill(skill_id).unwrap_or_default();
            for adapter in &adapters {
                if let Some(old) = old_targets.iter().find(|t| t.tool == adapter.key) {
                    let old_path = PathBuf::from(&old.target_path);
                    if old_path != adapter.skills_dir().join(&target_name) {
                        if let Err(e) = sync_engine::remove_target(&old_path) {
                            log::warn!("Failed to remove stale target {}: {e}", old_path.display());
                        }
                        let _ = store.delete_target(skill_id, &adapter.key);
                    }
                }

                let target = adapter.skills_dir().join(&target_name);
                let mode = sync_engine::sync_mode_for_tool(&adapter.key, configured_mode.as_deref());
                match sync_engine::sync_skill(&source, &target, mode) {
                    Ok(actual_mode) => {
                        let now = chrono::Utc::now().timestamp_millis();
                        let target_record = super::skill_store::SkillTargetRecord {
                            id: uuid::Uuid::new_v4().to_string(),
                            skill_id: skill_id.to_string(),
                            tool: adapter.key.clone(),
                            target_path: target.to_string_lossy().to_string(),
                            mode: actual_mode.as_str().to_string(),
                            status: "ok".to_string(),
                            synced_at: Some(now),
                            last_error: None,
                        };
                        if let Err(e) = store.insert_target(&target_record) {
                            log::warn!("Failed to insert sync target for skill {skill_id}: {e}");
                        }
                    }
                    Err(e) => {
                        log::warn!(
                            "Failed to sync skill {skill_id} to {}: {e}",
                            target.display()
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn ensure_default_startup_scenario(store: &SkillStore) -> Result<(), AppError> {
    let mut scenarios = store.get_all_scenarios().map_err(AppError::db)?;
    if scenarios.is_empty() {
        let now = chrono::Utc::now().timestamp_millis();
        let default_scenario = ScenarioRecord {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Default".to_string(),
            description: Some("Default startup scenario".to_string()),
            icon: None,
            sort_order: 0,
            created_at: now,
            updated_at: now,
        };
        store.insert_scenario(&default_scenario).map_err(AppError::db)?;
        scenarios.push(default_scenario);
    }

    let current_active = store.get_active_scenario_id().map_err(AppError::db)?;
    let preferred_default = store.get_setting("default_scenario").ok().flatten();

    let desired_active = preferred_default
        .filter(|id| scenarios.iter().any(|scenario| scenario.id == *id))
        .or_else(|| {
            current_active
                .clone()
                .filter(|id| scenarios.iter().any(|scenario| scenario.id == *id))
        })
        .unwrap_or_else(|| scenarios[0].id.clone());

    if current_active.as_deref() != Some(desired_active.as_str()) {
        if let Some(old_active) = current_active.as_deref() {
            unsync_scenario_skills(store, old_active)?;
        }
        store
            .set_active_scenario(&desired_active)
            .map_err(AppError::db)?;
    }

    sync_scenario_skills(store, &desired_active)
}

/// P0-6: Reconcile project-scenario subscription symlinks at app startup.
///
/// Why this exists:
///   Binding a scenario to a project only writes the symlinks once (at bind time).
///   There is nothing in the original startup path that re-asserts them. So if the
///   physical files get wiped between runs — by a prior bug (P0-5), manual cleanup,
///   a sync'd backup restore, or the OS doing housekeeping — the DB still records
///   the subscription but the project directory stays empty. Users see "bound
///   scenes" in the UI without the skill files, and the only way to recover is
///   to manually unbind + rebind.
///
/// What this function does:
///   For every (project, scenario) row in project_scenarios, re-run the same
///   symlink-writing logic that bind_scenario_to_project does. sync_engine::sync_skill
///   is idempotent — it will:
///     * create a missing symlink
///     * leave a correct existing symlink untouched
///     * fix a dangling / wrong-target symlink
///   And insert_target uses ON CONFLICT (skill_id, tool) DO UPDATE, so repeat
///   calls don't accumulate rows.
///
/// Failure mode:
///   Per-skill / per-project errors are logged and swallowed — a single broken
///   project must not prevent the rest from being reconciled, nor block app startup.
pub fn reconcile_project_subscriptions(store: &SkillStore) -> Result<(), AppError> {
    let projects = store.get_all_projects().map_err(AppError::db)?;
    if projects.is_empty() {
        return Ok(());
    }

    let configured_mode = store.get_setting("sync_mode").map_err(AppError::db)?;
    let mut reconciled_bindings = 0usize;
    let mut reconciled_skills = 0usize;
    let mut failures = 0usize;

    for project in &projects {
        let scenario_ids = match store.get_project_scenario_ids(&project.id) {
            Ok(ids) => ids,
            Err(e) => {
                log::warn!(
                    "reconcile: failed to read subscriptions for project {}: {e}",
                    project.id
                );
                failures += 1;
                continue;
            }
        };

        for scenario_id in &scenario_ids {
            reconciled_bindings += 1;
            let skill_ids = match store.get_skill_ids_for_scenario(scenario_id) {
                Ok(ids) => ids,
                Err(e) => {
                    log::warn!(
                        "reconcile: failed to read skills for scenario {scenario_id}: {e}"
                    );
                    failures += 1;
                    continue;
                }
            };

            for skill_id in &skill_ids {
                let Ok(Some(skill)) = store.get_skill_by_id(skill_id) else {
                    // Skill row was removed but scenario_skills still pointed at it;
                    // broadcast/cleanup races. Skip — it's not our job to fix it here.
                    continue;
                };

                let adapters = match enabled_installed_adapters_for_scenario_skill(
                    store,
                    scenario_id,
                    skill_id,
                ) {
                    Ok(a) => a,
                    Err(e) => {
                        log::warn!(
                            "reconcile: failed to resolve adapters for skill {skill_id} in scenario {scenario_id}: {e}"
                        );
                        failures += 1;
                        continue;
                    }
                };

                let source = PathBuf::from(&skill.central_path);
                let target_name = sync_engine::target_dir_name(&source, &skill.name);

                for adapter in &adapters {
                    let Some(target) =
                        resolve_project_skill_target(project, adapter, &target_name)
                    else {
                        continue; // tool not used in this project, skip
                    };
                    let mode =
                        sync_engine::sync_mode_for_tool(&adapter.key, configured_mode.as_deref());

                    match sync_engine::sync_skill(&source, &target, mode) {
                        Ok(actual_mode) => {
                            reconciled_skills += 1;
                            let now = chrono::Utc::now().timestamp_millis();
                            let record = SkillTargetRecord {
                                id: uuid::Uuid::new_v4().to_string(),
                                skill_id: skill_id.to_string(),
                                tool: adapter.key.clone(),
                                target_path: target.to_string_lossy().to_string(),
                                mode: actual_mode.as_str().to_string(),
                                status: "ok".to_string(),
                                synced_at: Some(now),
                                last_error: None,
                            };
                            if let Err(e) = store.insert_target(&record) {
                                log::warn!(
                                    "reconcile: failed to upsert target for skill {skill_id} in project {}: {e}",
                                    project.id
                                );
                                failures += 1;
                            }
                        }
                        Err(e) => {
                            log::warn!(
                                "reconcile: failed to sync skill {skill_id} to project {} (tool={}): {e}",
                                project.id,
                                adapter.key
                            );
                            failures += 1;
                        }
                    }
                }
            }
        }
    }

    log::info!(
        "reconcile_project_subscriptions: {reconciled_bindings} binding(s), {reconciled_skills} target(s) ensured, {failures} failure(s)"
    );
    Ok(())
}

pub fn sync_active_scenario_to_tool(store: &SkillStore, tool_key: &str) {
    if let Ok(Some(active_id)) = store.get_active_scenario_id() {
        let Ok(skill_ids) = store.get_skill_ids_for_scenario(&active_id) else {
            return;
        };
        for skill_id in skill_ids {
            if let Ok(adapters) = enabled_installed_adapters_for_scenario_skill(store, &active_id, &skill_id)
            {
                if adapters.iter().any(|adapter| adapter.key == tool_key) {
                    let _ = sync_skill_to_active_scenario(store, &active_id, &skill_id);
                }
            }
        }
    }
}

pub fn sync_single_skill_to_tool(
    store: &SkillStore,
    skill_id: &str,
    tool: &str,
) -> Result<(), AppError> {
    let adapter = tool_adapters::find_adapter_with_store(store, tool)
        .ok_or_else(|| AppError::not_found(format!("Unknown tool: {}", tool)))?;

    if !adapter.is_installed() {
        return Err(AppError::not_found(format!(
            "{} is not installed",
            adapter.display_name
        )));
    }

    if tool_service::get_disabled_tools(store).contains(&tool.to_string()) {
        return Err(AppError::invalid_input(format!(
            "{} is disabled",
            adapter.display_name
        )));
    }

    let skill = store
        .get_skill_by_id(skill_id)
        .map_err(AppError::db)?
        .ok_or_else(|| AppError::not_found("Skill not found"))?;

    let source = PathBuf::from(&skill.central_path);
    let target = adapter
        .skills_dir()
        .join(sync_engine::target_dir_name(&source, &skill.name));
    let configured_mode = store.get_setting("sync_mode").map_err(AppError::db)?;
    let mode = sync_engine::sync_mode_for_tool(tool, configured_mode.as_deref());
    let actual_mode = sync_engine::sync_skill(&source, &target, mode).map_err(AppError::io)?;

    let now = chrono::Utc::now().timestamp_millis();
    let target_record = SkillTargetRecord {
        id: uuid::Uuid::new_v4().to_string(),
        skill_id: skill_id.to_string(),
        tool: tool.to_string(),
        target_path: target.to_string_lossy().to_string(),
        mode: actual_mode.as_str().to_string(),
        status: "ok".to_string(),
        synced_at: Some(now),
        last_error: None,
    };

    store.insert_target(&target_record).map_err(AppError::db)?;
    Ok(())
}

/// When a skill is added to a scenario, broadcast the sync to all projects bound to that scenario.
pub fn sync_skill_to_bound_projects(
    store: &SkillStore,
    scenario_id: &str,
    skill_id: &str,
) -> Result<(), AppError> {
    let project_ids = store
        .get_scenario_project_ids(scenario_id)
        .map_err(AppError::db)?;
    if project_ids.is_empty() {
        return Ok(());
    }

    let Ok(Some(skill)) = store.get_skill_by_id(skill_id) else {
        return Ok(());
    };

    let configured_mode = store.get_setting("sync_mode").map_err(AppError::db)?;
    let adapters = enabled_installed_adapters_for_scenario_skill(store, scenario_id, skill_id)?;
    let source = PathBuf::from(&skill.central_path);
    let target_name = sync_engine::target_dir_name(&source, &skill.name);

    for project_id in &project_ids {
        let Ok(Some(project)) = store.get_project_by_id(project_id) else {
            continue;
        };

        for adapter in &adapters {
            let Some(target) = resolve_project_skill_target(&project, adapter, &target_name) else {
                continue; // tool not used in this project, skip
            };
            let mode = sync_engine::sync_mode_for_tool(&adapter.key, configured_mode.as_deref());
            match sync_engine::sync_skill(&source, &target, mode) {
                Ok(actual_mode) => {
                    let now = chrono::Utc::now().timestamp_millis();
                    let record = SkillTargetRecord {
                        id: uuid::Uuid::new_v4().to_string(),
                        skill_id: skill_id.to_string(),
                        tool: adapter.key.clone(),
                        target_path: target.to_string_lossy().to_string(),
                        mode: actual_mode.as_str().to_string(),
                        status: "ok".to_string(),
                        synced_at: Some(now),
                        last_error: None,
                    };
                    let _ = store.insert_target(&record);
                }
                Err(e) => {
                    log::warn!(
                        "Failed to sync skill {skill_id} to project {project_id}: {e}"
                    );
                }
            }
        }
    }
    Ok(())
}

/// When a skill is removed from a scenario, remove it from all projects bound to that scenario.
/// Skips removal if the project has another bound scenario that also contains this skill.
pub fn unsync_skill_from_bound_projects(
    store: &SkillStore,
    scenario_id: &str,
    skill_id: &str,
) -> Result<(), AppError> {
    let project_ids = store
        .get_scenario_project_ids(scenario_id)
        .map_err(AppError::db)?;
    if project_ids.is_empty() {
        return Ok(());
    }

    for project_id in &project_ids {
        // If another scenario bound to this project also contains this skill, keep it.
        let other_scenario_ids = store
            .get_project_scenario_ids(project_id)
            .map_err(AppError::db)?;
        let covered_by_other = other_scenario_ids
            .iter()
            .filter(|sid| sid.as_str() != scenario_id)
            .any(|sid| {
                store
                    .get_skill_ids_for_scenario(sid)
                    .unwrap_or_default()
                    .contains(&skill_id.to_string())
            });

        if covered_by_other {
            continue;
        }

        let Ok(Some(project)) = store.get_project_by_id(project_id) else {
            continue;
        };

        let targets = store.get_targets_for_skill(skill_id).unwrap_or_default();
        for target in &targets {
            let path = PathBuf::from(&target.target_path);
            // Only remove targets that live inside this project's directory.
            if path_is_under(&path, Path::new(&project.path)) {
                if let Err(e) = sync_engine::remove_target(&path) {
                    log::warn!("Failed to remove target {}: {e}", path.display());
                }
                let _ = store.delete_target(skill_id, &target.tool);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::path_is_under;
    use std::path::Path;

    #[test]
    fn path_is_under_direct_child() {
        assert!(path_is_under(
            Path::new("/a/proj/.claude/skills/foo"),
            Path::new("/a/proj")
        ));
    }

    #[test]
    fn path_is_under_does_not_match_sibling_with_same_prefix() {
        // /a/proj-legacy should NOT match parent /a/proj
        assert!(!path_is_under(
            Path::new("/a/proj-legacy/.claude/skills/foo"),
            Path::new("/a/proj")
        ));
    }

    #[test]
    fn path_is_under_exact_match_is_false() {
        // A path is not "under" itself
        assert!(!path_is_under(Path::new("/a/proj"), Path::new("/a/proj")));
    }

    #[test]
    fn path_is_under_unrelated_paths() {
        assert!(!path_is_under(
            Path::new("/b/other/skills/foo"),
            Path::new("/a/proj")
        ));
    }

    /// P0-8 regression: subscription symlinks inside a project point OUT of
    /// the project tree (into the central skills repo). A naive
    /// `canonicalize(child)` follows the symlink and decides the path is
    /// no longer under the project, which made unsync_scenario_from_project
    /// silently skip every cleanup. path_is_under must treat the symlink's
    /// own location — not its target — as "the path being tested".
    #[cfg(unix)]
    #[test]
    fn path_is_under_symlink_child_pointing_outside_is_still_under_parent() {
        use std::fs;
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        let outside = tmp.path().join("central");
        let real_skill = outside.join("html-ppt");
        fs::create_dir_all(project.join(".claude/skills")).unwrap();
        fs::create_dir_all(&real_skill).unwrap();

        // Write a symlink inside the project that targets a path OUTSIDE it.
        let link = project.join(".claude/skills/html-ppt");
        std::os::unix::fs::symlink(&real_skill, &link).unwrap();

        // The symlink itself lives under the project — that's what we're
        // asserting. If canonicalize(child) is applied blindly, this returns
        // false (because the link resolves to outside/central/html-ppt).
        assert!(
            path_is_under(&link, &project),
            "symlink lives inside project; its target destination is irrelevant to the containment check"
        );
    }
}
