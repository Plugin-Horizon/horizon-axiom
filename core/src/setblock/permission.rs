// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): integration/SectionPermissionChecker.java,
// integration/Box.java, integration/BoxWithBoolean.java

//! Per-section permission bounds. Coordinates inside a section are local, `0..=15`.

/// Inclusive box in section-local coordinates (Java `integration/Box`). Renamed to avoid clashing with `std::boxed::Box`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalBox {
    pub min_x: i32,
    pub min_y: i32,
    pub min_z: i32,
    pub max_x: i32,
    pub max_y: i32,
    pub max_z: i32,
}

/// Java `BoxWithBoolean`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoxWithBoolean {
    pub bx: LocalBox,
    pub value: bool,
}

impl LocalBox {
    pub const FULL: LocalBox = LocalBox::new(0, 0, 0, 15, 15, 15);
    pub const EMPTY: LocalBox = LocalBox::new(0, 0, 0, 0, 0, 0);

    pub const fn new(min_x: i32, min_y: i32, min_z: i32, max_x: i32, max_y: i32, max_z: i32) -> Self {
        LocalBox { min_x, min_y, min_z, max_x, max_y, max_z }
    }

    pub fn completely_overlaps(&self, other: &LocalBox) -> bool {
        self.min_x <= other.min_x
            && self.min_y <= other.min_y
            && self.min_z <= other.min_z
            && self.max_x >= other.max_x
            && self.max_y >= other.max_y
            && self.max_z >= other.max_z
    }

    pub fn contains(&self, x: i32, y: i32, z: i32) -> bool {
        self.min_x <= x && self.min_y <= y && self.min_z <= z && self.max_x >= x && self.max_y >= y && self.max_z >= z
    }

    /// Java `Box.intersection`. `None` when the boxes do not touch.
    pub fn intersection(a: &LocalBox, b: &LocalBox) -> Option<LocalBox> {
        if a.min_x > b.max_x
            || b.min_x > a.max_x
            || a.min_y > b.max_y
            || b.min_y > a.max_y
            || a.min_z > b.max_z
            || b.min_z > a.max_z
        {
            return None;
        }
        Some(LocalBox::new(
            a.min_x.max(b.min_x),
            a.min_y.max(b.min_y),
            a.min_z.max(b.min_z),
            a.max_x.min(b.max_x),
            a.max_y.min(b.max_y),
            a.max_z.min(b.max_z),
        ))
    }

    /// Java `Box.tryCombine`: merge two boxes that share two axes and touch on the third.
    pub fn try_combine(&self, other: &LocalBox) -> Option<LocalBox> {
        if self.completely_overlaps(other) {
            return Some(*self);
        }
        if other.completely_overlaps(self) {
            return Some(*other);
        }

        if other.min_x == self.min_x && other.max_x == self.max_x {
            if other.min_y == self.min_y && other.max_y == self.max_y {
                if continuous(other.min_z, other.max_z, self.min_z, self.max_z) {
                    return Some(LocalBox::new(
                        other.min_x,
                        other.min_y,
                        other.min_z.min(self.min_z),
                        other.max_x,
                        other.max_y,
                        other.max_z.max(self.max_z),
                    ));
                }
            } else if other.min_z == self.min_z && other.max_z == self.max_z && continuous(other.min_y, other.max_y, self.min_y, self.max_y) {
                return Some(LocalBox::new(
                    other.min_x,
                    other.min_y.min(self.min_y),
                    other.min_z,
                    other.max_x,
                    other.max_y.max(self.max_y),
                    other.max_z,
                ));
            }
        } else if other.min_y == self.min_y
            && other.max_y == self.max_y
            && other.min_z == self.min_z
            && other.max_z == self.max_z
            && continuous(other.min_x, other.max_x, self.min_x, self.max_x)
        {
            return Some(LocalBox::new(
                other.min_x.min(self.min_x),
                other.min_y,
                other.min_z,
                other.max_x.max(self.max_x),
                other.max_y,
                other.max_z,
            ));
        }
        None
    }

