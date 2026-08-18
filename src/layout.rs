use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PaneId(pub u64);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SplitId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    pub fn contains(self, x: u16, y: u16) -> bool {
        x >= self.x
            && y >= self.y
            && x < self.x.saturating_add(self.width)
            && y < self.y.saturating_add(self.height)
    }

    fn center(self) -> (i32, i32) {
        (
            i32::from(self.x) * 2 + i32::from(self.width),
            i32::from(self.y) * 2 + i32::from(self.height),
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum LayoutNode {
    Pane(PaneId),
    Split {
        id: SplitId,
        direction: SplitDirection,
        ratio: f32,
        first: Box<Self>,
        second: Box<Self>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SplitterRect {
    pub id: SplitId,
    pub direction: SplitDirection,
    pub rect: Rect,
    pub container: Rect,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CalculatedLayout {
    pub panes: BTreeMap<PaneId, Rect>,
    pub splitters: Vec<SplitterRect>,
}

impl LayoutNode {
    pub fn calculate(
        &self,
        area: Rect,
        minimum_width: u16,
        minimum_height: u16,
    ) -> CalculatedLayout {
        let mut result = CalculatedLayout::default();
        self.calculate_into(area, minimum_width, minimum_height, &mut result);
        result
    }

    fn calculate_into(
        &self,
        area: Rect,
        minimum_width: u16,
        minimum_height: u16,
        result: &mut CalculatedLayout,
    ) {
        match self {
            Self::Pane(id) => {
                result.panes.insert(*id, area);
            }
            Self::Split {
                id,
                direction,
                ratio,
                first,
                second,
            } => {
                let (first_area, splitter, second_area) =
                    split_rect(area, *direction, *ratio, minimum_width, minimum_height);
                first.calculate_into(first_area, minimum_width, minimum_height, result);
                result.splitters.push(SplitterRect {
                    id: *id,
                    direction: *direction,
                    rect: splitter,
                    container: area,
                });
                second.calculate_into(second_area, minimum_width, minimum_height, result);
            }
        }
    }

    pub fn split(
        &mut self,
        target: PaneId,
        new_pane: PaneId,
        split_id: SplitId,
        direction: SplitDirection,
        ratio: f32,
    ) -> bool {
        match self {
            Self::Pane(id) if *id == target => {
                *self = Self::Split {
                    id: split_id,
                    direction,
                    ratio: ratio.clamp(0.1, 0.9),
                    first: Box::new(Self::Pane(target)),
                    second: Box::new(Self::Pane(new_pane)),
                };
                true
            }
            Self::Pane(_) => false,
            Self::Split { first, second, .. } => {
                first.split(target, new_pane, split_id, direction, ratio)
                    || second.split(target, new_pane, split_id, direction, ratio)
            }
        }
    }

    pub fn close(self, target: PaneId) -> Option<Self> {
        match self {
            Self::Pane(id) => (id != target).then_some(Self::Pane(id)),
            Self::Split {
                id,
                direction,
                ratio,
                first,
                second,
            } => match ((*first).close(target), (*second).close(target)) {
                (Some(first), Some(second)) => Some(Self::Split {
                    id,
                    direction,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(remaining), None) | (None, Some(remaining)) => Some(remaining),
                (None, None) => None,
            },
        }
    }

    pub fn set_ratio(&mut self, target: SplitId, ratio: f32) -> bool {
        match self {
            Self::Pane(_) => false,
            Self::Split {
                id,
                ratio: current,
                first,
                second,
                ..
            } => {
                if *id == target {
                    *current = ratio.clamp(0.1, 0.9);
                    true
                } else {
                    first.set_ratio(target, ratio) || second.set_ratio(target, ratio)
                }
            }
        }
    }

    pub fn pane_count(&self) -> usize {
        match self {
            Self::Pane(_) => 1,
            Self::Split { first, second, .. } => first.pane_count() + second.pane_count(),
        }
    }
}

fn split_rect(
    area: Rect,
    direction: SplitDirection,
    ratio: f32,
    minimum_width: u16,
    minimum_height: u16,
) -> (Rect, Rect, Rect) {
    let (origin, total, minimum) = match direction {
        SplitDirection::Vertical => (area.x, area.width, minimum_width),
        SplitDirection::Horizontal => (area.y, area.height, minimum_height),
    };
    let available = total.saturating_sub(1);
    let desired = (f32::from(available) * ratio.clamp(0.0, 1.0)).round() as u16;
    let lower = minimum.min(available);
    let upper = available.saturating_sub(minimum).max(lower);
    let first_size = desired.clamp(lower.min(upper), upper);
    let second_size = available.saturating_sub(first_size);
    let divider = origin.saturating_add(first_size);
    match direction {
        SplitDirection::Vertical => (
            Rect {
                width: first_size,
                ..area
            },
            Rect {
                x: divider,
                width: u16::from(total > 0),
                ..area
            },
            Rect {
                x: divider.saturating_add(u16::from(total > 0)),
                width: second_size,
                ..area
            },
        ),
        SplitDirection::Horizontal => (
            Rect {
                height: first_size,
                ..area
            },
            Rect {
                y: divider,
                height: u16::from(total > 0),
                ..area
            },
            Rect {
                y: divider.saturating_add(u16::from(total > 0)),
                height: second_size,
                ..area
            },
        ),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FocusDirection {
    Left,
    Right,
    Up,
    Down,
}

pub fn nearest_pane(
    current: PaneId,
    direction: FocusDirection,
    panes: &BTreeMap<PaneId, Rect>,
) -> Option<PaneId> {
    let origin = panes.get(&current)?.center();
    panes
        .iter()
        .filter(|(id, _)| **id != current)
        .filter_map(|(id, rect)| {
            let center = rect.center();
            let eligible = match direction {
                FocusDirection::Left => center.0 < origin.0,
                FocusDirection::Right => center.0 > origin.0,
                FocusDirection::Up => center.1 < origin.1,
                FocusDirection::Down => center.1 > origin.1,
            };
            eligible.then(|| {
                let primary = match direction {
                    FocusDirection::Left | FocusDirection::Right => (center.0 - origin.0).abs(),
                    FocusDirection::Up | FocusDirection::Down => (center.1 - origin.1).abs(),
                };
                let secondary = match direction {
                    FocusDirection::Left | FocusDirection::Right => (center.1 - origin.1).abs(),
                    FocusDirection::Up | FocusDirection::Down => (center.0 - origin.0).abs(),
                };
                ((primary, secondary, id.0), *id)
            })
        })
        .min_by_key(|(score, _)| *score)
        .map(|(_, id)| id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_layout_fills_area_and_records_splitters() {
        let tree = LayoutNode::Split {
            id: SplitId(1),
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Pane(PaneId(1))),
            second: Box::new(LayoutNode::Split {
                id: SplitId(2),
                direction: SplitDirection::Horizontal,
                ratio: 0.5,
                first: Box::new(LayoutNode::Pane(PaneId(2))),
                second: Box::new(LayoutNode::Pane(PaneId(3))),
            }),
        };
        let calculated = tree.calculate(
            Rect {
                x: 0,
                y: 0,
                width: 81,
                height: 25,
            },
            10,
            3,
        );
        assert_eq!(calculated.panes.len(), 3);
        assert_eq!(calculated.splitters.len(), 2);
        assert_eq!(calculated.panes[&PaneId(1)].width, 40);
    }

    #[test]
    fn close_normalizes_parent_split() {
        let tree = LayoutNode::Split {
            id: SplitId(1),
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Pane(PaneId(1))),
            second: Box::new(LayoutNode::Pane(PaneId(2))),
        };
        assert_eq!(tree.close(PaneId(2)), Some(LayoutNode::Pane(PaneId(1))));
    }

    #[test]
    fn focus_uses_primary_then_secondary_distance() {
        let panes = BTreeMap::from([
            (
                PaneId(1),
                Rect {
                    x: 0,
                    y: 0,
                    width: 10,
                    height: 10,
                },
            ),
            (
                PaneId(2),
                Rect {
                    x: 11,
                    y: 0,
                    width: 10,
                    height: 4,
                },
            ),
            (
                PaneId(3),
                Rect {
                    x: 11,
                    y: 5,
                    width: 10,
                    height: 5,
                },
            ),
        ]);
        assert_eq!(
            nearest_pane(PaneId(1), FocusDirection::Right, &panes),
            Some(PaneId(3))
        );
    }
}
