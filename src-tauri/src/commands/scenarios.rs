use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::State;

use crate::core::{
    error::AppError,
    scenario_service,
    skill_store::{ScenarioRecord, SkillStore, SkillTargetRecord},
    sync_engine, sync_metadata,
};

/// Serializes lifecycle mutations to the project_scenarios relationship
/// (bind/unbind/delete-scenario/remove-project). Prevents races where two
/// concurrent unbinds both observe each other as "still covering" a skill
/// and neither ends up removing the orphaned symlink.
///
/// Held only for the duration of the blocking DB + filesystem work.
pub(crate) static PROJECT_SCENARIO_MUTATION_LOCK: Mutex<()> = Mutex::new(());

fn refresh_tray_menu_best_effort(app: &tauri::AppHandle) {
    if let Err(err) = crate::refresh_tray_menu(app) {
        log::warn!("Failed to refresh tray menu after scenario mutation: {err}");
    }
}

/// Sync a skill's files to all enabled tool adapter directories for the given scenario.
/// Only performs sync if the scenario is the currently active one.
pub(crate) fn sync_skill_to_active_scenario(
    store: &SkillStore,
    scenario_id: &str,
    skill_id: &str,
) -> Result<(), AppError> {
    scenario_service::sync_skill_to_active_scenario(store, scenario_id, skill_id)
}

/// Remove all skill symlinks for a scenario from a project (used when unbinding).
/// Skips skills that are also covered by another scenario bound to the same project.
pub(crate) fn unsync_scenario_from_project(
    store: &SkillStore,
    project_id: &str,
    scenario_id: &str,
) -> Result<(), AppError> {
    let project = store
        .get_project_by_id(project_id)
        .map_err(AppError::db)?
        .ok_or_else(|| AppError::not_found("Project not found"))?;

    // Collect skills covered by other scenarios bound to this project — don't remove those.
    let other_scenario_ids = store
        .get_project_scenario_ids(project_id)
        .map_err(AppError::db)?;
    let mut covered_skill_ids: std::collections::HashSet<String> = Default::default();
    for sid in other_scenario_ids.iter().filter(|sid| sid.as_str() != scenario_id) {
        let ids = store.get_skill_ids_for_scenario(sid).unwrap_or_default();
        covered_skill_ids.extend(ids);
    }

    let skill_ids = store
        .get_skill_ids_for_scenario(scenario_id)
        .map_err(AppError::db)?;

    for skill_id in &skill_ids {
        if covered_skill_ids.contains(skill_id) {
            continue;
        }

        let targets = store.get_targets_for_skill(skill_id).unwrap_or_default();
        for target in &targets {
            let path = PathBuf::from(&target.target_path);
            // Only remove targets that live inside this project's directory.
            if scenario_service::path_is_under(&path, Path::new(&project.path)) {
                if let Err(e) = sync_engine::remove_target(&path) {
                    log::warn!("Failed to remove target {}: {e}", path.display());
                }
                let _ = store.delete_target(skill_id, &target.tool);
            }
        }
    }

    Ok(())
}

/// Sync all skills in a scenario to a specific project's skill directory.
/// Called when binding a scenario to a project.
pub(crate) fn sync_scenario_to_project(
    store: &SkillStore,
    project_id: &str,
    scenario_id: &str,
) -> Result<(), AppError> {
    let configured_mode = store.get_setting("sync_mode").map_err(AppError::db)?;

    let project = store
        .get_project_by_id(project_id)
        .map_err(AppError::db)?
        .ok_or_else(|| AppError::not_found("Project not found"))?;

    let skill_ids = store
        .get_skill_ids_for_scenario(scenario_id)
        .map_err(AppError::db)?;

    for skill_id in skill_ids {
        let Ok(Some(skill)) = store.get_skill_by_id(&skill_id) else {
            continue;
        };

        let source = PathBuf::from(&skill.central_path);
        let target_name = sync_engine::target_dir_name(&source, &skill.name);

        let adapters = scenario_service::enabled_installed_adapters_for_scenario_skill(
            store,
            scenario_id,
            &skill_id,
        )?;

        for adapter in &adapters {
            let Some(target) = scenario_service::resolve_project_skill_target(
                &project,
                adapter,
                &target_name,
            ) else {
                continue; // tool not used in this project, skip
            };
            let mode = sync_engine::sync_mode_for_tool(&adapter.key, configured_mode.as_deref());

            match sync_engine::sync_skill(&source, &target, mode) {
                Ok(actual_mode) => {
                    let now = chrono::Utc::now().timestamp_millis();
                    let record = SkillTargetRecord {
                        id: uuid::Uuid::new_v4().to_string(),
                        skill_id: skill_id.clone(),
                        tool: adapter.key.clone(),
                        target_path: target.to_string_lossy().to_string(),
                        mode: actual_mode.as_str().to_string(),
                        status: "ok".to_string(),
                        synced_at: Some(now),
                        last_error: None,
                    };
                    if let Err(e) = store.insert_target(&record) {
                        log::warn!("Failed to insert sync target for skill {skill_id}: {e}");
                    }
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
#[derive(Debug, Serialize)]
pub struct ScenarioDto {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub sort_order: i32,
    pub skill_count: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[tauri::command]
pub async fn get_scenarios(
    store: State<'_, Arc<SkillStore>>,
) -> Result<Vec<ScenarioDto>, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let scenarios = store.get_all_scenarios().map_err(AppError::db)?;
        let mut result = Vec::new();
        for s in scenarios {
            let count = store.count_skills_for_scenario(&s.id).unwrap_or(0);
            result.push(ScenarioDto {
                id: s.id,
                name: s.name,
                description: s.description,
                icon: s.icon,
                sort_order: s.sort_order,
                skill_count: count,
                created_at: s.created_at,
                updated_at: s.updated_at,
            });
        }
        Ok(result)
    })
    .await?
}

#[tauri::command]
pub async fn get_active_scenario(
    store: State<'_, Arc<SkillStore>>,
) -> Result<Option<ScenarioDto>, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let active_id = store.get_active_scenario_id().map_err(AppError::db)?;

        if let Some(id) = active_id {
            let scenarios = store.get_all_scenarios().map_err(AppError::db)?;
            if let Some(s) = scenarios.into_iter().find(|s| s.id == id) {
                let count = store.count_skills_for_scenario(&s.id).unwrap_or(0);
                return Ok(Some(ScenarioDto {
                    id: s.id,
                    name: s.name,
                    description: s.description,
                    icon: s.icon,
                    sort_order: s.sort_order,
                    skill_count: count,
                    created_at: s.created_at,
                    updated_at: s.updated_at,
                }));
            }
        }
        Ok(None)
    })
    .await?
}

