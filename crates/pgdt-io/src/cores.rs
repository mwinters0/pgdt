//! The physical cores among the CPUs this process may run on, which a local
//! gzip first pass caps its recommendation at (`docs/design/decisions.md`,
//! "D2").

use std::collections::BTreeSet;
use std::path::Path;

/// How many physical cores hold the CPUs this process's affinity allows: each
/// allowed CPU's core read as the hardware threads sharing it, and the
/// distinct cores counted, so a core counts once whether one of its threads
/// is allowed or both (`docs/design/runtime-invariants.md`, `RT27`).
///
/// **`None` where nothing can be read** — no `/proc`, or no allowed CPU's
/// topology, as off Linux — and the caller keeps the count it had. A CPU
/// whose topology alone is missing, as an offline one's is, is skipped.
pub(crate) fn physical_cores() -> Option<usize> {
    physical_cores_in(Path::new("/"))
}

/// [`physical_cores`] against a root a test built.
pub(crate) fn physical_cores_in(root: &Path) -> Option<usize> {
    let status = std::fs::read_to_string(root.join("proc/self/status")).ok()?;
    let allowed = status.lines().find_map(|line| line.strip_prefix("Cpus_allowed_list:"))?;
    let cpus = root.join("sys/devices/system/cpu");
    let cores: BTreeSet<Vec<u32>> = cpu_list(allowed.trim())?
        .into_iter()
        .filter_map(|cpu| {
            let topology = cpus.join(format!("cpu{cpu}/topology"));
            // `thread_siblings_list` is the older name for the same list.
            ["core_cpus_list", "thread_siblings_list"]
                .iter()
                .find_map(|name| std::fs::read_to_string(topology.join(name)).ok())
                .and_then(|list| cpu_list(list.trim()))
        })
        .collect();
    (!cores.is_empty()).then_some(cores.len())
}

/// A kernel CPU list, `0-3,8,10-11`, as its CPUs in order; `None` where any
/// part of it is not a CPU or a range of them.
fn cpu_list(list: &str) -> Option<Vec<u32>> {
    let mut cpus = Vec::new();
    for part in list.split(',') {
        let (first, last): (u32, u32) = match part.split_once('-') {
            Some((first, last)) => (first.parse().ok()?, last.parse().ok()?),
            None => {
                let cpu = part.parse().ok()?;
                (cpu, cpu)
            }
        };
        if first > last {
            return None;
        }
        cpus.extend(first..=last);
    }
    Some(cpus)
}

#[cfg(test)]
mod tests;
