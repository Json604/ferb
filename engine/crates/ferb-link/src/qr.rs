//! Drawing a QR code so a phone reads it at the first look. The code itself comes from the `qrcode` crate; chosen
//! here: the error-correction level, the mask, and the terminal drawing.
//!
//! The mask. A reader finds a code by its three corner squares: dark-light-dark-light-dark runs of 1:1:3:1:1 in
//! both directions. Data can show the same runs by chance. ZXing 3.4 (the reader in the Android app) scans from
//! the top, and once it has the two upper squares it takes the next thing that passes as the third: a chance
//! look-alike above the real lower-left square makes it read nothing at that angle, until the phone is turned.
//! A QR code may be drawn with any of eight masks; the standard's own scoring only discourages look-alikes. So
//! the mask is chosen by running the reader's own test over each of the eight drawings, upright and in the three
//! quarter turns, and taking one with no look-alike, or the fewest.

use qrcode::{
    EcLevel,
    canvas::{Canvas, MaskPattern},
};

/// The error-correction level of every code FERB draws.
pub const LEVEL: EcLevel = EcLevel::M;
/// The white margin around a code, in modules (what the standard asks for).
pub const QUIET: i32 = 4;

const MASKS: [MaskPattern; 8] = [
    MaskPattern::Checkerboard,
    MaskPattern::HorizontalLines,
    MaskPattern::VerticalLines,
    MaskPattern::DiagonalLines,
    MaskPattern::LargeCheckerboard,
    MaskPattern::Fields,
    MaskPattern::Diamonds,
    MaskPattern::Meadow,
];

/// A QR code as rows of `1` (dark) and `0`, without the margin: one drawing for every surface that shows it.
pub fn rows(data: &str) -> Vec<String> {
    rows_at(data, LEVEL, true)
}
/// `careful`: choose the mask as described above; without, the crate's own choice (tests compare the two).
pub fn rows_at(data: &str, level: EcLevel, careful: bool) -> Vec<String> {
    let Some((w, cells)) = matrix(data, level, careful) else {
        return vec![];
    };
    cells
        .chunks(w)
        .map(|row| row.iter().map(|d| if *d { '1' } else { '0' }).collect())
        .collect()
}

fn matrix(data: &str, level: EcLevel, careful: bool) -> Option<(usize, Vec<bool>)> {
    let bits = qrcode::bits::encode_auto(data.as_bytes(), level).ok()?;
    let version = bits.version();
    let (codewords, ec) =
        qrcode::ec::construct_codewords(&bits.into_bytes(), version, level).ok()?;
    let mut blank = Canvas::new(version, level);
    blank.draw_all_functional_patterns();
    blank.draw_data(&codewords, &ec);
    let w = version.width() as usize;
    let dark = |c: Canvas| {
        c.into_colors()
            .into_iter()
            .map(|c| c == qrcode::Color::Dark)
            .collect::<Vec<bool>>()
    };
    let usual = dark(blank.apply_best_mask());
    if !careful {
        return Some((w, usual));
    }
    let mut best: Option<(usize, bool, Vec<bool>)> = None;
    for mask in MASKS {
        let mut c = blank.clone();
        c.apply_mask(mask);
        let cells = dark(c);
        // fewest look-alikes; among equals the standard's own choice
        let key = (lookalikes(w, &cells), cells != usual);
        if best.as_ref().is_none_or(|b| key < (b.0, b.1)) {
            best = Some((key.0, key.1, cells));
        }
    }
    best.map(|b| (w, b.2))
}

