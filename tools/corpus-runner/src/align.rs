//! Word alignment: a longest common subsequence of two sequences, by Myers' difference
//! algorithm in linear space (E. W. Myers, Algorithmica 1, 1986, section 4b), whose time
//! grows with the length of the sequences times the number of differences.

/// Pairs (index in `a`, index in `b`) of equal items, increasing in both: a longest common
/// subsequence.
pub fn matches<T: PartialEq>(a: &[T], b: &[T]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    align(a, b, 0, 0, &mut out);
    out
}

fn align<T: PartialEq>(a: &[T], b: &[T], a0: usize, b0: usize, out: &mut Vec<(usize, usize)>) {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    out.extend((0..prefix).map(|i| (a0 + i, b0 + i)));
    let (a, b) = (
        a.get(prefix..).unwrap_or_default(),
        b.get(prefix..).unwrap_or_default(),
    );
    let suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (a, b) = (
        a.get(..a.len() - suffix).unwrap_or_default(),
        b.get(..b.len() - suffix).unwrap_or_default(),
    );
    let (a0, b0) = (a0 + prefix, b0 + prefix);
    if !a.is_empty() && !b.is_empty() {
        // Both differ at their ends, so the edit script has at least two steps and each
        // half of the split has fewer.
        let (x, y, u, v) = middle_snake(a, b);
        let before = (a.get(..x), b.get(..y));
        let after = (a.get(u..), b.get(v..));
        // Each half must be smaller than the whole, which Myers' bound guarantees; a split
        // that is not would recurse for ever, so it ends the alignment of this part.
        let splits = (x, y) != (a.len(), b.len()) && (u, v) != (0, 0);
        if let ((Some(a1), Some(b1)), (Some(a2), Some(b2)), true) = (before, after, splits) {
            align(a1, b1, a0, b0, out);
            out.extend((0..u - x).map(|i| (a0 + x + i, b0 + y + i)));
            align(a2, b2, a0 + u, b0 + v, out);
        }
    }
    let (a1, b1) = (a0 + a.len(), b0 + b.len());
    out.extend((0..suffix).map(|i| (a1 + i, b1 + i)));
}

/// The middle snake of a shortest edit script: (x, y) to (u, v), where the forward and the
/// backward search meet.
// reason: every diagonal index stays within -(max + 1)..=max + 1, the vectors' range, and the
// items are read only at x < n and y < m.
#[allow(clippy::indexing_slicing)]
fn middle_snake<T: PartialEq>(a: &[T], b: &[T]) -> (usize, usize, usize, usize) {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let delta = n - m;
    let odd = delta % 2 != 0;
    let max = (n + m + 1) / 2;
    let offset = max + 1;
    let at = |k: isize| (k + offset) as usize;
    let mut forward = vec![0isize; (2 * max + 3) as usize];
    let mut backward = forward.clone();
    for d in 0..=max {
        for k in (-d..=d).step_by(2) {
            let mut x = if k == -d || (k != d && forward[at(k - 1)] < forward[at(k + 1)]) {
                forward[at(k + 1)]
            } else {
                forward[at(k - 1)] + 1
            };
            let mut y = x - k;
            let (x0, y0) = (x, y);
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            forward[at(k)] = x;
            if odd && (-(d - 1)..=d - 1).contains(&(k - delta)) && x + backward[at(delta - k)] >= n
            {
                return (x0 as usize, y0 as usize, x as usize, y as usize);
            }
        }
        for k in (-d..=d).step_by(2) {
            // In the reversed sequences: x counts items from the end.
            let mut x = if k == -d || (k != d && backward[at(k - 1)] < backward[at(k + 1)]) {
                backward[at(k + 1)]
            } else {
                backward[at(k - 1)] + 1
            };
            let mut y = x - k;
            let (x0, y0) = (x, y);
            while x < n && y < m && a[(n - 1 - x) as usize] == b[(m - 1 - y) as usize] {
                x += 1;
                y += 1;
            }
            backward[at(k)] = x;
            if !odd && (-d..=d).contains(&(delta - k)) && x + forward[at(delta - k)] >= n {
                return (
                    (n - x) as usize,
                    (m - y) as usize,
                    (n - x0) as usize,
                    (m - y0) as usize,
                );
            }
        }
    }
    // The searches always meet by d = max; an empty snake at the start splits nothing.
    (0, 0, 0, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The length of a longest common subsequence by dynamic programming.
    fn lcs(a: &[u8], b: &[u8]) -> usize {
        let mut table = vec![vec![0usize; b.len() + 1]; a.len() + 1];
        for i in 0..a.len() {
            for j in 0..b.len() {
                table[i + 1][j + 1] = if a[i] == b[j] {
                    table[i][j] + 1
                } else {
                    table[i][j + 1].max(table[i + 1][j])
                };
            }
        }
        table[a.len()][b.len()]
    }

    fn check(a: &[u8], b: &[u8]) {
        let found = matches(a, b);
        assert_eq!(found.len(), lcs(a, b), "{a:?} {b:?}");
        for pair in found.windows(2) {
            assert!(
                pair[0].0 < pair[1].0 && pair[0].1 < pair[1].1,
                "{a:?} {b:?} {found:?}"
            );
        }
        for &(i, j) in &found {
            assert_eq!(a[i], b[j]);
        }
    }

    #[test]
    fn small_cases() {
        check(b"", b"");
        check(b"abc", b"");
        check(b"", b"abc");
        check(b"abcabba", b"cbabac");
        check(b"xyz", b"abc");
        check(b"a", b"xa");
        check(b"xy", b"yx");
        check(b"the cat sat", b"the hat sat on");
    }

    /// Random sequences over small alphabets, against the dynamic programming length.
    #[test]
    fn random_sequences_give_a_longest_common_subsequence() {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for round in 0..3000 {
            let alphabet = 2 + (round % 5) as u64;
            let a: Vec<u8> = (0..next() % 25)
                .map(|_| (next() % alphabet) as u8)
                .collect();
            let b: Vec<u8> = if round % 3 == 0 {
                // A lightly edited copy, as two renderings of one text are.
                let mut b = a.clone();
                for _ in 0..next() % 4 {
                    let at = (next() as usize) % (b.len() + 1);
                    if next() % 2 == 0 && at < b.len() {
                        b.remove(at);
                    } else {
                        b.insert(at, (next() % alphabet) as u8);
                    }
                }
                b
            } else {
                (0..next() % 25)
                    .map(|_| (next() % alphabet) as u8)
                    .collect()
            };
            check(&a, &b);
        }
    }

    #[test]
    fn long_similar_sequences_are_fast() {
        let a: Vec<u32> = (0..20_000).map(|i| i % 997).collect();
        let mut b = a.clone();
        b.drain(5_000..5_040);
        b.insert(12_000, 5_000);
        let started = std::time::Instant::now();
        assert_eq!(matches(&a, &b).len(), a.len() - 40);
        assert!(started.elapsed().as_secs() < 5);
    }
}
