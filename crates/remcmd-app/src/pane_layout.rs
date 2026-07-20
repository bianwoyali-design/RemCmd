#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct PaneId(pub(crate) u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PaneLayout {
    Pane(PaneId),
    Split {
        axis: SplitAxis,
        first: Box<PaneLayout>,
        second: Box<PaneLayout>,
    },
}

impl PaneLayout {
    pub(crate) fn split(&mut self, target: PaneId, new_pane: PaneId, axis: SplitAxis) -> bool {
        match self {
            Self::Pane(pane_id) if *pane_id == target => {
                *self = Self::Split {
                    axis,
                    first: Box::new(Self::Pane(target)),
                    second: Box::new(Self::Pane(new_pane)),
                };
                true
            }
            Self::Pane(_) => false,
            Self::Split { first, second, .. } => {
                first.split(target, new_pane, axis) || second.split(target, new_pane, axis)
            }
        }
    }

    pub(crate) fn without(self, target: PaneId) -> (Option<Self>, bool) {
        match self {
            Self::Pane(pane_id) if pane_id == target => (None, true),
            Self::Pane(_) => (Some(self), false),
            Self::Split {
                axis,
                first,
                second,
            } => {
                let (first, removed) = first.without(target);
                if removed {
                    return (
                        Some(match first {
                            Some(first) => Self::Split {
                                axis,
                                first: Box::new(first),
                                second,
                            },
                            None => *second,
                        }),
                        true,
                    );
                }

                let first = first.expect("unmodified split branch should remain present");
                let (second, removed) = second.without(target);
                if removed {
                    return (
                        Some(match second {
                            Some(second) => Self::Split {
                                axis,
                                first: Box::new(first),
                                second: Box::new(second),
                            },
                            None => first,
                        }),
                        true,
                    );
                }

                (
                    Some(Self::Split {
                        axis,
                        first: Box::new(first),
                        second: Box::new(
                            second.expect("unmodified split branch should remain present"),
                        ),
                    }),
                    false,
                )
            }
        }
    }

    pub(crate) fn first_pane(&self) -> PaneId {
        match self {
            Self::Pane(pane_id) => *pane_id,
            Self::Split { first, .. } => first.first_pane(),
        }
    }

    pub(crate) fn contains(&self, target: PaneId) -> bool {
        match self {
            Self::Pane(pane_id) => *pane_id == target,
            Self::Split { first, second, .. } => first.contains(target) || second.contains(target),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_replaces_only_the_target_leaf() {
        let first = PaneId(1);
        let second = PaneId(2);
        let third = PaneId(3);
        let mut layout = PaneLayout::Pane(first);

        assert!(layout.split(first, second, SplitAxis::Horizontal));
        assert!(layout.split(second, third, SplitAxis::Vertical));
        assert!(layout.contains(first));
        assert!(layout.contains(second));
        assert!(layout.contains(third));
    }

    #[test]
    fn removing_a_nested_pane_collapses_its_parent_split() {
        let first = PaneId(1);
        let second = PaneId(2);
        let third = PaneId(3);
        let mut layout = PaneLayout::Pane(first);
        layout.split(first, second, SplitAxis::Horizontal);
        layout.split(second, third, SplitAxis::Vertical);

        let (layout, removed) = layout.without(second);
        let layout = layout.unwrap();

        assert!(removed);
        assert!(layout.contains(first));
        assert!(!layout.contains(second));
        assert!(layout.contains(third));
    }

    #[test]
    fn removing_the_only_pane_empties_the_layout() {
        let pane = PaneId(1);

        assert_eq!(PaneLayout::Pane(pane).without(pane), (None, true));
    }
}
