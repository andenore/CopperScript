//! Raster pour connectivity (`pcbir.plane_estimate`): the component
//! labelling the unified router and supply islands share. The estimate
//! itself (`estimate_plane_connectivity`) is not rewritten yet.

/// `_label`: 4-connected component labels of a row-major `(ny, nx)` mask via
/// row runs and union-find. Labels are root run ids (0 outside the mask);
/// the count is the number of run ids plus one, as Python returns it.
pub fn label(mask: &[bool], ny: usize, nx: usize) -> (Vec<i32>, usize) {
    let mut labels = vec![0i32; ny * nx];
    let mut parent: Vec<usize> = vec![0];
    fn find(parent: &mut [usize], mut a: usize) -> usize {
        while parent[a] != a {
            parent[a] = parent[parent[a]];
            a = parent[a];
        }
        a
    }
    let mut previous: Vec<(usize, usize, usize)> = Vec::new();
    for y in 0..ny {
        let row = &mask[y * nx..(y + 1) * nx];
        if !row.iter().any(|&v| v) {
            previous.clear();
            continue;
        }
        let mut runs = Vec::new();
        let mut j = 0;
        let mut x = 0;
        while x < nx {
            if !row[x] {
                x += 1;
                continue;
            }
            let start = x;
            while x < nx && row[x] {
                x += 1;
            }
            let end = x;
            let label = parent.len();
            parent.push(label);
            while j < previous.len() && previous[j].1 <= start {
                j += 1;
            }
            let mut k = j;
            while k < previous.len() && previous[k].0 < end {
                let (a, b) = (find(&mut parent, label), find(&mut parent, previous[k].2));
                if a != b {
                    parent[a.max(b)] = a.min(b);
                }
                k += 1;
            }
            for cell in &mut labels[y * nx + start..y * nx + end] {
                *cell = label as i32;
            }
            runs.push((start, end, label));
        }
        previous = runs;
    }
    let roots: Vec<i32> = (0..parent.len()).map(|i| find(&mut parent, i) as i32).collect();
    (labels.iter().map(|&l| roots[l as usize]).collect(), parent.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_runs_across_rows() {
        let mask = [true, false, true, true, true, true];
        let (labels, count) = label(&mask, 2, 3);
        assert_eq!(count, 4);
        assert_eq!(labels, vec![1, 0, 1, 1, 1, 1]);
    }
}