    /// Java `Box.combineAll`: repeatedly merge pairs until no pair combines.
    pub fn combine_all(boxes: &mut Vec<LocalBox>) {
        'main: while boxes.len() >= 2 {
            for i in 0..boxes.len() - 1 {
                for j in i + 1..boxes.len() {
                    if let Some(combined) = boxes[i].try_combine(&boxes[j]) {
                        boxes.remove(j);
                        boxes.remove(i);
                        boxes.push(combined);
                        continue 'main;
                    }
                }
            }
            break;
        }
    }
}

/// Java `Box.areLineSegmentsContinuous`.
fn continuous(min1: i32, max1: i32, min2: i32, max2: i32) -> bool {
    let size1 = max1 - min1 + 1;
    let size2 = max2 - min2 + 1;
    let mid_diff = ((min1 + max1) - (min2 + max2)).abs();
    mid_diff <= size1 + size2
}

/// Permission bounds for one chunk section (Java `SectionPermissionChecker`).
///
/// Java returns shared singletons (`ALL_ALLOWED`, `NONE_ALLOWED`) and anonymous classes. Each one becomes a
/// variant here. `combine` is the Java static method of the same name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SectionPermissionChecker {
    /// Java `ALL_ALLOWED`.
    All,
    /// Java `NONE_ALLOWED`.
    None,
    /// Java `AllAllowedInBox`: everything inside the box is allowed.
    AllInBox(LocalBox),
    /// Java `AllAllowedBoxes`: a position is allowed when it is inside any box.
    AllowedBoxes { bounds: LocalBox, allowed: Vec<LocalBox> },
    /// Java `BooleanBoxes`: the first box containing a position decides, otherwise `default_value`.
    BooleanBoxes { bounds: LocalBox, boxes: Vec<BoxWithBoolean>, default_value: bool },
    /// Anonymous checker from `combine`: both checkers must allow the position.
    Intersection { first: Box<SectionPermissionChecker>, second: Box<SectionPermissionChecker>, bounds: LocalBox },
}

impl SectionPermissionChecker {
    pub fn all_allowed(&self) -> bool {
        matches!(self, Self::All | Self::AllInBox(_))
    }

    pub fn none_allowed(&self) -> bool {
        matches!(self, Self::None)
    }

    pub fn allowed(&self, x: i32, y: i32, z: i32) -> bool {
        match self {
            Self::All | Self::AllInBox(_) => true,
            Self::None => false,
            Self::AllowedBoxes { allowed, .. } => allowed.iter().any(|b| b.contains(x, y, z)),
            Self::BooleanBoxes { boxes, default_value, .. } => {
                for bwb in boxes {
                    if bwb.bx.contains(x, y, z) {
                        return bwb.value;
                    }
                }
                *default_value
            }
            Self::Intersection { first, second, .. } => first.allowed(x, y, z) && second.allowed(x, y, z),
        }
    }

    pub fn bounds(&self) -> LocalBox {
        match self {
            Self::All => LocalBox::FULL,
            Self::None => LocalBox::EMPTY,
            Self::AllInBox(b) => *b,
            Self::AllowedBoxes { bounds, .. } | Self::BooleanBoxes { bounds, .. } | Self::Intersection { bounds, .. } => *bounds,
        }
    }

    /// Java `SectionPermissionChecker.combine(first, second)`.
    pub fn combine(first: SectionPermissionChecker, second: SectionPermissionChecker) -> SectionPermissionChecker {
        if first.none_allowed() || second.none_allowed() {
            return Self::None;
        }
        if matches!(first, Self::All) {
            return second;
        }
        if matches!(second, Self::All) {
            return first;
        }

        let Some(intersect) = LocalBox::intersection(&first.bounds(), &second.bounds()) else {
            return Self::None;
        };

        if first.all_allowed() && second.all_allowed() {
            return Self::AllInBox(intersect);
        }

        Self::Intersection { first: Box::new(first), second: Box::new(second), bounds: intersect }
    }

