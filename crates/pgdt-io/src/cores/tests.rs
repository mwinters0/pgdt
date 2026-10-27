use crate::test_support::*;

use super::*;

/// Four cores of two threads each, CPU `n` and `n + 4` sharing one, as
/// x86's SMT numbering lays them out, under the affinity `allowed`.
fn smt_machine(allowed: &str) -> FakeRoot {
    let root = FakeRoot::new();
    root.write("proc/self/status", &format!("Name:\tpgdt\nCpus_allowed_list:\t{allowed}\n"));
    for cpu in 0..8 {
        let core = cpu % 4;
        root.write(
            &format!("sys/devices/system/cpu/cpu{cpu}/topology/core_cpus_list"),
            &format!("{core},{}\n", core + 4),
        );
    }
    root
}

/// **A core counts once whichever of its threads the affinity allows**
/// (`RT27`): the whole machine is its four cores, and a cpuset of three CPUs,
/// two of them one core's siblings, is two.
#[test]
fn a_core_counts_once_whichever_of_its_threads_are_allowed() {
    assert_eq!(physical_cores_in(smt_machine("0-7").path()), Some(4));
    assert_eq!(physical_cores_in(smt_machine("0,4,1").path()), Some(2));
    assert_eq!(physical_cores_in(smt_machine("0-3").path()), Some(4));
    assert_eq!(physical_cores_in(smt_machine("6").path()), Some(1));
}

/// **The older name is read where the newer is absent, and a CPU with no
/// topology is skipped** (`RT27`): an offline CPU's directory has none.
#[test]
fn the_older_name_is_read_and_a_cpu_without_a_topology_is_skipped() {
    let root = FakeRoot::new();
    root.write("proc/self/status", "Cpus_allowed_list:\t0-2\n")
        .write("sys/devices/system/cpu/cpu0/topology/thread_siblings_list", "0,1\n")
        .write("sys/devices/system/cpu/cpu1/topology/thread_siblings_list", "0,1\n")
        .write("sys/devices/system/cpu/cpu2/online", "0\n");
    assert_eq!(physical_cores_in(root.path()), Some(1));
}

/// **Nothing readable is `None`, never zero**, so the caller keeps the count
/// it had: no `/proc`, no affinity line, a list that is not one, or no
/// allowed CPU's topology.
#[test]
fn nothing_readable_is_none() {
    assert_eq!(physical_cores_in(FakeRoot::new().path()), None);
    let root = FakeRoot::new();
    root.write("proc/self/status", "Name:\tpgdt\n");
    assert_eq!(physical_cores_in(root.path()), None);
    assert_eq!(physical_cores_in(smt_machine("0-x").path()), None);
    assert_eq!(physical_cores_in(smt_machine("9-12").path()), None);
}

#[test]
fn a_cpu_list_is_its_cpus_and_ranges() {
    assert_eq!(cpu_list("0-3,8,10-11"), Some(vec![0, 1, 2, 3, 8, 10, 11]));
    assert_eq!(cpu_list("5"), Some(vec![5]));
    assert_eq!(cpu_list(""), None);
    assert_eq!(cpu_list("3-1"), None);
}
