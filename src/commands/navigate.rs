use crate::cache::StackCache;
use crate::git::GitRepo;
use crate::github::GitHubClient;
use crate::{token_store, utils};
use anyhow::{anyhow, Result};
use colored::*;
use console::{Key, Term};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

const MAIN_BRANCHES: &[&str] = &["main", "master", "develop"];

/// Whether the cursor is currently hidden. Read by the signal handler.
static CURSOR_HIDDEN: AtomicBool = AtomicBool::new(false);

/// Signal handler that restores the cursor and exits.
/// Only uses async-signal-safe functions (write, _exit).
extern "C" fn restore_cursor_on_signal(_sig: libc::c_int) {
    if CURSOR_HIDDEN.load(Ordering::Relaxed) {
        // "\x1b[?25h" = show cursor escape sequence
        let seq = b"\x1b[?25h";
        unsafe {
            libc::write(2, seq.as_ptr() as *const libc::c_void, seq.len());
        }
    }
    unsafe {
        libc::_exit(130);
    }
}

/// Guard that restores the terminal cursor on drop (e.g. on Ctrl+C).
struct CursorGuard {
    term: Term,
}

impl CursorGuard {
    fn new() -> Result<Self> {
        let term = Term::stderr();
        // Install SIGINT handler before hiding cursor
        unsafe {
            libc::signal(
                libc::SIGINT,
                restore_cursor_on_signal as *const () as libc::sighandler_t,
            );
        }
        term.hide_cursor()?;
        CURSOR_HIDDEN.store(true, Ordering::Relaxed);
        Ok(CursorGuard { term })
    }
}

impl Drop for CursorGuard {
    fn drop(&mut self) {
        let _ = self.term.show_cursor();
        CURSOR_HIDDEN.store(false, Ordering::Relaxed);
        // Restore default SIGINT behavior
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
        }
    }
}

/// Shared context for branch operations to avoid passing many parameters.
struct BranchContext<'a> {
    git: &'a GitRepo,
    all_branches: &'a [String],
    commits: &'a HashMap<String, String>,
    merged: &'a HashSet<String>,
}

pub fn is_main_branch(name: &str) -> bool {
    MAIN_BRANCHES.contains(&name)
}

/// Render a branch and its descendants as an ASCII tree with commit messages.
fn render_subtree(
    ctx: &BranchContext,
    branch: &str,
    parent_for_msg: &str,
    prefix: &str,
    is_last: bool,
    is_root: bool,
) -> Vec<String> {
    let mut lines = Vec::new();

    let connector = if is_root {
        ""
    } else if is_last {
        "└─ "
    } else {
        "├─ "
    };

    let msg = ctx
        .git
        .first_commit_message(
            &format!("refs/heads/{parent_for_msg}"),
            &format!("refs/heads/{branch}"),
        )
        .ok()
        .flatten()
        .map(|m| utils::truncate_string(&m, 40))
        .unwrap_or_default();

    let branch_str = if is_root {
        format!("{}", branch.bold())
    } else {
        branch.to_string()
    };
    let msg_str = if msg.is_empty() {
        String::new()
    } else {
        format!(" {}", msg.dimmed())
    };
    lines.push(format!("  {prefix}{connector}{branch_str}{msg_str}"));

    let children = find_children(ctx.git, branch, ctx.all_branches, ctx.commits, ctx.merged)
        .unwrap_or_default();
    let n = children.len();

    let child_prefix = if is_root {
        String::new()
    } else if is_last {
        format!("{prefix}   ")
    } else {
        format!("{prefix}│  ")
    };

    for (i, child) in children.iter().enumerate() {
        lines.extend(render_subtree(
            ctx,
            child,
            branch,
            &child_prefix,
            i == n - 1,
            false,
        ));
    }

    lines
}

