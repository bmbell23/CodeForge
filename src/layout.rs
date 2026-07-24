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