#[tauri::command]
pub async fn get_project_scenarios(
    store: State<'_, Arc<SkillStore>>,
    project_id: String,
) -> Result<Vec<ScenarioDto>, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let scenarios = store.get_project_scenarios(&project_id).map_err(AppError::db)?;
        let mut result = Vec::new();
        for s in scenarios {
            let count = store.count_skills_for_scenario(&s.id).unwrap_or(0);
            result.push(ScenarioDto {
                id: s.id,
                name: s.name,
                description: s.description,
                icon: s.icon,
                sort_order: s.sort_order,
                skill_count: count,
                created_at: s.created_at,
                updated_at: s.updated_at,
            });
        }
        Ok(result)
    })
    .await?
}

#[tauri::command]
pub async fn create_scenario(
    app: tauri::AppHandle,
    name: String,
    description: Option<String>,
    icon: Option<String>,
    store: State<'_, Arc<SkillStore>>,
) -> Result<ScenarioDto, AppError> {
    let store = store.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let now = chrono::Utc::now().timestamp_millis();
        let id = uuid::Uuid::new_v4().to_string();
        let previous_active_id = store.get_active_scenario_id().map_err(AppError::db)?;

        let record = ScenarioRecord {
            id: id.clone(),
            name: name.clone(),
            description: description.clone(),
            icon: icon.clone(),
            sort_order: 999,
            created_at: now,
            updated_at: now,
        };

        sync_metadata::with_repo_lock("create scenario", || {
            store.insert_scenario(&record)?;
            sync_metadata::write_all_from_db_unlocked(&store)
        })
        .map_err(AppError::db)?;

        if let Some(previous_id) = previous_active_id.as_deref() {
            unsync_scenario_skills(&store, previous_id)?;
        }
        store.set_active_scenario(&id).map_err(AppError::db)?;

        Ok(ScenarioDto {
            id,
            name,
            description,
            icon,
            sort_order: 999,
            skill_count: 0,
            created_at: now,
            updated_at: now,
        })
    })
    .await?;
    if result.is_ok() {
        refresh_tray_menu_best_effort(&app);
    }
    result
}

#[tauri::command]
pub async fn update_scenario(
    app: tauri::AppHandle,
    id: String,
    name: String,
    description: Option<String>,
    icon: Option<String>,
    store: State<'_, Arc<SkillStore>>,
) -> Result<(), AppError> {
    let store = store.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        sync_metadata::with_repo_lock("update scenario", || {
            store.update_scenario(&id, &name, description.as_deref(), icon.as_deref())?;
            sync_metadata::write_all_from_db_unlocked(&store)
        })
        .map_err(AppError::db)
    })
    .await?;
    if result.is_ok() {
        refresh_tray_menu_best_effort(&app);
    }
    result
}

#[tauri::command]
pub async fn delete_scenario(
    app: tauri::AppHandle,
    id: String,
    store: State<'_, Arc<SkillStore>>,
) -> Result<(), AppError> {
    let store = store.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        // Serialize project-scenario lifecycle mutations to avoid races with
        // bind/unbind/delete across scenarios touching the same project.
        let _guard = PROJECT_SCENARIO_MUTATION_LOCK.lock().unwrap_or_else(|p| p.into_inner());

        let was_active = store
            .get_active_scenario_id()
            .map_err(AppError::db)?
            .as_deref()
            == Some(id.as_str());

        if was_active {
            unsync_scenario_skills(&store, &id)?;
        }

        // P0-3: clean up skill symlinks in every project that subscribed to this scenario
        // BEFORE the DB row (and its CASCADE) wipes the project_scenarios relationship.
        // Without this, deleting a subscribed scenario leaves orphan symlinks in project dirs
        // with no DB trace.
        let subscribed_project_ids = store
            .get_scenario_project_ids(&id)
            .map_err(AppError::db)?;
        for project_id in &subscribed_project_ids {
            if let Err(e) = unsync_scenario_from_project(&store, project_id, &id) {
                // Log and continue — best-effort cleanup; the DB CASCADE will still drop the
                // subscription row, so leftover files are acceptable over aborting the delete.
                log::warn!(
                    "Failed to clean up scenario {id} symlinks in project {project_id}: {e}"
                );
            }
        }

        sync_metadata::with_repo_lock("delete scenario", || {
            store.delete_scenario(&id)?;
            sync_metadata::write_all_from_db_unlocked(&store)
        })
        .map_err(AppError::db)?;

        if was_active {
            let remaining = store.get_all_scenarios().map_err(AppError::db)?;
            if let Some(first) = remaining.first() {
                store.set_active_scenario(&first.id).map_err(AppError::db)?;
                sync_scenario_skills(&store, &first.id)?;
            } else {
                store.clear_active_scenario().map_err(AppError::db)?;
            }
        }

        Ok(())
    })
    .await?;
    if result.is_ok() {
        refresh_tray_menu_best_effort(&app);
    }
    result
}