/// Interactive branch picker that shows commit messages and a live stack preview.
///
/// If `tree_roots` is provided, the tree preview renders starting from
/// `tree_roots[i]` instead of `choices[i]`. This lets the picker show e.g.
/// the *top* of each stack as the choice while rendering the full stack
/// (from its root) in the preview.
fn pick_branch_with_preview(
    prompt: &str,
    choices: &[String],
    parent_branch: &str,
    ctx: &BranchContext,
    tree_roots: Option<&[String]>,
) -> Result<String> {
    let messages: Vec<String> = choices
        .iter()
        .map(|c| {
            ctx.git
                .first_commit_message(
                    &format!("refs/heads/{parent_branch}"),
                    &format!("refs/heads/{c}"),
                )
                .ok()
                .flatten()
                .map(|m| utils::truncate_string(&m, 40))
                .unwrap_or_default()
        })
        .collect();

    let guard = CursorGuard::new()?;

    let mut selected: usize = 0;
    let mut total_lines: usize = 0;

    let result = (|| -> Result<String> {
        loop {
            if total_lines > 0 {
                guard.term.clear_last_lines(total_lines)?;
            }
            total_lines = 0;

            // Prompt
            guard.term.write_line(&format!("{}:", prompt.bold()))?;
            total_lines += 1;
            guard.term.write_line("")?;
            total_lines += 1;

            // Choice list
            for (i, choice) in choices.iter().enumerate() {
                let msg = &messages[i];
                let line = if i == selected {
                    if msg.is_empty() {
                        format!("  {} {}", ">".green(), choice.green().bold())
                    } else {
                        format!(
                            "  {} {} {}",
                            ">".green(),
                            choice.green().bold(),
                            format!("— {msg}").dimmed()
                        )
                    }
                } else if msg.is_empty() {
                    format!("    {choice}")
                } else {
                    format!("    {choice} {}", format!("— {msg}").dimmed())
                };
                guard.term.write_line(&line)?;
                total_lines += 1;
            }

            // Stack preview — use tree_roots if provided, otherwise choices
            guard.term.write_line("")?;
            total_lines += 1;
            let tree_branch = tree_roots
                .map(|roots| roots[selected].as_str())
                .unwrap_or(&choices[selected]);
            let tree = render_subtree(ctx, tree_branch, parent_branch, "", true, true);
            for line in &tree {
                guard.term.write_line(line)?;
                total_lines += 1;
            }

            match guard.term.read_key()? {
                Key::ArrowUp | Key::Char('k') => {
                    selected = selected.saturating_sub(1);
                }
                Key::ArrowDown | Key::Char('j') => {
                    selected = (selected + 1).min(choices.len() - 1);
                }
                Key::Enter => break,
                Key::Escape | Key::Char('\x03') => return Err(anyhow!("Selection cancelled")),
                _ => {}
            }
        }

        Ok(choices[selected].clone())
    })();

    // Clear picker and show final selection (guard restores cursor on drop)
    if total_lines > 0 {
        guard.term.clear_last_lines(total_lines)?;
    }
    if let Ok(ref choice) = result {
        guard
            .term
            .write_line(&format!("{}: {}", prompt.bold(), choice.green()))?;
    }

    result
}

/// Pre-compute commit hashes for all branches.
pub fn build_commit_cache(git: &GitRepo, branches: &[String]) -> Result<HashMap<String, String>> {
    branches
        .iter()
        .map(|b| {
            let hash = git.get_commit_hash(&format!("refs/heads/{b}"))?;
            Ok((b.clone(), hash))
        })
        .collect()
}

/// Find the parent of `branch` — the closest branch whose tip equals the
/// merge-base of itself and `branch`. O(n) merge-base calls.
///
/// Uses a two-pass approach:
/// 1. **Strict**: candidate's tip == merge-base(branch, candidate).
///    This is the common case for clean stacks.
/// 2. **Relaxed**: candidate shares non-trunk history with branch
///    (merge-base of branch & candidate is ahead of merge-base of branch &
///    trunk). This catches cases where a parent branch has merged/rebased
///    main, moving its tip past where the child branched off.
///
/// Merged non-trunk branches are skipped so navigation never lands on a
/// branch that has already been merged.
pub fn find_parent(
    git: &GitRepo,
    branch: &str,
    all_branches: &[String],
    commits: &HashMap<String, String>,
    merged: &HashSet<String>,
) -> Result<Option<String>> {
    let branch_commit = &commits[branch];
    let mut best_parent = None;
    let mut min_distance = usize::MAX;

    // Collect merge-bases for each candidate (reused in both passes)
    let mut candidate_merge_bases: Vec<(&String, String)> = Vec::new();

    // --- Pass 1: strict check (merge-base == candidate tip) ---
    for candidate in all_branches {
        if candidate == branch {
            continue;
        }
        // Skip merged non-trunk branches
        if !is_main_branch(candidate) && merged.contains(candidate) {
            continue;
        }
        let candidate_commit = &commits[candidate];
        if candidate_commit == branch_commit {
            continue;
        }
        if let Ok(merge_base) = git.get_merge_base(branch, candidate) {
            let mb_str = merge_base.to_string();
            if mb_str == *candidate_commit {
                let distance = git.count_commits_between(
                    &format!("refs/heads/{candidate}"),
                    &format!("refs/heads/{branch}"),
                )?;
                if distance < min_distance {
                    min_distance = distance;
                    best_parent = Some(candidate.clone());
                }
            }
            candidate_merge_bases.push((candidate, mb_str));
        }
    }

    if best_parent.is_some() {
        return Ok(best_parent);
    }

    // --- Pass 2: relaxed check for branches that merged trunk ---
    // Find the trunk branch so we can compute merge-base(branch, trunk).
    let trunk = all_branches
        .iter()
        .find(|b| is_main_branch(b) && *b != branch);
    if let Some(trunk_name) = trunk {
        if let Ok(mb_trunk) = git.get_merge_base(branch, trunk_name) {
            let mb_trunk_str = mb_trunk.to_string();

            for (candidate, mb_str) in &candidate_merge_bases {
                if is_main_branch(candidate) {
                    continue;
                }
                // Skip if merge-base is branch's own commit (reversed relationship)
                if *mb_str == *branch_commit {
                    continue;
                }
                // Skip if merge-base is the same as trunk merge-base
                // (candidate only shares trunk history, not unique commits)
                if *mb_str == mb_trunk_str {
                    continue;
                }
                // Check that the shared point (mb) is strictly ahead of
                // the trunk divergence point — meaning the candidate has
                // unique commits (beyond trunk) that are in branch's history.
                if let Ok(ancestry_base) = git.get_merge_base(&mb_trunk_str, mb_str) {
                    if ancestry_base.to_string() == mb_trunk_str {
                        // mb_trunk is an ancestor of mb — candidate shares
                        // non-trunk history with branch.
                        let distance = git.count_commits_between(
                            &format!("refs/heads/{candidate}"),
                            &format!("refs/heads/{branch}"),
                        )?;
                        if distance < min_distance {
                            min_distance = distance;
                            best_parent = Some(candidate.to_string());
                        }
                    }
                }
            }
        }
    }

    if best_parent.is_some() {
        return Ok(best_parent);
    }

    // Fall back to trunk
    for name in MAIN_BRANCHES {
        if all_branches.iter().any(|b| b == name) && *name != branch {
            return Ok(Some(name.to_string()));
        }
    }

    Ok(best_parent)
}