    /// Java `SectionPermissionChecker.fromAllowedBoxes`.
    pub fn from_allowed_boxes(allowed: Vec<LocalBox>) -> SectionPermissionChecker {
        if allowed.is_empty() {
            return Self::None;
        }
        if allowed.len() == 1 {
            let b = allowed[0];
            return if b.completely_overlaps(&LocalBox::FULL) { Self::All } else { Self::AllInBox(b) };
        }

        // Same starting values as Java (15 and 0), so the bounds match even for odd inputs.
        let bounds = enclosing(&allowed);
        Self::AllowedBoxes { bounds, allowed }
    }

    /// Java `SectionPermissionChecker.fromBoxWithBooleans`.
    pub fn from_box_with_booleans(boxes: Vec<BoxWithBoolean>, default_value: bool) -> SectionPermissionChecker {
        if boxes.is_empty() {
            return if default_value { Self::All } else { Self::None };
        }

        if boxes.len() == 1 {
            let bwb = boxes[0];
            if bwb.value {
                // Java: all allowed when the default is true or the box covers the whole section.
                if default_value || bwb.bx.completely_overlaps(&LocalBox::FULL) {
                    return Self::All;
                }
                return Self::AllInBox(bwb.bx);
            }
            // Java: a single denied box is "none allowed" unless the default allows everything else.
            if !default_value || bwb.bx.completely_overlaps(&LocalBox::FULL) {
                return Self::None;
            }
        }

        if default_value {
            return Self::BooleanBoxes { bounds: LocalBox::FULL, boxes, default_value: true };
        }

        let allowed: Vec<LocalBox> = boxes.iter().filter(|b| b.value).map(|b| b.bx).collect();
        let bounds = enclosing(&allowed);
        Self::BooleanBoxes { bounds, boxes, default_value: false }
    }
}

