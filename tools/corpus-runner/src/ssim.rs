//! The structural similarity of two grey pages (Wang, Bovik, Sheikh and Simoncelli, "Image
//! quality assessment: from error visibility to structural similarity", IEEE Transactions
//! on Image Processing 13(4), 2004): an 11×11 Gaussian window with σ = 1.5, K1 = 0.01,
//! K2 = 0.03, L = 255, averaged over every position where the window fits.

use crate::raster::Grey;

const WINDOW: usize = 11;
const SIGMA: f64 = 1.5;
const C1: f64 = (0.01 * 255.0) * (0.01 * 255.0);
const C2: f64 = (0.03 * 255.0) * (0.03 * 255.0);

/// The SSIM of two pages. The smaller one is padded with white to the size of the larger,
/// as a page of another size shows on paper; pages smaller than the window are padded to
/// it.
pub fn ssim(a: &Grey, b: &Grey) -> f64 {
    let width = a.width.max(b.width).max(WINDOW);
    let height = a.height.max(b.height).max(WINDOW);
    let weights = gaussian();
    let plane = |image: &Grey| -> Vec<f64> {
        let mut out = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                out.push(f64::from(image.at(x, y)));
            }
        }
        out
    };
    let (x, y) = (plane(a), plane(b));
    let product =
        |p: &[f64], q: &[f64]| -> Vec<f64> { p.iter().zip(q).map(|(a, b)| a * b).collect() };
    let blur = |values: &[f64]| filter(values, width, height, &weights);
    let (mean_x, mean_y) = (blur(&x), blur(&y));
    let (xx, yy, xy) = (
        blur(&product(&x, &x)),
        blur(&product(&y, &y)),
        blur(&product(&x, &y)),
    );
    let count = mean_x.len();
    let mut total = 0.0;
    for (((mx, my), (sxx, syy)), sxy) in mean_x.iter().zip(&mean_y).zip(xx.iter().zip(&yy)).zip(&xy)
    {
        let variance_x = sxx - mx * mx;
        let variance_y = syy - my * my;
        let covariance = sxy - mx * my;
        total += ((2.0 * mx * my + C1) * (2.0 * covariance + C2))
            / ((mx * mx + my * my + C1) * (variance_x + variance_y + C2));
    }
    total / count as f64
}

/// The normalised 11-tap Gaussian.
fn gaussian() -> [f64; WINDOW] {
    let mut weights = [0.0; WINDOW];
    let centre = (WINDOW / 2) as f64;
    for (index, weight) in weights.iter_mut().enumerate() {
        let distance = index as f64 - centre;
        *weight = (-(distance * distance) / (2.0 * SIGMA * SIGMA)).exp();
    }
    let sum: f64 = weights.iter().sum();
    weights.map(|weight| weight / sum)
}

/// The values filtered by the window, separably, at every position where it fits: a
/// (width − 10) × (height − 10) result.
fn filter(values: &[f64], width: usize, height: usize, weights: &[f64; WINDOW]) -> Vec<f64> {
    let out_width = width + 1 - WINDOW;
    let out_height = height + 1 - WINDOW;
    let mut rows = Vec::with_capacity(out_width * height);
    for line in values.chunks_exact(width) {
        for window in line.windows(WINDOW) {
            rows.push(window.iter().zip(weights).map(|(v, w)| v * w).sum::<f64>());
        }
    }
    let mut out = vec![0.0; out_width * out_height];
    for (row, weight) in weights.iter().enumerate() {
        for (target, source) in out
            .chunks_exact_mut(out_width)
            .zip(rows.chunks_exact(out_width).skip(row))
        {
            for (t, s) in target.iter_mut().zip(source) {
                *t += s * weight;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(width: usize, height: usize, pixel: impl Fn(usize, usize) -> u8) -> Grey {
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                pixels.push(pixel(x, y));
            }
        }
        Grey {
            width,
            height,
            pixels,
        }
    }

    /// Black text-like bars on white.
    fn text(x: usize, y: usize) -> u8 {
        if y % 12 < 7 && x % 9 < 6 && (5..55).contains(&x) && (4..56).contains(&y) {
            0
        } else {
            255
        }
    }

    #[test]
    fn identical_pages_score_one() {
        let a = page(60, 60, text);
        assert!((ssim(&a, &a) - 1.0).abs() < 1e-12);
        let white = page(20, 20, |_, _| 255);
        assert!((ssim(&white, &white) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn a_shift_scores_lower_the_further_it_goes() {
        let a = page(60, 60, text);
        let one = page(60, 60, |x, y| if y == 0 { 255 } else { text(x, y - 1) });
        let three = page(60, 60, |x, y| if y < 3 { 255 } else { text(x, y - 3) });
        let (s1, s3) = (ssim(&a, &one), ssim(&a, &three));
        assert!(s1 < 0.95 && s3 < s1, "{s1} {s3}");
        // From a direct 11×11 double sum with the paper's constants, outside this crate.
        assert!((s1 - 0.644_953_586_554).abs() < 1e-9, "{s1}");
        assert!((ssim(&one, &a) - s1).abs() < 1e-12, "symmetric");
    }

    #[test]
    fn a_missing_part_of_a_larger_page_counts_as_white() {
        let small = page(60, 60, text);
        let large = page(60, 80, |x, y| if y < 60 { text(x, y) } else { 255 });
        assert!((ssim(&small, &large) - 1.0).abs() < 1e-12);
        let inked = page(60, 80, |x, y| if y < 60 { text(x, y) } else { 0 });
        assert!(ssim(&small, &inked) < 0.9);
    }

    #[test]
    fn the_window_is_normalised_and_symmetric() {
        let weights = gaussian();
        assert!((weights.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        assert!((weights[0] - weights[10]).abs() < 1e-15 && weights[5] > weights[4]);
    }

    /// The filter against a direct two-dimensional sum at a few positions.
    #[test]
    fn the_separable_filter_is_the_two_dimensional_one() {
        let (width, height) = (17, 14);
        let values: Vec<f64> = (0..width * height)
            .map(|i| ((i * 37) % 101) as f64)
            .collect();
        let weights = gaussian();
        let out = filter(&values, width, height, &weights);
        assert_eq!(out.len(), (width - 10) * (height - 10));
        for (ox, oy) in [(0, 0), (6, 3), (3, 2)] {
            let mut direct = 0.0;
            for (dy, wy) in weights.iter().enumerate() {
                for (dx, wx) in weights.iter().enumerate() {
                    direct += values[(oy + dy) * width + ox + dx] * wx * wy;
                }
            }
            assert!((out[oy * (width - 10) + ox] - direct).abs() < 1e-9);
        }
    }
}