/// Find direct children of `branch` — branches whose closest parent is
/// `branch`. O(n) merge-base checks + O(k²) filtering among candidates.
/// Merged branches are excluded since they shouldn't be navigated to and
/// their presence in trunk history causes false parent-child matches.
pub fn find_children(
    git: &GitRepo,
    branch: &str,
    all_branches: &[String],
    commits: &HashMap<String, String>,
    merged: &HashSet<String>,
) -> Result<Vec<String>> {
    let branch_commit = &commits[branch];

    // Collect candidates: branches whose merge-base with `branch` equals branch's tip
    let mut candidates = Vec::new();
    for candidate in all_branches {
        if candidate == branch || is_main_branch(candidate) {
            continue;
        }
        // Skip merged branches — they shouldn't be navigation targets and
        // their commits in trunk history cause false positives.
        if merged.contains(candidate) {
            continue;
        }
        let candidate_commit = &commits[candidate];
        if candidate_commit == branch_commit {
            continue;
        }
        if let Ok(merge_base) = git.get_merge_base(branch, candidate) {
            if merge_base.to_string() == *branch_commit {
                candidates.push(candidate.clone());
            }
        }
    }

    // Filter to direct children: remove candidates that are descendants of
    // another candidate (those are grandchildren, not children).
    let mut direct = Vec::new();
    for candidate in &candidates {
        let candidate_commit = &commits[candidate];
        let is_grandchild = candidates.iter().any(|other| {
            if other == candidate {
                return false;
            }
            let other_commit = &commits[other];
            if other_commit == candidate_commit {
                return false;
            }
            git.get_merge_base(candidate, other)
                .map(|mb| mb.to_string() == *other_commit)
                .unwrap_or(false)
        });
        if !is_grandchild {
            direct.push(candidate.clone());
        }
    }

    Ok(direct)
}