/// Bounding box of `boxes` with the Java starting values 15 (min) and 0 (max).
fn enclosing(boxes: &[LocalBox]) -> LocalBox {
    let (mut min_x, mut min_y, mut min_z) = (15, 15, 15);
    let (mut max_x, mut max_y, mut max_z) = (0, 0, 0);
    for b in boxes {
        min_x = min_x.min(b.min_x);
        min_y = min_y.min(b.min_y);
        min_z = min_z.min(b.min_z);
        max_x = max_x.max(b.max_x);
        max_y = max_y.max(b.max_y);
        max_z = max_z.max(b.max_z);
    }
    LocalBox::new(min_x, min_y, min_z, max_x, max_y, max_z)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_box_overlaps_and_single_full_is_all() {
        assert_eq!(SectionPermissionChecker::from_allowed_boxes(vec![LocalBox::FULL]), SectionPermissionChecker::All);
        assert_eq!(
            SectionPermissionChecker::from_allowed_boxes(vec![LocalBox::new(0, 0, 0, 3, 3, 3)]),
            SectionPermissionChecker::AllInBox(LocalBox::new(0, 0, 0, 3, 3, 3))
        );
        assert_eq!(SectionPermissionChecker::from_allowed_boxes(vec![]), SectionPermissionChecker::None);
    }

    #[test]
    fn combine_with_none_and_all() {
        let inner = SectionPermissionChecker::AllInBox(LocalBox::new(2, 2, 2, 5, 5, 5));
        assert_eq!(SectionPermissionChecker::combine(SectionPermissionChecker::All, inner.clone()), inner);
        assert_eq!(SectionPermissionChecker::combine(inner.clone(), SectionPermissionChecker::All), inner);
        assert_eq!(SectionPermissionChecker::combine(inner.clone(), SectionPermissionChecker::None), SectionPermissionChecker::None);
    }

    #[test]
    fn combine_disjoint_is_none_and_overlapping_intersects() {
        let a = SectionPermissionChecker::AllInBox(LocalBox::new(0, 0, 0, 3, 3, 3));
        let b = SectionPermissionChecker::AllInBox(LocalBox::new(8, 0, 0, 9, 3, 3));
        assert_eq!(SectionPermissionChecker::combine(a.clone(), b), SectionPermissionChecker::None);

        let c = SectionPermissionChecker::AllInBox(LocalBox::new(2, 0, 0, 9, 3, 3));
        let both = SectionPermissionChecker::combine(a, c);
        assert_eq!(both, SectionPermissionChecker::AllInBox(LocalBox::new(2, 0, 0, 3, 3, 3)));
    }

    #[test]
    fn combine_with_partial_checker_requires_both() {
        let boxes = SectionPermissionChecker::from_box_with_booleans(
            vec![BoxWithBoolean { bx: LocalBox::new(0, 0, 0, 7, 15, 15), value: true }],
            false,
        );
        let region = SectionPermissionChecker::AllInBox(LocalBox::new(4, 0, 0, 15, 15, 15));
        let both = SectionPermissionChecker::combine(boxes, region);
        // Java AllAllowedInBox.allowed is true everywhere; callers only iterate inside bounds().
        assert_eq!(both.bounds(), LocalBox::new(4, 0, 0, 7, 15, 15));
        assert!(both.allowed(5, 0, 0));

        // With a partial (BooleanBoxes) checker, the intersection really filters.
        let partial = SectionPermissionChecker::from_box_with_booleans(
            vec![
                BoxWithBoolean { bx: LocalBox::new(0, 0, 0, 3, 15, 15), value: false },
                BoxWithBoolean { bx: LocalBox::new(0, 0, 0, 15, 15, 15), value: true },
            ],
            true,
        );
        let both = SectionPermissionChecker::combine(partial, SectionPermissionChecker::AllInBox(LocalBox::new(0, 0, 0, 9, 15, 15)));
        assert!(!both.allowed(1, 0, 0));
        assert!(both.allowed(5, 0, 0));
        assert_eq!(both.bounds(), LocalBox::new(0, 0, 0, 9, 15, 15));
    }

    #[test]
    fn boolean_boxes_first_match_wins_and_default() {
        let c = SectionPermissionChecker::from_box_with_booleans(
            vec![
                BoxWithBoolean { bx: LocalBox::new(0, 0, 0, 3, 3, 3), value: false },
                BoxWithBoolean { bx: LocalBox::new(0, 0, 0, 15, 15, 15), value: true },
            ],
            true,
        );
        assert!(!c.allowed(1, 1, 1));
        assert!(c.allowed(10, 10, 10));
        assert_eq!(c.bounds(), LocalBox::FULL);
    }

    #[test]
    fn single_false_box_with_default_false_is_none() {
        let c = SectionPermissionChecker::from_box_with_booleans(
            vec![BoxWithBoolean { bx: LocalBox::new(0, 0, 0, 3, 3, 3), value: false }],
            false,
        );
        assert_eq!(c, SectionPermissionChecker::None);
    }

    #[test]
    fn try_combine_merges_touching_boxes_on_one_axis() {
        let a = LocalBox::new(0, 0, 0, 3, 3, 3);
        let b = LocalBox::new(0, 0, 4, 3, 3, 7);
        assert_eq!(a.try_combine(&b), Some(LocalBox::new(0, 0, 0, 3, 3, 7)));
        let far = LocalBox::new(0, 0, 12, 3, 3, 15);
        assert_eq!(a.try_combine(&far), None);
    }

    #[test]
    fn combine_all_reduces_a_row_to_one_box() {
        let mut boxes = vec![
            LocalBox::new(0, 0, 0, 3, 3, 3),
            LocalBox::new(4, 0, 0, 7, 3, 3),
            LocalBox::new(8, 0, 0, 11, 3, 3),
        ];
        LocalBox::combine_all(&mut boxes);
        assert_eq!(boxes, vec![LocalBox::new(0, 0, 0, 11, 3, 3)]);
    }
}