/// How many places in this drawing, other than its three corner squares, ZXing's finder would take for a corner
/// square (`FinderPatternFinder` of ZXing 3.4.1: the run test along a row, then down the column through the
/// middle, along the row again, and along the diagonal). Counted on the drawing with its margin, four picture
/// points per module, so a middle that falls between two modules is judged as the reader would.
pub fn lookalikes(w: usize, cells: &[bool]) -> usize {
    // the reader goes row by row and rounds a middle down, so a quarter turn is not quite the same test: all four
    let mut turned = cells.to_vec();
    let far = w as f32 - 3.5;
    let mut corners = [(3.5, 3.5), (far, 3.5), (3.5, far)];
    let mut n = 0;
    for _ in 0..4 {
        n += lookalikes_upright(w, &turned, &corners);
        turned = (0..w * w)
            .map(|i| turned[(w - 1 - i % w) * w + i / w])
            .collect();
        corners = corners.map(|(x, y)| (w as f32 - y, x));
    }
    n
}
fn lookalikes_upright(w: usize, cells: &[bool], corners: &[(f32, f32); 3]) -> usize {
    const S: i32 = 4;
    let side = (w as i32 + 2 * QUIET) * S;
    let black = |x: i32, y: i32| {
        let (mx, my) = (x.div_euclid(S) - QUIET, y.div_euclid(S) - QUIET);
        mx >= 0 && my >= 0 && mx < w as i32 && my < w as i32 && cells[my as usize * w + mx as usize]
    };
    let cross = |c: &[i32; 5], slack: f32| {
        let total: i32 = c.iter().sum();
        if total < 7 || c.contains(&0) {
            return false;
        }
        let m = total as f32 / 7.0;
        let max = m / slack;
        (m - c[0] as f32).abs() < max
            && (m - c[1] as f32).abs() < max
            && (3.0 * m - c[2] as f32).abs() < 3.0 * max
            && (m - c[3] as f32).abs() < max
            && (m - c[4] as f32).abs() < max
    };
    // the runs through (x, y) along (dx, dy), as the reader counts them; None when it gives up
    let through =
        |x: i32, y: i32, dx: i32, dy: i32, max: i32, original: Option<(i32, i32)>| -> Option<f32> {
            let inside = |x: i32, y: i32| x >= 0 && y >= 0 && x < side && y < side;
            let mut c = [0i32; 5];
            let (mut i, mut j) = (x, y);
            while inside(i, j) && black(i, j) {
                c[2] += 1;
                i -= dx;
                j -= dy;
            }
            if !inside(i, j) {
                return None;
            }
            while inside(i, j) && !black(i, j) && c[1] <= max {
                c[1] += 1;
                i -= dx;
                j -= dy;
            }
            if !inside(i, j) || c[1] > max {
                return None;
            }
            while inside(i, j) && black(i, j) && c[0] <= max {
                c[0] += 1;
                i -= dx;
                j -= dy;
            }
            if c[0] > max {
                return None;
            }
            let (mut i, mut j) = (x + dx, y + dy);
            while inside(i, j) && black(i, j) {
                c[2] += 1;
                i += dx;
                j += dy;
            }
            if !inside(i, j) {
                return None;
            }
            while inside(i, j) && !black(i, j) && c[3] < max {
                c[3] += 1;
                i += dx;
                j += dy;
            }
            if !inside(i, j) || c[3] >= max {
                return None;
            }
            while inside(i, j) && black(i, j) && c[4] < max {
                c[4] += 1;
                i += dx;
                j += dy;
            }
            if c[4] >= max {
                return None;
            }
            let total: i32 = c.iter().sum();
            if let Some((was, times)) = original
                && 5 * (total - was).abs() >= times * was
            {
                return None;
            }
            let end = if dx != 0 { i } else { j };
            cross(&c, if dx != 0 && dy != 0 { 1.333 } else { 2.0 })
                .then(|| (end - c[4] - c[3]) as f32 - c[2] as f32 / 2.0)
        };
    let real = |x: f32, y: f32| {
        corners.iter().any(|(cx, cy)| {
            (x - (cx + QUIET as f32) * S as f32).abs() < 3.0 * S as f32
                && (y - (cy + QUIET as f32) * S as f32).abs() < 3.0 * S as f32
        })
    };
    let mut found: Vec<(f32, f32)> = vec![];
    for y in (S / 2..side).step_by(S as usize) {
        // the runs of this row
        let mut runs: Vec<(bool, i32, i32)> = vec![]; // (dark, start, length)
        for x in 0..side {
            let d = black(x, y);
            match runs.last_mut() {
                Some(r) if r.0 == d => r.2 += 1,
                _ => runs.push((d, x, 1)),
            }
        }
        for win in runs.windows(5).filter(|w| w[0].0) {
            let c = [win[0].2, win[1].2, win[2].2, win[3].2, win[4].2];
            if !cross(&c, 2.0) {
                continue;
            }
            let total: i32 = c.iter().sum();
            let end = win[4].1 + win[4].2;
            let cx = (end - c[4] - c[3]) as f32 - c[2] as f32 / 2.0;
            let Some(cy) = through(cx as i32, y, 0, 1, c[2], Some((total, 2))) else {
                continue;
            };
            let Some(cx) = through(cx as i32, cy as i32, 1, 0, c[2], Some((total, 1))) else {
                continue;
            };
            // (the diagonal: no limit on a run, no comparison with the row)
            if through(cx as i32, cy as i32, 1, 1, i32::MAX - 1, None).is_none() {
                continue;
            }
            if !real(cx, cy)
                && !found
                    .iter()
                    .any(|(fx, fy)| (fx - cx).abs() < S as f32 && (fy - cy).abs() < S as f32)
            {
                found.push((cx, cy));
            }
        }
    }
    found.len()
}

/// A QR code for a terminal: two modules per character with half blocks, a four-module margin. With `colours`
/// it sets pure black on pure white itself (the terminal's theme then does not matter); without, it assumes light
/// text on a dark background. A terminal's cells are not exactly twice as tall as wide, so the modules are not
/// exactly square: a phone reads it, but less easily than the Mac app's drawing.
pub fn text(data: &str, colours: bool) -> String {
    let rows: Vec<Vec<bool>> = rows(data)
        .iter()
        .map(|r| r.bytes().map(|b| b == b'1').collect())
        .collect();
    let w = rows.len() as i32;
    if w == 0 {
        return String::new();
    }
    let dark = |x: i32, y: i32| x >= 0 && y >= 0 && x < w && y < w && rows[y as usize][x as usize];
    let mut out = String::new();
    let mut y = -QUIET;
    while y < w + QUIET {
        if colours {
            out.push_str("\x1b[38;5;16;48;5;231m");
        }
        for x in -QUIET..w + QUIET {
            out.push(match (colours, dark(x, y), dark(x, y + 1)) {
                (true, true, true) => '█',
                (true, true, false) => '▀',
                (true, false, true) => '▄',
                (true, false, false) => ' ',
                (false, true, true) => ' ',
                (false, true, false) => '▄',
                (false, false, true) => '▀',
                (false, false, false) => '█',
            });
        }
        out.push_str(if colours { "\x1b[0m\n" } else { "\n" });
        y += 2;
    }
    out
}
