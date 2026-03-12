use crate::cache::{CachedBranch, ShadowBranch, StackCache};
use crate::commands::navigate::get_branches_and_parent_map;
use crate::git::GitRepo;
use crate::utils;
use anyhow::{anyhow, Result};

pub async fn run(branch: &str) -> Result<()> {
    log::debug!("include: adding '{}' as a merge source", branch);
    let git = GitRepo::open(".")?;

    if git.is_rebase_in_progress() {
        return Err(anyhow!(
            "A rebase is currently in progress. Resolve it first."
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
        // First include: sources = [current_parent, new_branch]
        vec![current_parent.clone(), branch.to_string()]
    } else {
        // Already has includes: add the new branch
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

    let shadow_name = StackCache::shadow_name_for(&consumer);

    utils::print_info(&format!(
        "Creating shadow branch '{}' from {:?}",
        shadow_name, sources
    ));

    // Create the shadow merge branch
    let source_refs: Vec<&str> = sources.iter().map(|s| s.as_str()).collect();
    git.recreate_shadow_branch(&shadow_name, &source_refs)?;

    // Rebase consumer onto shadow
    utils::print_info(&format!("Rebasing '{}' onto shadow branch...", consumer));
    git.rebase_onto(&consumer, &shadow_name)?;

    // Update cache: shadow branch entry
    let shadow_tip = git.get_commit_hash(&format!("refs/heads/{shadow_name}"))?;
    cache.upsert_shadow(
        &shadow_name,
        ShadowBranch {
            consumer: consumer.clone(),
            sources: sources.clone(),
            tip: shadow_tip.clone(),
        },
    );

    // Update cache: consumer's merge_sources + parent → shadow, and shadow branch entry
    let consumer_tip = git.get_commit_hash(&format!("refs/heads/{consumer}"))?;
    if let Some(data) = cache.data_mut() {
        data.branches.insert(
            consumer.clone(),
            CachedBranch {
                tip: consumer_tip,
                parent: Some(shadow_name.clone()),
                merge_sources: sources.clone(),
            },
        );
        data.branches.insert(
            shadow_name.clone(),
            CachedBranch {
                tip: shadow_tip,
                parent: Some(sources[0].clone()),
                merge_sources: Vec::new(),
            },
        );
    }
    cache.save_current();

    utils::print_success(&format!(
        "Included '{}' into '{}' via shadow branch",
        branch, consumer
    ));
    utils::print_info(&format!("Sources: {}", sources.join(", ")));

    Ok(())
}
