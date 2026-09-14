use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Layout {
    Leaf(usize),
    Split {
        axis: Axis,
        ratio: f32,
        first: Box<Layout>,
        second: Box<Layout>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedTab {
    pub sessions: Vec<crate::session::SessionKind>,
    pub layout: Layout,
    pub focused: usize,
}

impl Layout {
    pub fn split(&mut self, target: usize, new: usize, axis: Axis) {
        match self {
            Self::Leaf(index) if *index == target => {
                *self = Self::Split {
                    axis,
                    ratio: 0.5,
                    first: Box::new(Self::Leaf(target)),
                    second: Box::new(Self::Leaf(new)),
                };
            }
            Self::Split { first, second, .. } => {
                first.split(target, new, axis);
                second.split(target, new, axis);
            }
            _ => {}
        }
    }

    pub fn remove(self, target: usize) -> Option<Self> {
        match self {
            Self::Leaf(index) => {
                if index == target {
                    None
                } else {
                    Some(Self::Leaf(if index > target { index - 1 } else { index }))
                }
            }
            Self::Split {
                axis,
                ratio,
                first,
                second,
            } => match (first.remove(target), second.remove(target)) {
                (Some(a), Some(b)) => Some(Self::Split {
                    axis,
                    ratio,
                    first: Box::new(a),
                    second: Box::new(b),
                }),
                (a, b) => a.or(b),
            },
        }
    }

    pub fn valid(&self, count: usize) -> bool {
        fn visit(node: &Layout, ids: &mut Vec<usize>, depth: usize) -> bool {
            if depth > 32 {
                return false;
            }
            match node {
                Layout::Leaf(i) => {
                    ids.push(*i);
                    true
                }
                Layout::Split {
                    ratio,
                    first,
                    second,
                    ..
                } => {
                    ratio.is_finite()
                        && *ratio > 0.0
                        && *ratio < 1.0
                        && visit(first, ids, depth + 1)
                        && visit(second, ids, depth + 1)
                }
            }
        }
        let mut ids = vec![];
        if !visit(self, &mut ids, 0) {
            return false;
        }
        ids.sort_unstable();
        ids == (0..count).collect::<Vec<_>>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_splits_collapse_and_remap_after_close() {
        let mut l = Layout::Leaf(0);
        l.split(0, 1, Axis::Horizontal);
        l.split(1, 2, Axis::Vertical);
        assert!(l.valid(3));
        let l = l.remove(1).unwrap();
        assert!(l.valid(2));
        assert!(l.remove(0).unwrap().valid(1));
    }
}
