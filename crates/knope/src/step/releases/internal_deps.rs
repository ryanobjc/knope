use std::collections::{HashMap, HashSet};

use knope_config::InternalDependencyUpdate;
use knope_versioning::{
    changes::{Change, ChangeSource, ChangeType},
    package::Name,
    semver::Version,
};
use relative_path::RelativePathBuf;
use tracing::debug;

use super::Package;

/// A bumped dependency that should be reflected in a dependent's release notes.
#[derive(Clone, Debug)]
pub(super) struct BumpedDependency {
    pub name: Name,
    pub new_version: Version,
}

/// Build dependents-of edges: `dependents[D]` = indices of packages that depend on D.
///
/// A package P "owns" a path if it has a `versioned_files` entry for that path with no
/// `dependency` set (i.e., the file defines P's own version). Every other entry of the form
/// `{ path, dependency = D }` whose `path` is owned by some other package O implies the edge
/// "O depends on D".
pub(super) fn build_dependents(packages: &[Package]) -> HashMap<Name, Vec<usize>> {
    let mut owners: HashMap<RelativePathBuf, usize> = HashMap::new();
    for (idx, pkg) in packages.iter().enumerate() {
        for vf in pkg.versioning.versioned_files() {
            if vf.dependency.is_none() {
                owners.insert(vf.as_path(), idx);
            }
        }
    }

    let mut edges: HashMap<Name, Vec<usize>> = HashMap::new();
    let mut seen: HashSet<(usize, String)> = HashSet::new();
    for pkg in packages {
        for vf in pkg.versioning.versioned_files() {
            let Some(dep_name) = vf.dependency.as_ref() else {
                continue;
            };
            let Some(&owner_idx) = owners.get(&vf.as_path()) else {
                continue;
            };
            let Some(owner) = packages.get(owner_idx) else {
                continue;
            };
            if owner.name().as_ref() == dep_name.as_str() {
                continue;
            }
            if !seen.insert((owner_idx, dep_name.clone())) {
                continue;
            }
            edges
                .entry(Name::from(dep_name.as_str()))
                .or_default()
                .push(owner_idx);
        }
    }
    edges
}

/// Topologically sort package indices so that a package appears after all of its internal
/// dependencies. Falls back to original order on cycles (we still return every index).
#[expect(
    clippy::indexing_slicing,
    reason = "all indices come from 0..packages.len() or from `dependents` values that were built from those same indices"
)]
pub(super) fn topological_order(
    packages: &[Package],
    dependents: &HashMap<Name, Vec<usize>>,
) -> Vec<usize> {
    let n = packages.len();
    let mut in_degree = vec![0usize; n];
    for targets in dependents.values() {
        for &t in targets {
            if t < n {
                in_degree[t] += 1;
            }
        }
    }
    let mut queue: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
    let mut visited = vec![false; n];
    let mut order = Vec::with_capacity(n);
    while let Some(idx) = queue.pop() {
        if visited[idx] {
            continue;
        }
        visited[idx] = true;
        order.push(idx);
        if let Some(targets) = dependents.get(packages[idx].name()) {
            for &t in targets {
                if t >= n || visited[t] {
                    continue;
                }
                in_degree[t] = in_degree[t].saturating_sub(1);
                if in_degree[t] == 0 {
                    queue.push(t);
                }
            }
        }
    }
    // Append any remaining (would happen if there's a cycle) — better to release them than panic.
    for idx in 0..n {
        if !visited[idx] {
            debug!(
                "Internal dependency cycle detected; falling back to source order for {pkg}",
                pkg = packages[idx].name()
            );
            order.push(idx);
        }
    }
    order
}

/// Build a synthetic [`Change`] representing "this package was bumped because its internal
/// dependencies updated." The change is grouped with any others on the same package, and
/// rendered as an "Updated dependencies" section in the release notes.
pub(super) fn synthetic_change(
    policy: InternalDependencyUpdate,
    bumps: &[BumpedDependency],
) -> Option<Change> {
    match policy {
        InternalDependencyUpdate::None => None,
        InternalDependencyUpdate::Patch | InternalDependencyUpdate::Minor => {
            let change_type = match policy {
                InternalDependencyUpdate::Minor => ChangeType::Feature,
                _ => ChangeType::Fix,
            };
            let details = bumps
                .iter()
                .map(|bump| format!("  - {name}@{version}", name = bump.name, version = bump.new_version))
                .collect::<Vec<_>>()
                .join("\n");
            Some(Change {
                change_type,
                summary: "Updated dependencies".to_string(),
                details: Some(details),
                original_source: ChangeSource::DependencyUpdate,
                git: None,
            })
        }
    }
}