/// Build a map from each branch to its parent (via `find_parent`).
/// Runs `find_parent` calls in parallel across multiple threads, each with
/// its own `GitRepo` handle. This turns O(n²) sequential merge-base calls
/// into ~O(n²/threads) wall-clock time.
///
/// Merged branches are skipped — they don't need parent computation since
/// they'll never be navigation targets.  This is the biggest perf win in
/// large team repos where most branches are already merged.
pub fn build_parent_map(
    git: &GitRepo,
    all_branches: &[String],
    commits: &HashMap<String, String>,
    merged: &HashSet<String>,
) -> Result<HashMap<String, Option<String>>> {
    let mut map = HashMap::new();

    // Separate main branches (trivial: parent = None) from active branches
    // that need find_parent.
    let mut active: Vec<&String> = Vec::new();
    for branch in all_branches {
        if is_main_branch(branch) {
            map.insert(branch.clone(), None);
        } else if !merged.contains(branch) {
            active.push(branch);
        }
    }

    if active.is_empty() {
        return Ok(map);
    }

    let num_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(active.len());

    if num_threads <= 1 {
        // Not worth spawning threads for ≤1 active branch
        for branch in &active {
            let parent = find_parent(git, branch, all_branches, commits, merged)?;
            map.insert((*branch).clone(), parent);
        }
        return Ok(map);
    }

    let repo_path = git.repo_workdir()?;
    let chunk_size = active.len().div_ceil(num_threads);

    // Each scoped thread opens its own GitRepo (git2::Repository is not
    // Send/Sync, but multiple handles to the same .git dir are safe for
    // read-only operations).  The commits/merged/all_branches refs are
    // shared read-only.
    #[allow(clippy::type_complexity)]
    let results: Result<Vec<Vec<(String, Option<String>)>>> = std::thread::scope(|s| {
        let handles: Vec<_> = active
            .chunks(chunk_size)
            .map(|chunk| {
                s.spawn(|| {
                    let git = GitRepo::open(&repo_path)?;
                    chunk
                        .iter()
                        .map(|branch| {
                            let parent = find_parent(&git, branch, all_branches, commits, merged)?;
                            Ok(((*branch).clone(), parent))
                        })
                        .collect::<Result<Vec<_>>>()
                })
            })
            .collect();

        handles
            .into_iter()
            .map(|h| h.join().expect("thread panicked"))
            .collect()
    });

    for chunk_results in results? {
        for (branch, parent) in chunk_results {
            map.insert(branch, parent);
        }
    }

    Ok(map)
}

/// Look up direct children of `branch` from a pre-computed parent map.
pub fn children_from_map(
    branch: &str,
    parent_map: &HashMap<String, Option<String>>,
    merged: &HashSet<String>,
) -> Vec<String> {
    let mut children = Vec::new();
    for (candidate, parent) in parent_map {
        if is_main_branch(candidate) || merged.contains(candidate) {
            continue;
        }
        if let Some(p) = parent {
            if p == branch {
                children.push(candidate.clone());
            }
        }
    }
    children.sort();
    children
}

/// Walk from `start` towards the top of its stack, following linear chains.
/// Stops at the leaf (no children) or at a fork (multiple children).
/// Returns the top-most branch reached.
pub fn walk_to_top(
    start: &str,
    parent_map: &HashMap<String, Option<String>>,
    merged: &HashSet<String>,
) -> String {
    let mut current = start.to_string();
    loop {
        let children = children_from_map(&current, parent_map, merged);
        match children.len() {
            0 => break,
            1 => current = children[0].clone(),
            _ => break, // fork — stop here, user will be prompted later
        }
    }
    current
}

/// Find branches that form the base of stacks off `main_branch`
/// using a pre-computed parent map.
pub fn root_children_from_map(
    main_branch: &str,
    parent_map: &HashMap<String, Option<String>>,
    merged: &HashSet<String>,
) -> Vec<String> {
    let mut root_children = Vec::new();
    for (branch, parent) in parent_map {
        if branch == main_branch || is_main_branch(branch) || merged.contains(branch) {
            continue;
        }
        if let Some(p) = parent {
            if p == main_branch || merged.contains(p) {
                root_children.push(branch.clone());
            }
        }
    }
    root_children.sort();
    root_children
}

/// Get all local branches with pre-computed commit hashes and merged status.
/// Returns the full branch list, commit cache, and set of merged branch names.
/// All branches are kept (including merged ones) so that `find_parent` can
/// still detect parent chains through merged branches.  Individual functions
/// like `find_children` and `find_root_children` use the merged set to skip
/// branches that shouldn't be navigation targets.
#[allow(clippy::type_complexity)]
pub fn get_branches_with_cache(
    git: &GitRepo,
) -> Result<(Vec<String>, HashMap<String, String>, HashSet<String>)> {
    let all = git.get_branches()?;
    let commits = build_commit_cache(git, &all)?;

    let trunk = all.iter().find(|b| is_main_branch(b)).cloned();
    let merged = if let Some(ref trunk) = trunk {
        // Collect non-main branches that need a merge-base check
        let candidates: Vec<&String> = all
            .iter()
            .filter(|b| !is_main_branch(b) && commits.contains_key(*b))
            .collect();

        if candidates.is_empty() {
            HashSet::new()
        } else {
            let num_threads = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .min(candidates.len());

            if num_threads <= 1 {
                candidates
                    .iter()
                    .filter(|b| {
                        let bh = &commits[**b];
                        git.get_merge_base(b, trunk)
                            .map(|mb| *bh == mb.to_string())
                            .unwrap_or(false)
                    })
                    .map(|b| (*b).clone())
                    .collect()
            } else {
                let repo_path = git.repo_workdir()?;
                let chunk_size = candidates.len().div_ceil(num_threads);

                let results: Vec<Vec<String>> = std::thread::scope(|s| {
                    let handles: Vec<_> = candidates
                        .chunks(chunk_size)
                        .map(|chunk| {
                            s.spawn(|| {
                                let git = GitRepo::open(&repo_path).ok()?;
                                Some(
                                    chunk
                                        .iter()
                                        .filter(|b| {
                                            let bh = &commits[**b];
                                            git.get_merge_base(b, trunk)
                                                .map(|mb| *bh == mb.to_string())
                                                .unwrap_or(false)
                                        })
                                        .map(|b| (*b).clone())
                                        .collect::<Vec<_>>(),
                                )
                            })
                        })
                        .collect();

                    handles
                        .into_iter()
                        .filter_map(|h| h.join().expect("thread panicked"))
                        .collect()
                });

                results.into_iter().flatten().collect()
            }
        }
    } else {
        HashSet::new()
    };

    Ok((all, commits, merged))
}

