//! The pane layout tree (#2). A binary tree whose leaves are pane ids and whose
//! internal nodes are splits — either a row (children side by side) or a column
//! (children stacked). Geometry is derived by recursively slicing a `Rect`.
//!
//! This replaces the equal-width-columns placeholder from #1. Directional focus
//! movement across the tree is #3; adjustable split ratios are a later issue
//! (splits are a fixed 50/50 for now).

/// A rectangle in terminal cells, borders included.
#[derive(Clone, Copy)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl Rect {
    /// The drawable content area inside the 1-cell border, if any remains.
    pub fn inner(&self) -> Option<Rect> {
        if self.w < 3 || self.h < 3 {
            return None;
        }
        Some(Rect {
            x: self.x + 1,
            y: self.y + 1,
            w: self.w - 2,
            h: self.h - 2,
        })
    }
}

/// A direction to move focus, for `neighbor`.
#[derive(Clone, Copy)]
pub enum FocusDir {
    Left,
    Right,
    Up,
    Down,
}

/// Find the best pane to move focus to from `from` in direction `dir`, given
/// each pane's rectangle. Candidates must lie on the correct side; among them we
/// prefer one that overlaps on the perpendicular axis, then the nearest along
/// the direction, then the nearest perpendicular offset. Returns `None` if there
/// is no pane that way.
pub fn neighbor(rects: &[(usize, Rect)], from: usize, dir: FocusDir) -> Option<usize> {
    let f = rects.iter().find(|(id, _)| *id == from)?.1;
    let (fx, fy) = center(&f);

    let mut best: Option<(usize, (bool, i32, i32))> = None;
    for (id, r) in rects {
        if *id == from {
            continue;
        }
        let (cx, cy) = center(r);
        let on_side = match dir {
            FocusDir::Left => cx < fx,
            FocusDir::Right => cx > fx,
            FocusDir::Up => cy < fy,
            FocusDir::Down => cy > fy,
        };
        if !on_side {
            continue;
        }
        let (axis, perp) = match dir {
            FocusDir::Left | FocusDir::Right => ((cx - fx).abs(), (cy - fy).abs()),
            FocusDir::Up | FocusDir::Down => ((cy - fy).abs(), (cx - fx).abs()),
        };
        let overlaps = match dir {
            FocusDir::Left | FocusDir::Right => ranges_overlap(f.y, f.h, r.y, r.h),
            FocusDir::Up | FocusDir::Down => ranges_overlap(f.x, f.w, r.x, r.w),
        };
        // Sort key: prefer perpendicular overlap, then nearest along axis.
        let score = (!overlaps, axis, perp);
        if best.as_ref().is_none_or(|(_, b)| score < *b) {
            best = Some((*id, score));
        }
    }
    best.map(|(id, _)| id)
}

/// Center point of a rect, in signed coords for distance math.
fn center(r: &Rect) -> (i32, i32) {
    (r.x as i32 + r.w as i32 / 2, r.y as i32 + r.h as i32 / 2)
}

/// Do the intervals `[a, a+alen)` and `[b, b+blen)` overlap?
fn ranges_overlap(a: u16, alen: u16, b: u16, blen: u16) -> bool {
    let (a0, a1) = (a as i32, a as i32 + alen as i32);
    let (b0, b1) = (b as i32, b as i32 + blen as i32);
    a0 < b1 && b0 < a1
}

/// Split orientation.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    /// Children sit side by side (the divider is vertical). tmux's `Ctrl-a |`.
    Row,
    /// Children stack top/bottom (the divider is horizontal). tmux's `Ctrl-a -`.
    Col,
}

/// A node in the layout tree.
pub enum Layout {
    /// A single pane, referenced by its stable id.
    Leaf(usize),
    /// A split of two subtrees. `ratio` is the fraction given to `a`.
    Split {
        dir: Dir,
        ratio: f32,
        a: Box<Layout>,
        b: Box<Layout>,
    },
}

impl Layout {
    /// Collect leaf ids left-to-right / top-to-bottom (in-order).
    pub fn leaves(&self, out: &mut Vec<usize>) {
        match self {
            Layout::Leaf(id) => out.push(*id),
            Layout::Split { a, b, .. } => {
                a.leaves(out);
                b.leaves(out);
            }
        }
    }

    /// The first leaf id, if any.
    pub fn first_leaf(&self) -> Option<usize> {
        let mut v = Vec::new();
        self.leaves(&mut v);
        v.first().copied()
    }

