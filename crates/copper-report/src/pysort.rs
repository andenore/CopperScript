//! `list.sort(key=...)` with Python's comparison sequence.
//!
//! Sorting loosely typed report values can raise (`'<' not supported between
//! instances of 'int' and 'str'`), and which pair raises first depends on the
//! order CPython compares them in. Lists shorter than 64 items are sorted by
//! CPython 3.13's `count_run` and binary insertion exactly; longer ones by a
//! stable merge sort, which agrees with CPython whenever no comparison raises.

use crate::py::Py;

const MAX_MINRUN: usize = 64;

/// Sort `items` by their `keys` (one per item) with the fallible `less`.
pub fn sort_by_keys<T, K>(items: &mut Vec<T>, keys: Vec<K>, mut less: impl FnMut(&K, &K) -> Py<bool>) -> Py<()> {
    assert_eq!(items.len(), keys.len());
    let mut slots: Vec<(K, T)> = keys.into_iter().zip(items.drain(..)).collect();
    let result =
        if slots.len() < MAX_MINRUN { small_sort(&mut slots, &mut less) } else { merge_sort(&mut slots, &mut less) };
    items.extend(slots.into_iter().map(|(_, item)| item));
    result
}

/// CPython's path for fewer than `MAX_MINRUN` items: one natural run, then
/// binary insertion of the rest.
fn small_sort<K, T>(slots: &mut [(K, T)], less: &mut impl FnMut(&K, &K) -> Py<bool>) -> Py<()> {
    if slots.len() < 2 {
        return Ok(());
    }
    let run = count_run(slots, less)?;
    binary_sort(slots, run, less)
}

fn count_run<K, T>(slots: &mut [(K, T)], less: &mut impl FnMut(&K, &K) -> Py<bool>) -> Py<usize> {
    let total = slots.len();
    let mut n = 1;
    while n < total {
        if less(&slots[n].0, &slots[n - 1].0)? {
            break;
        }
        n += 1;
    }
    if n == total {
        return Ok(n);
    }
    if n > 1 {
        if less(&slots[0].0, &slots[n - 1].0)? {
            return Ok(n);
        }
        slots[..n].reverse();
    }
    n += 1;
    let mut equal = 0;
    let reverse_last_equal = |slots: &mut [(K, T)], n: usize, equal: &mut usize| {
        if *equal > 0 {
            let count = *equal + 1;
            slots[n - count..n].reverse();
            *equal = 0;
        }
    };
    while n < total {
        if less(&slots[n].0, &slots[n - 1].0)? {
            reverse_last_equal(slots, n, &mut equal);
        } else if less(&slots[n - 1].0, &slots[n].0)? {
            break;
        } else {
            equal += 1;
        }
        n += 1;
    }
    reverse_last_equal(slots, n, &mut equal);
    slots[..n].reverse();
    while n < total {
        if less(&slots[n].0, &slots[n - 1].0)? {
            break;
        }
        n += 1;
    }
    Ok(n)
}

fn binary_sort<K, T>(slots: &mut [(K, T)], sorted: usize, less: &mut impl FnMut(&K, &K) -> Py<bool>) -> Py<()> {
    for ok in sorted.max(1)..slots.len() {
        let (mut low, mut high) = (0, ok);
        while low < high {
            let middle = (low + high) >> 1;
            if less(&slots[ok].0, &slots[middle].0)? {
                high = middle;
            } else {
                low = middle + 1;
            }
        }
        slots[low..=ok].rotate_right(1);
    }
    Ok(())
}

fn merge_sort<K, T>(slots: &mut Vec<(K, T)>, less: &mut impl FnMut(&K, &K) -> Py<bool>) -> Py<()> {
    if slots.len() < 2 {
        return Ok(());
    }
    let mut right = slots.split_off(slots.len() / 2);
    merge_sort(slots, less)?;
    merge_sort(&mut right, less)?;
    let left = std::mem::take(slots);
    let (mut left, mut right) = (left.into_iter().peekable(), right.into_iter().peekable());
    while let (Some(a), Some(b)) = (left.peek(), right.peek()) {
        // Stable: take from the right only when it is strictly less.
        let next = if less(&b.0, &a.0)? { right.next() } else { left.next() };
        slots.extend(next);
    }
    slots.extend(left);
    slots.extend(right);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(values: &[i32]) -> Vec<i32> {
        let mut items: Vec<usize> = (0..values.len()).collect();
        sort_by_keys(&mut items, values.to_vec(), |a, b| Ok(a < b)).unwrap();
        items.into_iter().map(|index| values[index]).collect()
    }

    #[test]
    fn sorts_stably() {
        assert_eq!(sorted(&[3, 1, 2, 1, 0, 5, 5, 4]), [0, 1, 1, 2, 3, 4, 5, 5]);
        assert_eq!(sorted(&[5, 4, 4, 3, 1]), [1, 3, 4, 4, 5]);
        let long: Vec<i32> = (0..200).map(|i| (i * 37) % 101).collect();
        let mut expected = long.clone();
        expected.sort();
        assert_eq!(sorted(&long), expected);
        // Equal keys keep their order.
        let mut items = vec!["a", "b", "c", "d"];
        sort_by_keys(&mut items, vec![1, 0, 1, 0], |a, b| Ok(a < b)).unwrap();
        assert_eq!(items, ["b", "d", "a", "c"]);
    }
}
