use crate::cache::{CachedBranch, ShadowBranch, ShadowMergeState, StackCache};
use crate::commands::navigate::get_branches_and_parent_map;
use crate::git::{GitRepo, ShadowMergeConflict};
use crate::utils;
use anyhow::{anyhow, Result};

pub async fn run(branch: Option<&str>, continue_merge: bool) -> Result<()> {
    let git = GitRepo::open(".")?;

    if continue_merge {
        return do_continue(&git).await;
    }

    let branch =
        branch.ok_or_else(|| anyhow!("Branch name is required (unless using --continue)"))?;

    log::debug!("include: adding '{}' as a merge source", branch);

    if git.is_rebase_in_progress() {
        return Err(anyhow!(
            "A rebase is currently in progress. Resolve it first."
        ));
    }
    if git.is_merge_in_progress() {
        return Err(anyhow!(
            "A merge is currently in progress.\n\
             If this is from a previous 'stax include', resolve conflicts and run:\n  \
             stax include --continue"
        ));
    }
    if !git.is_clean()? {
        return Err(anyhow!(
            "Working directory has uncommitted changes. Please commit or stash them first."
        ));
    }

    let consumer = git.current_branch()?;

    if consumer == branch {
        return Err(anyhow!("Cannot include a branch into itself"));
    }

    let all_branches = git.get_branches()?;
    if !all_branches.contains(&branch.to_string()) {
        return Err(anyhow!("Branch '{}' does not exist", branch));
    }

    let main_branches = ["main", "master", "develop"];
    if main_branches.contains(&consumer.as_str()) {
        return Err(anyhow!(
            "Cannot include into a main branch ('{}')",
            consumer
        ));
    }
    if main_branches.contains(&branch) {
        return Err(anyhow!(
            "Cannot include a main branch ('{}') — use 'stax sync' instead",
            branch
        ));
    }

    let (_, _, _, parent_map) = get_branches_and_parent_map(&git)?;

    let mut cache = StackCache::new(&git.git_dir());
    cache.load();

    // Clear any stale shadow merge state
    cache.clear_shadow_merge_state();

    // Determine current parent of consumer
    let current_parent = parent_map
        .get(&consumer)
        .and_then(|p| p.as_ref())
        .cloned()
        .ok_or_else(|| anyhow!("Cannot determine parent of '{}'", consumer))?;

    // Determine sources: existing merge_sources (if any) + new branch
    let existing_sources: Vec<String> = cache
        .data_ref()
        .and_then(|d| d.branches.get(&consumer))
        .map(|b| b.merge_sources.clone())
        .unwrap_or_default();

    let sources: Vec<String> = if existing_sources.is_empty() {
        vec![current_parent.clone(), branch.to_string()]
    } else {
        if existing_sources.contains(&branch.to_string()) {
            return Err(anyhow!(
                "Branch '{}' is already included in '{}'",
                branch,
                consumer
            ));
        }
        let mut s = existing_sources;
        s.push(branch.to_string());
        s
    };

    // Guard: if one source is an ancestor of another, a shadow merge is
    // unnecessary — the user should just reparent. This catches the common
    // mistake of `stax include B` when A and B are both off main (sources
    // would be ["main", "B"], but main is an ancestor of B so the shadow
    // would just be a redundant merge commit equivalent to B).
    if sources.len() == 2 {
        let a = &sources[0];
        let b = &sources[1];
        let a_is_ancestor = git
            .get_merge_base(a, b)
            .ok()
            .map(|mb| {
                mb.to_string()
                    == git
                        .get_commit_hash(&format!("refs/heads/{a}"))
                        .unwrap_or_default()
            })
            .unwrap_or(false);
        let b_is_ancestor = git
            .get_merge_base(a, b)
            .ok()
            .map(|mb| {
                mb.to_string()
                    == git
                        .get_commit_hash(&format!("refs/heads/{b}"))
                        .unwrap_or_default()
            })
            .unwrap_or(false);
        if a_is_ancestor || b_is_ancestor {
            let (ancestor, descendant) = if a_is_ancestor { (a, b) } else { (b, a) };
            return Err(anyhow!(
                "'{branch}' already contains all commits from '{ancestor}', \
                 so a diamond merge is unnecessary.\n\
                 To make '{descendant}' the parent of '{consumer}' instead, \
                 rebase onto it:\n  \
                 git rebase {descendant}\n\
                 Then update the PR base branch with:\n  \
                 stax submit"
            ));
        }
    }

    let shadow_name = StackCache::shadow_name_for(&consumer);

    utils::print_info(&format!(
        "Creating shadow branch '{}' from {:?}",
        shadow_name, sources
    ));

    let source_refs: Vec<&str> = sources.iter().map(|s| s.as_str()).collect();
    match git.recreate_shadow_branch(&shadow_name, &source_refs) {
        Ok(()) => {}
        Err(e) => {
            if let Some(conflict) = e.downcast_ref::<ShadowMergeConflict>() {
                // Save state for --continue
                cache.save_shadow_merge_state(&ShadowMergeState {
                    shadow_name: conflict.shadow_name.clone(),
                    consumer: consumer.clone(),
                    all_sources: sources.clone(),
                    remaining_sources: conflict.remaining_sources.clone(),
                    original_branch: consumer.clone(),
                    continue_command: "stax include --continue".to_string(),
                });

                return Err(anyhow!(
                    "Merge conflict while building shadow branch '{}'.\n\
                     Source '{}' conflicts with prior sources.\n\n\
                     Resolve the conflicts, stage the files, then run:\n  \
                     stax include --continue\n\n\
                     To abort instead:\n  \
                     git merge --abort && git checkout {}",
                    shadow_name,
                    conflict.failed_source,
                    consumer,
                ));
            }
            return Err(e);
        }
    }

    finish_include(&git, &mut cache, &consumer, &shadow_name, &sources)?;

    utils::print_success(&format!(
        "Included '{}' into '{}' via shadow branch",
        branch, consumer
    ));
    utils::print_info(&format!("Sources: {}", sources.join(", ")));

    Ok(())
}

