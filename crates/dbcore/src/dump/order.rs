//! Dependency order for objects that must be created after the ones they use (views on views,
//! partitions after their parents…).

use std::collections::{BTreeSet, HashMap};
use std::hash::Hash;

/// Sorts `items` so every item comes after the items it depends on (`deps`: item → dependency),
/// keeping the original order otherwise. Dependencies outside `items` are ignored, and items in a
/// cycle keep their original relative order at the end.
pub(crate) fn topo_sort<K: Eq + Hash + Clone>(items: &[K], deps: &[(K, K)]) -> Vec<K> {
    let index: HashMap<&K, usize> = items.iter().enumerate().map(|(i, k)| (k, i)).collect();
    let mut blockers = vec![0usize; items.len()];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); items.len()];
    let mut seen = BTreeSet::new();
    for (item, dependency) in deps {
        let (Some(&i), Some(&d)) = (index.get(item), index.get(dependency)) else { continue };
        if i == d || !seen.insert((i, d)) {
            continue;
        }
        blockers[i] += 1;
        dependents[d].push(i);
    }
    // Always take the earliest ready item, so the result stays close to the input order.
    let mut ready: BTreeSet<usize> = (0..items.len()).filter(|&i| blockers[i] == 0).collect();
    let mut sorted = Vec::with_capacity(items.len());
    let mut done = vec![false; items.len()];
    while let Some(i) = ready.pop_first() {
        sorted.push(i);
        done[i] = true;
        for &dependent in &dependents[i] {
            blockers[dependent] -= 1;
            if blockers[dependent] == 0 {
                ready.insert(dependent);
            }
        }
    }
    sorted.extend((0..items.len()).filter(|&i| !done[i]));
    sorted.into_iter().map(|i| items[i].clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_dependencies_first() {
        let items = ["c", "b", "a", "d"];
        let deps = [("c", "b"), ("b", "a"), ("c", "zzz")];
        assert_eq!(topo_sort(&items, &deps), ["a", "b", "c", "d"]);
        assert_eq!(topo_sort(&items, &[]), items);
    }

    #[test]
    fn keeps_cycles_at_the_end() {
        let items = ["x", "y", "z"];
        let deps = [("x", "y"), ("y", "x")];
        assert_eq!(topo_sort(&items, &deps), ["z", "x", "y"]);
    }
}