/// Apply a scenario to the default targets (all enabled agent globals).
///
/// This is the explicit user-initiated action introduced in v1.16. It performs
/// the same disk-writing work as the legacy [`switch_scenario`] command but is
/// only invoked when the user clicks "Apply to Default" — sidebar/command-palette
/// scenario clicks no longer call this.
#[tauri::command]
pub async fn apply_scenario_to_default(
    app: tauri::AppHandle,
    id: String,
    store: State<'_, Arc<SkillStore>>,
) -> Result<(), AppError> {
    apply_scenario_to_default_impl(app, id, store.inner().clone()).await
}

/// Legacy command kept for the tray menu and backward compatibility. Frontend
/// callers should use [`apply_scenario_to_default`] instead.
#[tauri::command]
pub async fn switch_scenario(
    app: tauri::AppHandle,
    id: String,
    store: State<'_, Arc<SkillStore>>,
) -> Result<(), AppError> {
    apply_scenario_to_default_impl(app, id, store.inner().clone()).await
}

async fn apply_scenario_to_default_impl(
    app: tauri::AppHandle,
    id: String,
    store: Arc<SkillStore>,
) -> Result<(), AppError> {
    let result = tauri::async_runtime::spawn_blocking(move || {
        scenario_service::apply_scenario_to_default(&store, &id)
    })
    .await?;
    if result.is_ok() {
        refresh_tray_menu_best_effort(&app);
    }
    result
}

#[tauri::command]
pub async fn add_skill_to_scenario(
    app: tauri::AppHandle,
    skill_id: String,
    scenario_id: String,
    store: State<'_, Arc<SkillStore>>,
) -> Result<(), AppError> {
    let store = store.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        sync_metadata::with_repo_lock("add skill to scenario", || {
            store.add_skill_to_scenario(&scenario_id, &skill_id)?;
            sync_metadata::write_all_from_db_unlocked(&store)
        })
        .map_err(AppError::db)?;

        sync_skill_to_active_scenario(&store, &scenario_id, &skill_id)?;

        // Broadcast to all projects that have subscribed to this scenario.
        scenario_service::sync_skill_to_bound_projects(&store, &scenario_id, &skill_id)
            .unwrap_or_else(|e| log::warn!("Failed to broadcast skill add to bound projects: {e}"));

        Ok(())
    })
    .await?;
    if result.is_ok() {
        refresh_tray_menu_best_effort(&app);
    }
    result
}

#[tauri::command]
pub async fn remove_skill_from_scenario(
    app: tauri::AppHandle,
    skill_id: String,
    scenario_id: String,
    store: State<'_, Arc<SkillStore>>,
) -> Result<(), AppError> {
    let store = store.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        sync_metadata::with_repo_lock("remove skill from scenario", || {
            store.remove_skill_from_scenario(&scenario_id, &skill_id)?;
            sync_metadata::write_all_from_db_unlocked(&store)
        })
        .map_err(AppError::db)?;

        // If this is the active scenario, unsync the skill from global (non-project) targets only.
        // Project-level targets are handled separately by unsync_skill_from_bound_projects below.
        if let Ok(Some(active_id)) = store.get_active_scenario_id() {
            if active_id == scenario_id {
                // Collect all known project paths so we can exclude project-level targets.
                let project_paths: Vec<std::path::PathBuf> = store
                    .get_all_projects()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|p| std::path::PathBuf::from(p.path))
                    .collect();

                let targets = store.get_targets_for_skill(&skill_id).unwrap_or_default();
                for target in &targets {
                    let path = PathBuf::from(&target.target_path);
                    // Only remove global targets — skip anything living inside a project dir.
                    let is_project_target = project_paths.iter().any(|proj| {
                        scenario_service::path_is_under(&path, proj)
                    });
                    if is_project_target {
                        continue;
                    }
                    if let Err(e) = sync_engine::remove_target(&path) {
                        log::warn!("Failed to remove sync target {}: {e}", path.display());
                    }
                    if let Err(e) = store.delete_target(&skill_id, &target.tool) {
                        log::warn!(
                            "Failed to delete target record for skill {skill_id}, tool {}: {e}",
                            target.tool
                        );
                    }
                }
            }
        }

        // Broadcast removal to all projects that have subscribed to this scenario.
        scenario_service::unsync_skill_from_bound_projects(&store, &scenario_id, &skill_id)
            .unwrap_or_else(|e| log::warn!("Failed to broadcast skill remove to bound projects: {e}"));

        Ok(())
    })
    .await?;
    if result.is_ok() {
        refresh_tray_menu_best_effort(&app);
    }
    result
}