async fn do_continue(git: &GitRepo) -> Result<()> {
    let mut cache = StackCache::new(&git.git_dir());
    let state = cache
        .load_shadow_merge_state()
        .ok_or_else(|| anyhow!("No shadow merge in progress. Nothing to continue."))?;

    log::debug!(
        "include --continue: shadow='{}' consumer='{}' remaining={:?}",
        state.shadow_name,
        state.consumer,
        state.remaining_sources
    );

    if git.is_merge_in_progress() {
        utils::print_info("Committing resolved merge...");
    }

    // Continue the shadow merge (commits resolved merge + merges remaining sources)
    let remaining_refs: Vec<&str> = state.remaining_sources.iter().map(|s| s.as_str()).collect();
    match git.continue_shadow_merge(&state.shadow_name, &remaining_refs) {
        Ok(()) => {}
        Err(e) => {
            if let Some(conflict) = e.downcast_ref::<ShadowMergeConflict>() {
                // Update state with new remaining sources
                cache.save_shadow_merge_state(&ShadowMergeState {
                    remaining_sources: conflict.remaining_sources.clone(),
                    ..state
                });

                return Err(anyhow!(
                    "Another merge conflict while merging '{}'.\n\
                     Resolve the conflicts, stage the files, then run:\n  \
                     stax include --continue",
                    conflict.failed_source,
                ));
            }
            return Err(e);
        }
    }

    // Shadow is fully built — restore original branch and rebase consumer
    git.checkout_branch(&state.consumer)?;

    cache.load();
    finish_include(
        git,
        &mut cache,
        &state.consumer,
        &state.shadow_name,
        &state.all_sources,
    )?;

    cache.clear_shadow_merge_state();

    utils::print_success(&format!(
        "Shadow branch '{}' built and '{}' rebased onto it",
        state.shadow_name, state.consumer
    ));
    utils::print_info(&format!("Sources: {}", state.all_sources.join(", ")));

    Ok(())
}

/// Shared tail: rebase consumer onto shadow and update cache.
fn finish_include(
    git: &GitRepo,
    cache: &mut StackCache,
    consumer: &str,
    shadow_name: &str,
    sources: &[String],
) -> Result<()> {
    utils::print_info(&format!("Rebasing '{}' onto shadow branch...", consumer));
    git.rebase_onto(consumer, shadow_name)?;

    let shadow_tip = git.get_commit_hash(&format!("refs/heads/{shadow_name}"))?;
    cache.upsert_shadow(
        shadow_name,
        ShadowBranch {
            consumer: consumer.to_string(),
            sources: sources.to_vec(),
            tip: shadow_tip.clone(),
        },
    );

    let consumer_tip = git.get_commit_hash(&format!("refs/heads/{consumer}"))?;
    if let Some(data) = cache.data_mut() {
        data.branches.insert(
            consumer.to_string(),
            CachedBranch {
                tip: consumer_tip,
                parent: Some(shadow_name.to_string()),
                merge_sources: sources.to_vec(),
            },
        );
        data.branches.insert(
            shadow_name.to_string(),
            CachedBranch {
                tip: shadow_tip,
                parent: Some(sources[0].clone()),
                merge_sources: Vec::new(),
            },
        );
    }
    cache.save_current();

    Ok(())
}
