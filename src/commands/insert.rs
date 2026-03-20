use crate::cache::StackCache;
use crate::commands::navigate::{
    children_from_map, get_branches_and_parent_map, is_main_branch, is_shadow_branch,
};
use crate::git::GitRepo;
use crate::utils;
use crate::InsertPosition;
use anyhow::{anyhow, Result};

pub async fn run(position: InsertPosition, name: Option<&str>, force: bool) -> Result<()> {
    let git = GitRepo::open(".")?;

    if !git.is_clean()? {
        utils::print_warning("Working directory has uncommitted changes");
        if !utils::confirm("Continue anyway?")? {
            return Ok(());
        }
    }

    let branch_name = match name {
        Some(name) => name.to_string(),
        None => {
            let input = utils::prompt("Enter branch name")?;
            if input.is_empty() {
                utils::print_error("Branch name cannot be empty");
                return Ok(());
            }
            input
        }
    };

    let current_branch = git.current_branch()?;

    if branch_name == current_branch {
        return Err(anyhow!("Cannot insert a branch relative to itself"));
    }

    let existing_branches = git.get_branches()?;
    let branch_exists = existing_branches.contains(&branch_name);

    match position {
        InsertPosition::Below => {
            if branch_exists {
                reparent_below(&git, &current_branch, &branch_name, force)
            } else {
                create_below(&git, &current_branch, &branch_name, force)
            }
        }
        InsertPosition::Above => {
            if branch_exists {
                reparent_above(&git, &current_branch, &branch_name)
            } else {
                create_above(&git, &current_branch, &branch_name)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Create new branch variants (original behavior)
// ---------------------------------------------------------------------------

/// Create a new branch between the current branch and its parent.
fn create_below(git: &GitRepo, current: &str, new_name: &str, force: bool) -> Result<()> {
    log::debug!(
        "insert below (create): current='{}', new='{}'",
        current,
        new_name
    );

    if is_main_branch(current) {
        return Err(anyhow!("Cannot insert below a main branch ('{}')", current));
    }

    let (_, _, merged, parent_map) = get_branches_and_parent_map(git)?;

    let parent = parent_map
        .get(current)
        .and_then(|p| p.as_ref())
        .ok_or_else(|| anyhow!("Cannot determine parent of '{}'", current))?
        .clone();

    // Diamond consumer guard: check merge_sources in cache
    let mut cache = StackCache::new(&git.git_dir());
    cache.load();
    let has_merge_sources = cache
        .data_ref()
        .and_then(|d| d.branches.get(current))
        .map(|b| !b.merge_sources.is_empty())
        .unwrap_or(false);

    if has_merge_sources {
        return Err(anyhow!(
            "Cannot insert below '{}': it has diamond merge dependencies \
             (managed by 'stax include'). Remove the diamond first or insert above instead.",
            current
        ));
    }

    // Shadow parent guard
    if is_shadow_branch(&parent) {
        return Err(anyhow!(
            "Cannot insert below '{}': its parent is a shadow branch \
             (managed by 'stax include').",
            current
        ));
    }

    // Confirm the detected parent with the user. The parent map can be wrong
    // when PR data is stale (e.g., PR targets a deleted branch), so showing
    // the planned structure lets the user catch mistakes before they happen.
    utils::print_info(&format!(
        "This will create: {} -> {} -> {}",
        parent, new_name, current
    ));
    if !force && !utils::confirm("Proceed?")? {
        return Ok(());
    }

    // Create the new branch from the parent's tip
    git.create_branch(new_name, Some(&format!("refs/heads/{parent}")))?;

    let new_tip = git.get_commit_hash(&format!("refs/heads/{new_name}"))?;
    let current_tip = git.get_commit_hash(&format!("refs/heads/{current}"))?;

    // Update cache: new branch's parent = old parent, current's parent = new branch
    cache.upsert_branch(new_name, &new_tip, Some(&parent));
    cache.upsert_branch(current, &current_tip, Some(new_name));

    // Update cached PR base_ref so apply_pr_overrides and topology checks
    // see the new parent relationship (the actual GitHub PR will be updated
    // when the user runs `stax submit`).
    let pr_updated = update_cached_pr_base(cache.data_mut(), current, new_name);
    if pr_updated {
        cache.save_current();
    }

    git.checkout_branch(new_name)?;

    // If the current branch had children, check if any of them were the new
    // branch's siblings (they still point to current, which is correct).
    let children = children_from_map(new_name, &parent_map, &merged);
    let child_info = if children.is_empty() {
        String::new()
    } else {
        format!(
            "\nSiblings (still parented to '{}'): {}",
            parent,
            children.join(", ")
        )
    };

    utils::print_success(&format!("Inserted '{}' below '{}'", new_name, current));
    utils::print_info(&format!("Stack: {} -> {} -> {}", parent, new_name, current));
    if !child_info.is_empty() {
        utils::print_info(&child_info);
    }
    if pr_updated {
        utils::print_info(&format!(
            "Run 'stax submit' to update the PR base branch for '{}' on GitHub",
            current
        ));
    }

    Ok(())
}

/// Create a new branch between the current branch and its children.
fn create_above(git: &GitRepo, current: &str, new_name: &str) -> Result<()> {
    log::debug!(
        "insert above (create): current='{}', new='{}'",
        current,
        new_name
    );

    let (_, _, merged, parent_map) = get_branches_and_parent_map(git)?;

    // Find children of current branch
    let children = children_from_map(current, &parent_map, &merged);

    // Create the new branch from the current branch's tip
    git.create_branch(new_name, Some(&format!("refs/heads/{current}")))?;

    let new_tip = git.get_commit_hash(&format!("refs/heads/{new_name}"))?;

    let mut cache = StackCache::new(&git.git_dir());
    cache.load();

    // Set new branch's parent to current
    cache.upsert_branch(new_name, &new_tip, Some(current));

    // Reparent each child to the new branch
    let mut prs_updated = Vec::new();
    for child in &children {
        reparent_child(git, &mut cache, child, current, new_name, &mut prs_updated)?;
    }

    git.checkout_branch(new_name)?;

    utils::print_success(&format!("Inserted '{}' above '{}'", new_name, current));
    if children.is_empty() {
        utils::print_info(&format!("Parent: {}", current));
    } else {
        utils::print_info(&format!(
            "Stack: {} -> {} -> [{}]",
            current,
            new_name,
            children.join(", ")
        ));
    }
    print_submit_hint(&prs_updated);

    Ok(())
}

// ---------------------------------------------------------------------------
// Reparent existing branch variants
// ---------------------------------------------------------------------------

/// Reparent an existing branch to be a child of the current branch.
fn reparent_above(git: &GitRepo, current: &str, target: &str) -> Result<()> {
    log::debug!(
        "insert above (reparent): current='{}', target='{}'",
        current,
        target
    );

    if is_main_branch(target) {
        return Err(anyhow!("Cannot reparent a main branch ('{}')", target));
    }

    let mut cache = StackCache::new(&git.git_dir());
    cache.load();

    let target_tip = git.get_commit_hash(&format!("refs/heads/{target}"))?;

    // Update cache: target's parent = current
    cache.upsert_branch(target, &target_tip, Some(current));

    // Update cached PR base_ref
    let mut prs_updated = Vec::new();
    if update_cached_pr_base(cache.data_mut(), target, current) {
        prs_updated.push(target.to_string());
        cache.save_current();
    }

    utils::print_success(&format!(
        "Reparented '{}' onto '{}' (above)",
        target, current
    ));
    utils::print_info(&format!("Stack: {} -> {}", current, target));
    utils::print_info("Run 'stax restack' to rebase the branch onto its new parent");
    print_submit_hint(&prs_updated);

    Ok(())
}

/// Reparent the current branch to be a child of an existing branch.
fn reparent_below(git: &GitRepo, current: &str, target: &str, force: bool) -> Result<()> {
    log::debug!(
        "insert below (reparent): current='{}', target='{}'",
        current,
        target
    );

    if is_main_branch(current) {
        return Err(anyhow!("Cannot insert below a main branch ('{}')", current));
    }

    // Confirm the reparenting
    utils::print_info(&format!(
        "This will reparent '{}' onto '{}'",
        current, target
    ));
    if !force && !utils::confirm("Proceed?")? {
        return Ok(());
    }

    let mut cache = StackCache::new(&git.git_dir());
    cache.load();

    let current_tip = git.get_commit_hash(&format!("refs/heads/{current}"))?;

    // Update cache: current's parent = target
    cache.upsert_branch(current, &current_tip, Some(target));

    // Update cached PR base_ref
    let mut prs_updated = Vec::new();
    if update_cached_pr_base(cache.data_mut(), current, target) {
        prs_updated.push(current.to_string());
        cache.save_current();
    }

    utils::print_success(&format!(
        "Reparented '{}' onto '{}' (below)",
        current, target
    ));
    utils::print_info(&format!("Stack: {} -> {}", target, current));
    utils::print_info("Run 'stax restack' to rebase the branch onto its new parent");
    print_submit_hint(&prs_updated);

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Reparent a single child branch to a new parent, handling diamond consumers.
fn reparent_child(
    git: &GitRepo,
    cache: &mut StackCache,
    child: &str,
    old_parent: &str,
    new_parent: &str,
    prs_updated: &mut Vec<String>,
) -> Result<()> {
    log::debug!(
        "reparenting '{}' from '{}' to '{}'",
        child,
        old_parent,
        new_parent
    );

    // Check if this child is a diamond consumer that has old_parent in merge_sources
    let child_merge_sources: Vec<String> = cache
        .data_ref()
        .and_then(|d| d.branches.get(child))
        .map(|b| b.merge_sources.clone())
        .unwrap_or_default();

    if child_merge_sources.contains(&old_parent.to_string()) {
        // Diamond child: update merge_sources to replace old_parent with new_parent
        update_diamond_child_sources(git, cache, child, old_parent, new_parent)?;
    } else {
        // Regular child: just reparent
        let child_tip = git.get_commit_hash(&format!("refs/heads/{child}"))?;
        cache.upsert_branch(child, &child_tip, Some(new_parent));
    }

    // Update cached PR base_ref for the reparented child
    if update_cached_pr_base(cache.data_mut(), child, new_parent) {
        prs_updated.push(child.to_string());
        cache.save_current();
    }

    Ok(())
}

fn print_submit_hint(prs_updated: &[String]) {
    if !prs_updated.is_empty() {
        utils::print_info(&format!(
            "Run 'stax submit' to update PR base branches on GitHub for: {}",
            prs_updated.join(", ")
        ));
    }
}

/// Update the cached PR `base_ref` for a branch to reflect a new parent.
/// Returns true if a PR was updated.
fn update_cached_pr_base(
    data: Option<&mut crate::cache::CacheFile>,
    branch: &str,
    new_base: &str,
) -> bool {
    if let Some(data) = data {
        if let Some(pr) = data.pull_requests.get_mut(branch) {
            log::debug!(
                "insert: updating cached PR #{} base_ref for '{}': '{}' → '{}'",
                pr.number,
                branch,
                pr.base_ref,
                new_base,
            );
            pr.base_ref = new_base.to_string();
            return true;
        }
    }
    false
}

/// Update a diamond consumer child's merge_sources and shadow branch when
/// the current branch (one of its sources) is being replaced by new_name.
pub fn update_diamond_child_sources(
    git: &GitRepo,
    cache: &mut StackCache,
    child: &str,
    old_source: &str,
    new_source: &str,
) -> Result<()> {
    log::debug!(
        "insert: updating diamond child '{}': replacing source '{}' with '{}'",
        child,
        old_source,
        new_source
    );

    let shadow_name = StackCache::shadow_name_for(child);

    // Update merge_sources and shadow in cache
    if let Some(data) = cache.data_mut() {
        // Update the child's merge_sources
        if let Some(child_entry) = data.branches.get_mut(child) {
            for source in &mut child_entry.merge_sources {
                if source == old_source {
                    *source = new_source.to_string();
                }
            }
        }

        // Update the shadow branch sources
        if let Some(shadow) = data.shadow_branches.get_mut(&shadow_name) {
            for source in &mut shadow.sources {
                if source == old_source {
                    *source = new_source.to_string();
                }
            }
        }

        // Also update the shadow's cache entry parent if it pointed to old_source
        if let Some(shadow_entry) = data.branches.get_mut(&shadow_name) {
            if shadow_entry.parent.as_deref() == Some(old_source) {
                shadow_entry.parent = Some(new_source.to_string());
            }
        }
    }
    cache.save_current();

    // Recreate the actual git shadow branch with updated sources.
    let new_sources: Vec<String> = cache
        .data_ref()
        .and_then(|d| d.shadow_branches.get(&shadow_name))
        .map(|s| s.sources.clone())
        .unwrap_or_default();

    if !new_sources.is_empty() {
        let source_refs: Vec<&str> = new_sources.iter().map(|s| s.as_str()).collect();
        git.recreate_shadow_branch(&shadow_name, &source_refs)?;

        // Update shadow tip in cache
        let shadow_tip = git.get_commit_hash(&format!("refs/heads/{shadow_name}"))?;
        if let Some(data) = cache.data_mut() {
            if let Some(shadow) = data.shadow_branches.get_mut(&shadow_name) {
                shadow.tip = shadow_tip.clone();
            }
            if let Some(shadow_entry) = data.branches.get_mut(&shadow_name) {
                shadow_entry.tip = shadow_tip;
            }
        }
        cache.save_current();
    }

    Ok(())
}