#[tauri::command]
pub async fn reorder_scenarios(
    app: tauri::AppHandle,
    ids: Vec<String>,
    store: State<'_, Arc<SkillStore>>,
) -> Result<(), AppError> {
    let store = store.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        sync_metadata::with_repo_lock("reorder scenarios", || {
            store.reorder_scenarios(&ids)?;
            sync_metadata::write_all_from_db_unlocked(&store)
        })
        .map_err(AppError::db)
    })
    .await?;
    if result.is_ok() {
        refresh_tray_menu_best_effort(&app);
    }
    result
}

#[tauri::command]
pub async fn get_scenario_skill_order(
    scenario_id: String,
    store: State<'_, Arc<SkillStore>>,
) -> Result<Vec<String>, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        store
            .get_skill_ids_for_scenario(&scenario_id)
            .map_err(AppError::db)
    })
    .await?
}

#[tauri::command]
pub async fn reorder_scenario_skills(
    scenario_id: String,
    skill_ids: Vec<String>,
    store: State<'_, Arc<SkillStore>>,
) -> Result<(), AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        sync_metadata::with_repo_lock("reorder scenario skills", || {
            store.reorder_scenario_skills(&scenario_id, &skill_ids)?;
            sync_metadata::write_all_from_db_unlocked(&store)
        })
        .map_err(AppError::db)
    })
    .await?
}

// ── Internal helpers ──

pub(crate) fn sync_scenario_skills(store: &SkillStore, scenario_id: &str) -> Result<(), AppError> {
    scenario_service::sync_scenario_skills(store, scenario_id)
}