    /// Assign a rectangle to every leaf, slicing `area` down the tree.
    pub fn rects(&self, area: Rect, out: &mut Vec<(usize, Rect)>) {
        match self {
            Layout::Leaf(id) => out.push((*id, area)),
            Layout::Split { dir, ratio, a, b } => {
                let (ra, rb) = split_rect(area, *dir, *ratio);
                a.rects(ra, out);
                b.rects(rb, out);
            }
        }
    }

    /// Split the leaf holding `target` into `[target, new_id]` with `dir`.
    /// Returns true if `target` was found.
    pub fn split(&mut self, target: usize, new_id: usize, dir: Dir) -> bool {
        match self {
            Layout::Leaf(id) => {
                if *id == target {
                    let old = *id;
                    *self = Layout::Split {
                        dir,
                        ratio: 0.5,
                        a: Box::new(Layout::Leaf(old)),
                        b: Box::new(Layout::Leaf(new_id)),
                    };
                    true
                } else {
                    false
                }
            }
            Layout::Split { a, b, .. } => {
                a.split(target, new_id, dir) || b.split(target, new_id, dir)
            }
        }
    }

    /// Remove leaf `target`, collapsing a split into its surviving sibling.
    /// Returns `None` if the whole tree was just that leaf.
    pub fn remove(self, target: usize) -> Option<Layout> {
        match self {
            Layout::Leaf(id) => {
                if id == target {
                    None
                } else {
                    Some(Layout::Leaf(id))
                }
            }
            Layout::Split { dir, ratio, a, b } => match a.remove(target) {
                None => Some(*b),
                Some(a2) => match b.remove(target) {
                    None => Some(a2),
                    Some(b2) => Some(Layout::Split {
                        dir,
                        ratio,
                        a: Box::new(a2),
                        b: Box::new(b2),
                    }),
                },
            },
        }
    }
}

/// Slice `area` into two rects per `dir`, giving `ratio` of the split axis to
/// the first. The first side never exceeds the available extent, so the second
/// side's size (`total - first`) can never underflow. When there's room, both
/// sides keep at least 1 cell; a zero-sized axis yields two zero-sized sides.
fn split_rect(area: Rect, dir: Dir, ratio: f32) -> (Rect, Rect) {
    match dir {
        Dir::Row => {
            let wa = split_extent(area.w, ratio);
            (
                Rect {
                    x: area.x,
                    y: area.y,
                    w: wa,
                    h: area.h,
                },
                Rect {
                    x: area.x + wa,
                    y: area.y,
                    w: area.w - wa,
                    h: area.h,
                },
            )
        }
        Dir::Col => {
            let ha = split_extent(area.h, ratio);
            (
                Rect {
                    x: area.x,
                    y: area.y,
                    w: area.w,
                    h: ha,
                },
                Rect {
                    x: area.x,
                    y: area.y + ha,
                    w: area.w,
                    h: area.h - ha,
                },
            )
        }
    }
}

/// The first side's extent along a split axis of length `total`. Guaranteed
/// `<= total`, so `total - result` never underflows.
fn split_extent(total: u16, ratio: f32) -> u16 {
    let first = ((total as f32) * ratio).round() as u16;
    let first = first.min(total);
    if total >= 2 {
        first.clamp(1, total - 1)
    } else {
        first
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default IDE layout: nvim left (0), shell top-right (1), claude
    /// bottom-right (2), in a 100x40 area.
    fn default_rects() -> Vec<(usize, Rect)> {
        let layout = Layout::Split {
            dir: Dir::Row,
            ratio: 0.68,
            a: Box::new(Layout::Leaf(0)),
            b: Box::new(Layout::Split {
                dir: Dir::Col,
                ratio: 0.55,
                a: Box::new(Layout::Leaf(1)),
                b: Box::new(Layout::Leaf(2)),
            }),
        };
        let mut rects = Vec::new();
        layout.rects(
            Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 40,
            },
            &mut rects,
        );
        rects
    }

    #[test]
    fn directional_focus() {
        let r = default_rects();
        // From nvim (0), right lands on the top-right pane (shell, 1).
        assert_eq!(neighbor(&r, 0, FocusDir::Right), Some(1));
        // From shell (1): left -> nvim, down -> claude.
        assert_eq!(neighbor(&r, 1, FocusDir::Left), Some(0));
        assert_eq!(neighbor(&r, 1, FocusDir::Down), Some(2));
        // From claude (2): up -> shell, left -> nvim.
        assert_eq!(neighbor(&r, 2, FocusDir::Up), Some(1));
        assert_eq!(neighbor(&r, 2, FocusDir::Left), Some(0));
        // Nothing to the left of nvim.
        assert_eq!(neighbor(&r, 0, FocusDir::Left), None);
    }
}