/// Apply PR base_ref overrides to the parent map.
///
/// When a branch has a cached PR, the PR's `base_ref` is the authoritative
/// parent (the user set it explicitly when creating the PR).  This ensures
/// navigate commands and `stax stack` always agree on the stack structure.
fn apply_pr_overrides(
    parent_map: &mut HashMap<String, Option<String>>,
    cache: &StackCache,
    all_branches: &[String],
) {
    let prs = match cache.data_ref() {
        Some(data) => &data.pull_requests,
        None => return,
    };
    if prs.is_empty() {
        return;
    }
    let branch_set: HashSet<&str> = all_branches.iter().map(|s| s.as_str()).collect();
    let mut count = 0u32;
    for pr in prs.values() {
        if parent_map.contains_key(&pr.head_ref) && branch_set.contains(pr.base_ref.as_str()) {
            let current = parent_map.get(&pr.head_ref).and_then(|p| p.as_ref());
            if current != Some(&pr.base_ref) {
                log::debug!(
                    "cache: PR #{} overrides parent of '{}': {:?} → '{}'",
                    pr.number,
                    pr.head_ref,
                    current,
                    pr.base_ref
                );
                parent_map.insert(pr.head_ref.clone(), Some(pr.base_ref.clone()));
                count += 1;
            }
        }
    }
    if count > 0 {
        log::debug!("cache: applied {} PR base_ref overrides", count);
    }
}

/// Load branches, commit hashes, merged status, and parent map.
///
/// Uses a local cache (`.git/stax/cache.json`) when all branch tips match,
/// falling back to full recomputation on any cache miss or error.  On a full
/// or partial recompute the cache is updated for next time.
///
/// PR `base_ref` overrides are applied to the parent map so that all
/// consumers (navigate commands, `stax stack`, etc.) see a consistent view
/// of the stack structure.
#[allow(clippy::type_complexity)]
pub fn get_branches_and_parent_map(
    git: &GitRepo,
) -> Result<(
    Vec<String>,
    HashMap<String, String>,
    HashSet<String>,
    HashMap<String, Option<String>>,
)> {
    // 1. Enumerate branches and build commit cache (microseconds — just ref reads)
    let all = git.get_branches()?;
    let commits = build_commit_cache(git, &all)?;

    let trunk_name = all
        .iter()
        .find(|b| is_main_branch(b))
        .cloned()
        .unwrap_or_else(|| "main".to_string());

    // 2. Try cache
    let mut cache = StackCache::new(&git.git_dir());

    if cache.load().is_some() {
        if let Some(validation) = cache.validate(&commits, &trunk_name) {
            let is_full_hit = validation.stale.is_empty()
                && validation.new_branches.is_empty()
                && validation.deleted.is_empty()
                && !validation.trunk_changed;

            if is_full_hit {
                // === FULL CACHE HIT ===
                log::debug!("cache: full hit, skipping recompute");
                let data = cache.data_ref().unwrap();
                let mut parent_map = StackCache::to_parent_map(data);
                let merged = StackCache::to_merged_set(data);
                apply_pr_overrides(&mut parent_map, &cache, &all);
                return Ok((all, commits, merged, parent_map));
            }

            // === PARTIAL CACHE HIT ===
            let merged = if validation.trunk_changed {
                log::debug!("cache: trunk changed, recomputing merged set");
                compute_merged_set(git, &all, &commits)?
            } else {
                validation.cached_merged
            };

            // Start with valid entries
            let mut parent_map: HashMap<String, Option<String>> = HashMap::new();
            for b in &all {
                if is_main_branch(b) {
                    parent_map.insert(b.clone(), None);
                }
            }
            for (name, cached) in &validation.valid {
                if !merged.contains(name) {
                    parent_map.insert(name.clone(), cached.parent.clone());
                }
            }

            // Identify branches needing recompute
            let mut needs_recompute: HashSet<String> = HashSet::new();
            needs_recompute.extend(validation.stale.iter().cloned());
            needs_recompute.extend(validation.new_branches.iter().cloned());
            // Also recompute branches whose cached parent is stale or deleted
            for (name, cached) in &validation.valid {
                if let Some(ref parent) = cached.parent {
                    if validation.stale.contains(parent) || validation.deleted.contains(parent) {
                        log::debug!(
                            "cache: {} needs recompute (parent {} is stale/deleted)",
                            name,
                            parent,
                        );
                        needs_recompute.insert(name.clone());
                    }
                }
            }

            // Filter to active (non-main, non-merged) branches
            let recompute_vec: Vec<&String> = needs_recompute
                .iter()
                .filter(|b| !is_main_branch(b) && !merged.contains(*b))
                .collect();

            log::debug!(
                "cache: partial hit, recomputing {} branches",
                recompute_vec.len()
            );

            if !recompute_vec.is_empty() {
                let recomputed = recompute_parents(git, &recompute_vec, &all, &commits, &merged)?;
                for (branch, parent) in recomputed {
                    parent_map.insert(branch, parent);
                }
            }

            // Apply PR overrides before saving — the saved cache should
            // reflect the authoritative parent relationships.
            apply_pr_overrides(&mut parent_map, &cache, &all);

            // Save updated cache (preserves existing PR data)
            let trunk_tip = commits.get(&trunk_name).cloned().unwrap_or_default();
            let data =
                cache.build_cache_data(&trunk_name, &trunk_tip, &parent_map, &commits, &merged);
            cache.save(&data);

            return Ok((all, commits, merged, parent_map));
        }
    }

    // === CACHE MISS ===
    log::debug!("cache: miss, full recompute ({} branches)", all.len());
    let merged = compute_merged_set(git, &all, &commits)?;
    let mut parent_map = build_parent_map(git, &all, &commits, &merged)?;

    // Apply PR overrides (cache may still have PR data even on branch miss)
    apply_pr_overrides(&mut parent_map, &cache, &all);

    // Save for next time (preserves existing PR data)
    let trunk_tip = commits.get(&trunk_name).cloned().unwrap_or_default();
    let data = cache.build_cache_data(&trunk_name, &trunk_tip, &parent_map, &commits, &merged);
    cache.save(&data);

    Ok((all, commits, merged, parent_map))
}

