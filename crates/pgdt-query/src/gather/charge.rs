//! What a gatherer's vectors and maps hold, charged to its account ahead of
//! the allocation growing them (`docs/design/decisions.md`, "D81").

use std::mem::{align_of, size_of};

use crate::statistics::Charge;

/// Make room in `vec` for `additional` more, **charging the growth ahead of
/// the allocation** ([`Charge::ahead`]): at least doubling, as the vector's
/// own growth would, but reserved exactly so the charge is the capacity.
pub(super) fn reserve_charged<T>(vec: &mut Vec<T>, additional: usize, charge: &mut Charge) {
    let wanted = vec.len() + additional;
    let capacity = vec.capacity();
    if wanted <= capacity {
        return;
    }
    let grown = wanted.max(capacity * 2).max(4);
    let bytes = ((grown - capacity) * size_of::<T>()) as u64;
    charge.ahead((bytes, 0), (0, 0), || vec.reserve_exact(grown - vec.len()));
}

/// Push `item` onto `vec`, charging a growth it forces ahead of it.
pub(super) fn push_charged<T>(vec: &mut Vec<T>, item: T, charge: &mut Charge) {
    reserve_charged(vec, 1, charge);
    vec.push(item);
}

/// The heap a `HashMap<K, V>` of `capacity` allocates, as the standard
/// library's table lays one out: a slot per bucket, then a control byte per
/// bucket and one probe group's worth past the end. The layout is the
/// toolchain's, not a guarantee (`docs/design/runtime-invariants.md`, "RT11").
pub(super) fn map_heap<K, V>(capacity: usize) -> u64 {
    if capacity == 0 {
        return 0;
    }
    let buckets = if capacity < 8 { capacity + 1 } else { (capacity * 8 / 7).next_power_of_two() };
    table_heap::<K, V>(buckets)
}

/// The heap the table a full `HashMap<K, V>` of `capacity` grows into
/// allocates: the table sized for one more entry than it holds.
pub(super) fn grown_map_heap<K, V>(capacity: usize) -> u64 {
    let wanted = capacity + 1;
    let buckets = if wanted < 15 {
        match wanted.max(3) {
            3 => 4,
            4..=7 => 8,
            _ => 16,
        }
    } else {
        (wanted * 8 / 7).next_power_of_two()
    };
    table_heap::<K, V>(buckets)
}

fn table_heap<K, V>(buckets: usize) -> u64 {
    let probe = if cfg!(any(target_arch = "x86", target_arch = "x86_64")) { 16 } else { 8 };
    let align = align_of::<(K, V)>().max(probe);
    let slots = (size_of::<(K, V)>() * buckets).next_multiple_of(align);
    (slots + buckets + probe) as u64
}