pub(crate) fn unsync_scenario_skills(
    store: &SkillStore,
    scenario_id: &str,
) -> Result<(), AppError> {
    scenario_service::unsync_scenario_skills(store, scenario_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::scenario_service::{
        collect_scenario_sync_targets, sync_desired_targets, unsync_obsolete_scenario_targets,
    };
    use crate::core::skill_store::SkillRecord;
    use crate::core::tool_adapters::{self, CustomToolDef};
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    use tempfile::tempdir;

    fn sample_skill(id: &str, name: &str, central_path: &std::path::Path) -> SkillRecord {
        SkillRecord {
            id: id.to_string(),
            name: name.to_string(),
            description: None,
            source_type: "import".to_string(),
            source_ref: Some(central_path.to_string_lossy().to_string()),
            source_ref_resolved: None,
            source_subpath: None,
            source_branch: None,
            source_revision: None,
            remote_revision: None,
            central_path: central_path.to_string_lossy().to_string(),
            content_hash: None,
            enabled: true,
            created_at: 1,
            updated_at: 1,
            status: "ok".to_string(),
            update_status: "local_only".to_string(),
            last_checked_at: None,
            last_check_error: None,
        }
    }

    fn sample_scenario(id: &str, name: &str) -> ScenarioRecord {
        ScenarioRecord {
            id: id.to_string(),
            name: name.to_string(),
            description: None,
            icon: None,
            sort_order: 0,
            created_at: 1,
            updated_at: 1,
        }
    }

    fn write_skill_dir(base: &std::path::Path, name: &str) -> PathBuf {
        let dir = base.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\n---\n")).unwrap();
        dir
    }

    fn configure_single_custom_tool(store: &SkillStore, target_base: &std::path::Path) {
        let custom_tools = vec![CustomToolDef {
            key: "test_agent".to_string(),
            display_name: "Test Agent".to_string(),
            skills_dir: target_base.to_string_lossy().to_string(),
            project_relative_skills_dir: None,
        }];
        store
            .set_setting(
                "custom_tools",
                &serde_json::to_string(&custom_tools).unwrap(),
            )
            .unwrap();
        let disabled_builtin_tools: Vec<String> = tool_adapters::default_tool_adapters()
            .into_iter()
            .map(|adapter| adapter.key)
            .collect();
        store
            .set_setting(
                "disabled_tools",
                &serde_json::to_string(&disabled_builtin_tools).unwrap(),
            )
            .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn switching_scenarios_keeps_overlapping_skill_target() {
        let tmp = tempdir().unwrap();
        let store = SkillStore::new(&tmp.path().join("test.db")).unwrap();
        let source_base = tmp.path().join("central");
        let target_base = tmp.path().join("agent-skills");
        fs::create_dir_all(&source_base).unwrap();
        fs::create_dir_all(&target_base).unwrap();

        configure_single_custom_tool(&store, &target_base);

        store
            .insert_scenario(&sample_scenario("old", "Old"))
            .unwrap();
        store
            .insert_scenario(&sample_scenario("new", "New"))
            .unwrap();

        let shared_dir = write_skill_dir(&source_base, "shared");
        let old_only_dir = write_skill_dir(&source_base, "old-only");
        let new_only_dir = write_skill_dir(&source_base, "new-only");
        store
            .insert_skill(&sample_skill("shared", "shared", &shared_dir))
            .unwrap();
        store
            .insert_skill(&sample_skill("old-only", "old-only", &old_only_dir))
            .unwrap();
        store
            .insert_skill(&sample_skill("new-only", "new-only", &new_only_dir))
            .unwrap();

        store.add_skill_to_scenario("old", "shared").unwrap();
        store.add_skill_to_scenario("old", "old-only").unwrap();
        store.add_skill_to_scenario("new", "shared").unwrap();
        store.add_skill_to_scenario("new", "new-only").unwrap();

        store.set_active_scenario("old").unwrap();
        sync_scenario_skills(&store, "old").unwrap();

        let shared_target = target_base.join("shared");
        let old_only_target = target_base.join("old-only");
        let new_only_target = target_base.join("new-only");
        assert_eq!(fs::read_link(&shared_target).unwrap(), shared_dir);
        assert!(old_only_target.is_symlink());
        let shared_inode_before = fs::symlink_metadata(&shared_target).unwrap().ino();

        let desired_targets = collect_scenario_sync_targets(&store, "new").unwrap();
        unsync_obsolete_scenario_targets(&store, "old", &desired_targets).unwrap();
        store.set_active_scenario("new").unwrap();
        sync_desired_targets(&store, &desired_targets).unwrap();

        assert_eq!(fs::read_link(&shared_target).unwrap(), shared_dir);
        assert_eq!(
            fs::symlink_metadata(&shared_target).unwrap().ino(),
            shared_inode_before
        );
        assert!(!old_only_target.exists());
        assert_eq!(fs::read_link(&new_only_target).unwrap(), new_only_dir);

        let targets = store.get_all_targets().unwrap();
        assert_eq!(targets.len(), 2);
        assert!(targets
            .iter()
            .any(|target| target.skill_id == "shared" && target.tool == "test_agent"));
        assert!(targets
            .iter()
            .any(|target| target.skill_id == "new-only" && target.tool == "test_agent"));
    }

    #[test]
    fn scenario_sync_keeps_duplicate_skill_names_separate() {
        let tmp = tempdir().unwrap();
        let store = SkillStore::new(&tmp.path().join("test.db")).unwrap();
        let source_base = tmp.path().join("central");
        let target_base = tmp.path().join("agent-skills");
        fs::create_dir_all(&source_base).unwrap();
        fs::create_dir_all(&target_base).unwrap();
        configure_single_custom_tool(&store, &target_base);
        store.set_setting("sync_mode", "copy").unwrap();

        store
            .insert_scenario(&sample_scenario("active", "Active"))
            .unwrap();

        let first_dir = write_skill_dir(&source_base, "skill123");
        let second_dir = write_skill_dir(&source_base, "skill123-2");
        fs::write(first_dir.join("unique.txt"), "first").unwrap();
        fs::write(second_dir.join("unique.txt"), "second").unwrap();

        store
            .insert_skill(&sample_skill("first", "skill123", &first_dir))
            .unwrap();
        store
            .insert_skill(&sample_skill("second", "skill123", &second_dir))
            .unwrap();
        store.add_skill_to_scenario("active", "first").unwrap();
        store.add_skill_to_scenario("active", "second").unwrap();

        sync_scenario_skills(&store, "active").unwrap();

        assert_eq!(
            fs::read_to_string(target_base.join("skill123/unique.txt")).unwrap(),
            "first"
        );
        assert_eq!(
            fs::read_to_string(target_base.join("skill123-2/unique.txt")).unwrap(),
            "second"
        );
        let targets = store.get_all_targets().unwrap();
        assert!(targets.iter().any(|target| {
            target.skill_id == "first" && target.target_path.ends_with("skill123")
        }));
        assert!(targets.iter().any(|target| {
            target.skill_id == "second" && target.target_path.ends_with("skill123-2")
        }));
    }

    // ──────────────────────────────────────────────────────────────────
    // Regression tests for the project-scenario subscription lifecycle.
    // Added 2026-05-08 to lock down P0 bugs fixed in this PR.
    // ──────────────────────────────────────────────────────────────────

    use crate::core::skill_store::ProjectRecord;
    use crate::core::scenario_service;

    /// Build a non-linked project at `project_path`, creating the `.claude/`
    /// detect dir so resolve_project_skill_target returns a real target path.
    fn insert_project_with_claude(
        store: &SkillStore,
        id: &str,
        project_path: &std::path::Path,
    ) -> ProjectRecord {
        fs::create_dir_all(project_path.join(".claude")).unwrap();
        let record = ProjectRecord {
            id: id.to_string(),
            name: id.to_string(),
            path: project_path.to_string_lossy().to_string(),
            workspace_type: "project".to_string(),
            linked_agent_key: None,
            linked_agent_name: None,
            disabled_path: None,
            sort_order: 0,
            created_at: 1,
            updated_at: 1,
        };
        store.insert_project(&record).unwrap();
        record
    }

    /// P0-1 regression: removing a skill from the active scenario must NOT
    /// delete symlinks that live inside subscribed project directories. Those
    /// are owned by the subscription and get cleaned up via the bound-projects
    /// broadcast, not the active-scenario global sweep.
    #[cfg(unix)]
    #[test]
    fn remove_skill_from_active_scenario_does_not_delete_project_symlinks() {
        let tmp = tempdir().unwrap();
        let store = SkillStore::new(&tmp.path().join("test.db")).unwrap();
        let source_base = tmp.path().join("central");
        let project_path = tmp.path().join("proj");
        fs::create_dir_all(&source_base).unwrap();

        // Use the custom-tool escape hatch to keep the test hermetic — point
        // the global skills_dir at an isolated temp location so built-in
        // adapters don't probe the real $HOME.
        let global_target = tmp.path().join("global-agent");
        fs::create_dir_all(&global_target).unwrap();
        configure_single_custom_tool(&store, &global_target);

        store
            .insert_scenario(&sample_scenario("s1", "S1"))
            .unwrap();
        let skill_dir = write_skill_dir(&source_base, "my-skill");
        store
            .insert_skill(&sample_skill("sk", "my-skill", &skill_dir))
            .unwrap();
        store.add_skill_to_scenario("s1", "sk").unwrap();
        store.set_active_scenario("s1").unwrap();

        // Project has a project-level target directly inserted to mimic
        // what bind_scenario_to_project would have written.
        insert_project_with_claude(&store, "p1", &project_path);
        let project_skill_target = project_path.join(".claude/skills/my-skill");
        fs::create_dir_all(project_skill_target.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&skill_dir, &project_skill_target).unwrap();
        store
            .insert_target(&SkillTargetRecord {
                id: "t-project".to_string(),
                skill_id: "sk".to_string(),
                tool: "claude_code".to_string(),
                target_path: project_skill_target.to_string_lossy().to_string(),
                mode: "symlink".to_string(),
                status: "ok".to_string(),
                synced_at: Some(1),
                last_error: None,
            })
            .unwrap();

        // Also put a non-project global target to make sure global IS cleaned.
        let global_skill_target = global_target.join("my-skill");
        std::os::unix::fs::symlink(&skill_dir, &global_skill_target).unwrap();
        store
            .insert_target(&SkillTargetRecord {
                id: "t-global".to_string(),
                skill_id: "sk".to_string(),
                tool: "test_agent".to_string(),
                target_path: global_skill_target.to_string_lossy().to_string(),
                mode: "symlink".to_string(),
                status: "ok".to_string(),
                synced_at: Some(1),
                last_error: None,
            })
            .unwrap();

        // Subscribe project p1 to scenario s1 so the project target is known
        // to the subscription system.
        store.bind_scenario_to_project("p1", "s1").unwrap();

        // Exercise the active-scenario global-sweep branch from
        // remove_skill_from_scenario: collect project paths, exclude any target
        // that is under any project path, delete the rest.
        let project_paths: Vec<std::path::PathBuf> = store
            .get_all_projects()
            .unwrap()
            .into_iter()
            .map(|p| std::path::PathBuf::from(p.path))
            .collect();
        let targets = store.get_targets_for_skill("sk").unwrap();
        for t in &targets {
            let path = PathBuf::from(&t.target_path);
            let is_project_target = project_paths
                .iter()
                .any(|proj| scenario_service::path_is_under(&path, proj));
            if is_project_target {
                continue;
            }
            sync_engine::remove_target(&path).unwrap();
            store.delete_target("sk", &t.tool).unwrap();
        }

        // Assertions: project target must survive, global target must be gone.
        assert!(
            project_skill_target.is_symlink(),
            "project symlink must be preserved"
        );
        assert!(!global_skill_target.exists(), "global symlink must be cleaned");
    }

    /// P0-2 regression: cleanup must use path-component comparison, not byte
    /// prefix. A project named `/tmp/proj-legacy` must NOT be affected when
    /// we clean up artifacts for `/tmp/proj`.
    #[cfg(unix)]
    #[test]
    fn unbind_scenario_does_not_affect_sibling_project_with_same_prefix() {
        let tmp = tempdir().unwrap();
        let store = SkillStore::new(&tmp.path().join("test.db")).unwrap();
        let source_base = tmp.path().join("central");
        fs::create_dir_all(&source_base).unwrap();

        let target_base = tmp.path().join("agent-skills");
        fs::create_dir_all(&target_base).unwrap();
        configure_single_custom_tool(&store, &target_base);

        // Two project directories whose names share a common prefix.
        let proj = tmp.path().join("proj");
        let proj_legacy = tmp.path().join("proj-legacy");
        fs::create_dir_all(proj.join(".claude")).unwrap();
        fs::create_dir_all(proj_legacy.join(".claude")).unwrap();

        store
            .insert_project(&ProjectRecord {
                id: "proj".to_string(),
                name: "proj".to_string(),
                path: proj.to_string_lossy().to_string(),
                workspace_type: "project".to_string(),
                linked_agent_key: None,
                linked_agent_name: None,
                disabled_path: None,
                sort_order: 0,
                created_at: 1,
                updated_at: 1,
            })
            .unwrap();

        // Pretend a skill target lives inside proj-legacy. The cleanup routine
        // for `proj` must NOT treat it as being under `proj`.
        let legacy_target = proj_legacy.join(".claude").join("skills").join("foo");
        let under = scenario_service::path_is_under(&legacy_target, &proj);
        assert!(
            !under,
            "path_is_under must not match sibling project with shared name prefix"
        );

        // Conversely, a real descendant of `proj` must match.
        let own_target = proj.join(".claude").join("skills").join("foo");
        assert!(
            scenario_service::path_is_under(&own_target, &proj),
            "path_is_under must match genuine descendants"
        );
    }

    /// P0-3 regression: deleting a scenario must remove the symlinks that
    /// subscribed projects hold for that scenario's skills, even though the
    /// scenario row (and its project_scenarios rows via CASCADE) is about to
    /// disappear.
    #[cfg(unix)]
    #[test]
    fn delete_scenario_cleans_up_subscribed_project_symlinks() {
        let tmp = tempdir().unwrap();
        let store = SkillStore::new(&tmp.path().join("test.db")).unwrap();
        let source_base = tmp.path().join("central");
        let project_path = tmp.path().join("proj");
        fs::create_dir_all(&source_base).unwrap();

        // No global adapters — we only care about project-side cleanup here.
        let target_base = tmp.path().join("agent-skills-unused");
        fs::create_dir_all(&target_base).unwrap();
        configure_single_custom_tool(&store, &target_base);

        store
            .insert_scenario(&sample_scenario("s1", "S1"))
            .unwrap();
        let skill_dir = write_skill_dir(&source_base, "my-skill");
        store
            .insert_skill(&sample_skill("sk", "my-skill", &skill_dir))
            .unwrap();
        store.add_skill_to_scenario("s1", "sk").unwrap();

        insert_project_with_claude(&store, "p1", &project_path);
        let project_skill_target = project_path.join(".claude/skills/my-skill");
        fs::create_dir_all(project_skill_target.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&skill_dir, &project_skill_target).unwrap();
        store
            .insert_target(&SkillTargetRecord {
                id: "t1".to_string(),
                skill_id: "sk".to_string(),
                tool: "claude_code".to_string(),
                target_path: project_skill_target.to_string_lossy().to_string(),
                mode: "symlink".to_string(),
                status: "ok".to_string(),
                synced_at: Some(1),
                last_error: None,
            })
            .unwrap();
        store.bind_scenario_to_project("p1", "s1").unwrap();

        assert!(project_skill_target.is_symlink(), "precondition: symlink should exist");

        // Simulate the P0-3 fix: iterate subscribed projects and unsync before
        // deleting the scenario.
        let subscribed = store.get_scenario_project_ids("s1").unwrap();
        for project_id in &subscribed {
            unsync_scenario_from_project(&store, project_id, "s1").unwrap();
        }
        store.delete_scenario("s1").unwrap();

        assert!(
            !project_skill_target.exists(),
            "subscribed-project symlink must be removed when its source scenario is deleted"
        );
        assert!(
            skill_dir.exists(),
            "central skill dir must be untouched — we only removed the symlink"
        );
    }

    /// P0-4 regression: removing a project must clean up the skill symlinks
    /// that its subscriptions put into the project directory. Central-repo
    /// files must remain untouched.
    #[cfg(unix)]
    #[test]
    fn remove_project_cleans_up_subscription_symlinks() {
        let tmp = tempdir().unwrap();
        let store = SkillStore::new(&tmp.path().join("test.db")).unwrap();
        let source_base = tmp.path().join("central");
        let project_path = tmp.path().join("proj");
        fs::create_dir_all(&source_base).unwrap();

        let target_base = tmp.path().join("agent-skills-unused");
        fs::create_dir_all(&target_base).unwrap();
        configure_single_custom_tool(&store, &target_base);

        store
            .insert_scenario(&sample_scenario("s1", "S1"))
            .unwrap();
        let skill_dir = write_skill_dir(&source_base, "my-skill");
        store
            .insert_skill(&sample_skill("sk", "my-skill", &skill_dir))
            .unwrap();
        store.add_skill_to_scenario("s1", "sk").unwrap();

        insert_project_with_claude(&store, "p1", &project_path);
        let project_skill_target = project_path.join(".claude/skills/my-skill");
        fs::create_dir_all(project_skill_target.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&skill_dir, &project_skill_target).unwrap();
        store
            .insert_target(&SkillTargetRecord {
                id: "t1".to_string(),
                skill_id: "sk".to_string(),
                tool: "claude_code".to_string(),
                target_path: project_skill_target.to_string_lossy().to_string(),
                mode: "symlink".to_string(),
                status: "ok".to_string(),
                synced_at: Some(1),
                last_error: None,
            })
            .unwrap();
        store.bind_scenario_to_project("p1", "s1").unwrap();

        assert!(project_skill_target.is_symlink(), "precondition: symlink should exist");

        // Simulate the P0-4 fix: iterate subscribed scenarios and unsync
        // before dropping the project row.
        let subscribed = store.get_project_scenario_ids("p1").unwrap();
        for scenario_id in &subscribed {
            unsync_scenario_from_project(&store, "p1", scenario_id).unwrap();
        }
        store.delete_project("p1").unwrap();

        assert!(
            !project_skill_target.exists(),
            "subscription-owned symlink must be removed when project is removed"
        );
        assert!(
            skill_dir.exists(),
            "central skill source must remain — removing a project only affects its own dir"
        );
    }

    /// P0-5 regression: app startup runs `unsync_scenario_skills` on the
    /// previously-active scenario when switching to the configured default.
    /// That sweep must NOT touch symlinks in subscribed project directories,
    /// otherwise users see "bound" scenes in the UI but missing skill files
    /// on disk after every restart.
    ///
    /// Reproduction without the fix:
    ///   1. Bind project P to scenario S
    ///   2. App writes ~/work/proj/.claude/skills/foo
    ///   3. Restart app — startup picks default scenario, calls
    ///      `unsync_scenario_skills(S)` which deletes ALL targets of S's skills
    ///   4. ~/work/proj/.claude/skills/foo is gone, DB still says P is bound to S
    #[cfg(unix)]
    #[test]
    fn unsync_scenario_skills_preserves_subscribed_project_symlinks() {
        let tmp = tempdir().unwrap();
        let store = SkillStore::new(&tmp.path().join("test.db")).unwrap();
        let source_base = tmp.path().join("central");
        let project_path = tmp.path().join("proj");
        fs::create_dir_all(&source_base).unwrap();

        let global_target = tmp.path().join("global-agent");
        fs::create_dir_all(&global_target).unwrap();
        configure_single_custom_tool(&store, &global_target);

        store
            .insert_scenario(&sample_scenario("dev", "Dev"))
            .unwrap();
        let skill_dir = write_skill_dir(&source_base, "agent-manage-build");
        store
            .insert_skill(&sample_skill("sk", "agent-manage-build", &skill_dir))
            .unwrap();
        store.add_skill_to_scenario("dev", "sk").unwrap();

        // Project subscribes to the dev scenario.
        insert_project_with_claude(&store, "p1", &project_path);
        let project_skill_target = project_path.join(".claude/skills/agent-manage-build");
        fs::create_dir_all(project_skill_target.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&skill_dir, &project_skill_target).unwrap();
        store
            .insert_target(&SkillTargetRecord {
                id: "t-project".to_string(),
                skill_id: "sk".to_string(),
                tool: "claude_code".to_string(),
                target_path: project_skill_target.to_string_lossy().to_string(),
                mode: "symlink".to_string(),
                status: "ok".to_string(),
                synced_at: Some(1),
                last_error: None,
            })
            .unwrap();
        store.bind_scenario_to_project("p1", "dev").unwrap();

        // Also a global target so we verify global IS still cleaned.
        let global_skill_target = global_target.join("agent-manage-build");
        std::os::unix::fs::symlink(&skill_dir, &global_skill_target).unwrap();
        store
            .insert_target(&SkillTargetRecord {
                id: "t-global".to_string(),
                skill_id: "sk".to_string(),
                tool: "test_agent".to_string(),
                target_path: global_skill_target.to_string_lossy().to_string(),
                mode: "symlink".to_string(),
                status: "ok".to_string(),
                synced_at: Some(1),
                last_error: None,
            })
            .unwrap();

        // Simulate the startup path: switch active scenario away from "dev",
        // which calls unsync_scenario_skills("dev").
        scenario_service::unsync_scenario_skills(&store, "dev").unwrap();

        // The subscribed project symlink MUST survive — its lifecycle is owned
        // by the subscription, not by the active-scenario sweep.
        assert!(
            project_skill_target.is_symlink(),
            "subscribed project symlink must survive startup unsync sweep"
        );
        // The DB target row for the project must also survive.
        let remaining: Vec<_> = store
            .get_targets_for_skill("sk")
            .unwrap()
            .into_iter()
            .filter(|t| t.target_path.contains("proj/.claude"))
            .collect();
        assert_eq!(
            remaining.len(),
            1,
            "project-level skill_targets row must be preserved"
        );

        // The global target SHOULD be cleaned — that's the whole point of
        // unsync_scenario_skills.
        assert!(
            !global_skill_target.exists(),
            "global symlink must still be cleaned by unsync_scenario_skills"
        );
    }

    /// P0-5 regression for the "Apply to Default" path: switching the active
    /// scenario via apply_scenario_to_default → unsync_obsolete_scenario_targets
    /// must also leave subscribed project symlinks alone.
    #[cfg(unix)]
    #[test]
    fn unsync_obsolete_scenario_targets_preserves_subscribed_project_symlinks() {
        let tmp = tempdir().unwrap();
        let store = SkillStore::new(&tmp.path().join("test.db")).unwrap();
        let source_base = tmp.path().join("central");
        let project_path = tmp.path().join("proj");
        fs::create_dir_all(&source_base).unwrap();

        let global_target = tmp.path().join("global-agent");
        fs::create_dir_all(&global_target).unwrap();
        configure_single_custom_tool(&store, &global_target);

        // Two scenarios — switch from "dev" to "release".
        store
            .insert_scenario(&sample_scenario("dev", "Dev"))
            .unwrap();
        store
            .insert_scenario(&sample_scenario("release", "Release"))
            .unwrap();

        let skill_dir = write_skill_dir(&source_base, "swagger-sync");
        store
            .insert_skill(&sample_skill("sk", "swagger-sync", &skill_dir))
            .unwrap();
        store.add_skill_to_scenario("dev", "sk").unwrap();

        insert_project_with_claude(&store, "p1", &project_path);
        let project_skill_target = project_path.join(".claude/skills/swagger-sync");
        fs::create_dir_all(project_skill_target.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&skill_dir, &project_skill_target).unwrap();
        store
            .insert_target(&SkillTargetRecord {
                id: "t-project".to_string(),
                skill_id: "sk".to_string(),
                tool: "claude_code".to_string(),
                target_path: project_skill_target.to_string_lossy().to_string(),
                mode: "symlink".to_string(),
                status: "ok".to_string(),
                synced_at: Some(1),
                last_error: None,
            })
            .unwrap();
        store.bind_scenario_to_project("p1", "dev").unwrap();
        store.set_active_scenario("dev").unwrap();

        // "release" doesn't contain "sk", so its desired_targets is empty.
        // Without the fix, unsync_obsolete_scenario_targets would wipe ALL of
        // dev's targets, including the subscribed project's symlink.
        let desired_targets =
            collect_scenario_sync_targets(&store, "release").unwrap();
        scenario_service::unsync_obsolete_scenario_targets(&store, "dev", &desired_targets)
            .unwrap();

        assert!(
            project_skill_target.is_symlink(),
            "subscribed project symlink must survive 'Apply to Default' switch"
        );
    }
}