/// Compute the merged set — branches whose tip equals their merge-base with
/// trunk.  Parallelised with `std::thread::scope`.
fn compute_merged_set(
    git: &GitRepo,
    all: &[String],
    commits: &HashMap<String, String>,
) -> Result<HashSet<String>> {
    let trunk = match all.iter().find(|b| is_main_branch(b)) {
        Some(t) => t,
        None => {
            log::debug!("compute_merged_set: no trunk branch found, returning empty set");
            return Ok(HashSet::new());
        }
    };

    let candidates: Vec<&String> = all
        .iter()
        .filter(|b| !is_main_branch(b) && commits.contains_key(*b))
        .collect();

    if candidates.is_empty() {
        return Ok(HashSet::new());
    }

    let num_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(candidates.len());

    if num_threads <= 1 {
        return Ok(candidates
            .iter()
            .filter(|b| {
                let bh = &commits[**b];
                git.get_merge_base(b, trunk)
                    .map(|mb| *bh == mb.to_string())
                    .unwrap_or(false)
            })
            .map(|b| (*b).clone())
            .collect());
    }

    let repo_path = git.repo_workdir()?;
    let chunk_size = candidates.len().div_ceil(num_threads);

    let results: Vec<Vec<String>> = std::thread::scope(|s| {
        let handles: Vec<_> = candidates
            .chunks(chunk_size)
            .map(|chunk| {
                s.spawn(|| {
                    let git = GitRepo::open(&repo_path).ok()?;
                    Some(
                        chunk
                            .iter()
                            .filter(|b| {
                                let bh = &commits[**b];
                                git.get_merge_base(b, trunk)
                                    .map(|mb| *bh == mb.to_string())
                                    .unwrap_or(false)
                            })
                            .map(|b| (*b).clone())
                            .collect::<Vec<_>>(),
                    )
                })
            })
            .collect();

        handles
            .into_iter()
            .filter_map(|h| h.join().expect("thread panicked"))
            .collect()
    });

    let merged: HashSet<String> = results.into_iter().flatten().collect();
    log::debug!(
        "compute_merged_set: found {} merged branches out of {} candidates",
        merged.len(),
        candidates.len()
    );
    if !merged.is_empty() {
        let names: Vec<&String> = merged.iter().collect();
        log::debug!("compute_merged_set: merged branches: {names:?}");
    }
    Ok(merged)
}

/// Recompute parents for a subset of branches.  Uses the same parallel
/// `std::thread::scope` pattern as `build_parent_map`.
fn recompute_parents(
    git: &GitRepo,
    branches: &[&String],
    all_branches: &[String],
    commits: &HashMap<String, String>,
    merged: &HashSet<String>,
) -> Result<Vec<(String, Option<String>)>> {
    let branch_names: Vec<&str> = branches.iter().map(|b| b.as_str()).collect();
    log::debug!(
        "recompute_parents: recomputing {} branches: {branch_names:?}",
        branches.len()
    );

    if branches.len() <= 1 {
        let mut out = Vec::new();
        for branch in branches {
            let parent = find_parent(git, branch, all_branches, commits, merged)?;
            log::debug!("recompute_parents: {} -> {:?}", branch, parent);
            out.push(((*branch).clone(), parent));
        }
        return Ok(out);
    }

    let repo_path = git.repo_workdir()?;
    let num_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(branches.len());
    let chunk_size = branches.len().div_ceil(num_threads);

    #[allow(clippy::type_complexity)]
    let results: Result<Vec<Vec<(String, Option<String>)>>> = std::thread::scope(|s| {
        let handles: Vec<_> = branches
            .chunks(chunk_size)
            .map(|chunk| {
                s.spawn(|| {
                    let git = GitRepo::open(&repo_path)?;
                    chunk
                        .iter()
                        .map(|branch| {
                            let parent = find_parent(&git, branch, all_branches, commits, merged)?;
                            Ok(((*branch).clone(), parent))
                        })
                        .collect::<Result<Vec<_>>>()
                })
            })
            .collect();

        handles
            .into_iter()
            .map(|h| h.join().expect("thread panicked"))
            .collect()
    });

    let out: Vec<(String, Option<String>)> = results?.into_iter().flatten().collect();
    for (branch, parent) in &out {
        log::debug!("recompute_parents: {} -> {:?}", branch, parent);
    }
    Ok(out)
}

pub async fn down() -> Result<()> {
    let git = GitRepo::open(".")?;
    let current = git.current_branch()?;
    log::debug!("navigate down: current='{}'", current);

    if is_main_branch(&current) {
        return Err(anyhow!("Already at the bottom of the stack"));
    }

    let (_branches, _commits, _merged, parent_map) = get_branches_and_parent_map(&git)?;
    let parent = parent_map
        .get(&current)
        .and_then(|p| p.clone())
        .ok_or_else(|| anyhow!("Already at the bottom of the stack"))?;
    log::debug!("navigate down: target='{}'", parent);

    git.checkout_branch(&parent)?;
    utils::print_success(&format!("Moved down to {}", parent));
    Ok(())
}

pub async fn up() -> Result<()> {
    let git = GitRepo::open(".")?;
    let current = git.current_branch()?;
    log::debug!("navigate up: current='{}'", current);
    let (branches, commits, merged, parent_map) = get_branches_and_parent_map(&git)?;

    let children = if is_main_branch(&current) {
        root_children_from_map(&current, &parent_map, &merged)
    } else {
        children_from_map(&current, &parent_map, &merged)
    };

    let ctx = BranchContext {
        git: &git,
        all_branches: &branches,
        commits: &commits,
        merged: &merged,
    };
    let target = match children.len() {
        0 => return Err(anyhow!("Already at the top of the stack")),
        1 => children[0].clone(),
        _ => pick_branch_with_preview(
            "Multiple children — pick a branch",
            &children,
            &current,
            &ctx,
            None,
        )?,
    };

    log::debug!("navigate up: target='{}'", target);
    git.checkout_branch(&target)?;
    utils::print_success(&format!("Moved up to {}", target));
    Ok(())
}

pub async fn bottom() -> Result<()> {
    let git = GitRepo::open(".")?;
    let current = git.current_branch()?;
    log::debug!("navigate bottom: current='{}'", current);
    let (branches, commits, merged, parent_map) = get_branches_and_parent_map(&git)?;

    if is_main_branch(&current) {
        let children = root_children_from_map(&current, &parent_map, &merged);
        return match children.len() {
            0 => {
                utils::print_info("No stack branches above main");
                Ok(())
            }
            1 => {
                git.checkout_branch(&children[0])?;
                utils::print_success(&format!("Moved to bottom of stack: {}", children[0]));
                Ok(())
            }
            _ => {
                let ctx = BranchContext {
                    git: &git,
                    all_branches: &branches,
                    commits: &commits,
                    merged: &merged,
                };
                let target = pick_branch_with_preview(
                    "Multiple stacks — pick a branch",
                    &children,
                    &current,
                    &ctx,
                    None,
                )?;
                git.checkout_branch(&target)?;
                utils::print_success(&format!("Moved to bottom of stack: {}", target));
                Ok(())
            }
        };
    }

    // Walk parent chain using the pre-computed map
    let mut target = current.clone();
    loop {
        match parent_map.get(&target).and_then(|p| p.as_ref()) {
            Some(p) if is_main_branch(p) => break,
            Some(p) => target = p.clone(),
            None => break,
        }
    }

    if target == current {
        utils::print_info("Already at the bottom of the stack");
        return Ok(());
    }

    git.checkout_branch(&target)?;
    utils::print_success(&format!("Moved to bottom of stack: {}", target));
    Ok(())
}

/// Spawn a background task to fetch open PR branch names from GitHub.
/// Returns a JoinHandle so the caller can overlap git work with the network
/// request.  Returns None if GitHub is unavailable (no token, no remote).
fn spawn_open_pr_branches(
    git: &GitRepo,
) -> Option<tokio::task::JoinHandle<Option<HashSet<String>>>> {
    let token = token_store::get_token()?;
    let remote_url = git.get_remote_url("origin")?;
    let client = GitHubClient::new(&token, &remote_url).ok()?;
    Some(tokio::spawn(async move {
        let prs = client.get_open_pull_requests().await.ok()?;
        Some(prs.into_iter().map(|pr| pr.head_ref).collect())
    }))
}

/// Returns true if the stack (represented by its root and top) should be
/// shown in the picker.  A single-branch stack is hidden when the branch
/// has no open PR — it's effectively dead (merged/closed/abandoned).
pub fn is_active_stack(
    root: &str,
    top: &str,
    open_branches: Option<&HashSet<String>>,
    parent_map: &HashMap<String, Option<String>>,
    merged: &HashSet<String>,
) -> bool {
    // Multi-branch stacks are always shown (user explicitly built a stack)
    if root != top {
        return true;
    }
    // Single-branch stack: check if it has children (fork at root)
    let children = children_from_map(root, parent_map, merged);
    if children.len() > 1 {
        return true;
    }
    // If we have PR data, hide branches without an open PR
    if let Some(open) = open_branches {
        return open.contains(root);
    }
    // No PR data available — show everything
    true
}

pub async fn top() -> Result<()> {
    let git = GitRepo::open(".")?;
    let current = git.current_branch()?;
    log::debug!("navigate top: current='{}'", current);

    // Fire off GitHub API call immediately so it runs concurrently with
    // the (potentially expensive) parent-map computation below.
    let pr_handle = if is_main_branch(&current) {
        spawn_open_pr_branches(&git)
    } else {
        None
    };

    let (branches, commits, merged, parent_map) = get_branches_and_parent_map(&git)?;

    let ctx = BranchContext {
        git: &git,
        all_branches: &branches,
        commits: &commits,
        merged: &merged,
    };

    // When on a main branch, use root_children_from_map to correctly find
    // branches even when main has moved forward since they were created.
    // Walk each root to its top so the picker shows where you'll end up.
    let mut target = current.clone();
    if is_main_branch(&current) {
        let root_children = root_children_from_map(&current, &parent_map, &merged);

        // Walk each root child to the top of its linear stack
        let mut tops = Vec::new();
        let mut roots = Vec::new();
        for root in &root_children {
            let top = walk_to_top(root, &parent_map, &merged);
            tops.push(top);
            roots.push(root.clone());
        }

        // Await the GitHub API result (likely already finished by now)
        let open_branches = match pr_handle {
            Some(h) => h.await.ok().flatten(),
            None => None,
        };

        // Filter to active stacks only
        let (tops, roots): (Vec<_>, Vec<_>) = tops
            .into_iter()
            .zip(roots)
            .filter(|(top, root)| {
                is_active_stack(root, top, open_branches.as_ref(), &parent_map, &merged)
            })
            .unzip();

        match tops.len() {
            0 => {
                utils::print_info("No active stack branches above main");
                return Ok(());
            }
            1 => {
                target = tops.into_iter().next().unwrap();
            }
            _ => {
                target = pick_branch_with_preview(
                    "Multiple stacks — pick one to go to the top of",
                    &tops,
                    &current,
                    &ctx,
                    Some(&roots),
                )?;
            }
        }
    }

    // Walk children until reaching a leaf, prompting at forks.
    // Uses the pre-computed parent map so no extra git calls.
    loop {
        let children = children_from_map(&target, &parent_map, &merged);
        match children.len() {
            0 => break,
            1 => target = children[0].clone(),
            _ => {
                target = pick_branch_with_preview(
                    "Multiple children — pick a branch",
                    &children,
                    &target,
                    &ctx,
                    None,
                )?
            }
        }
    }

    if target == current {
        utils::print_info("Already at the top of the stack");
        return Ok(());
    }

    git.checkout_branch(&target)?;
    utils::print_success(&format!("Moved to top of stack: {}", target));
    Ok(())
}
