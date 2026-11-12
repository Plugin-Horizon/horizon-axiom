// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): restrictions/AxiomPermissionSet.java

use std::collections::BTreeSet;

use super::permission::{AxiomPermission, ALL_PERMISSIONS};

/// Explicit allow/deny lists plus the effective permission set derived from them.
///
/// Rules (same as the Java class): `ALL` grants everything; an explicitly allowed node is granted;
/// a node that is not explicitly denied is granted when its parent is granted; everything else is not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AxiomPermissionSet {
    explicitly_allowed: BTreeSet<AxiomPermission>,
    explicitly_denied: BTreeSet<AxiomPermission>,
    calculated: BTreeSet<AxiomPermission>,
}

impl AxiomPermissionSet {
    pub fn new(
        explicitly_allowed: BTreeSet<AxiomPermission>,
        explicitly_denied: BTreeSet<AxiomPermission>,
    ) -> Self {
        let calculated = calculate(&explicitly_allowed, &explicitly_denied);
        Self {
            explicitly_allowed,
            explicitly_denied,
            calculated,
        }
    }

    /// Grants everything (Java `AxiomPermissionSet.ALL`).
    pub fn all() -> Self {
        Self::new(BTreeSet::from([AxiomPermission::All]), BTreeSet::new())
    }

    /// Grants the default node only (Java `AxiomPermissionSet.DEFAULT`).
    pub fn default_set() -> Self {
        Self::new(BTreeSet::from([AxiomPermission::Default]), BTreeSet::new())
    }

    /// Denies everything (Java `AxiomPermissionSet.NONE`).
    pub fn none() -> Self {
        Self::new(BTreeSet::new(), ALL_PERMISSIONS.iter().copied().collect())
    }

    pub fn explicitly_allowed(&self) -> &BTreeSet<AxiomPermission> {
        &self.explicitly_allowed
    }

    pub fn explicitly_denied(&self) -> &BTreeSet<AxiomPermission> {
        &self.explicitly_denied
    }

    /// Effective permissions after inheritance.
    pub fn granted(&self) -> &BTreeSet<AxiomPermission> {
        &self.calculated
    }

    pub fn contains(&self, permission: AxiomPermission) -> bool {
        self.calculated.contains(&permission)
    }
}

fn calculate(
    allowed: &BTreeSet<AxiomPermission>,
    denied: &BTreeSet<AxiomPermission>,
) -> BTreeSet<AxiomPermission> {
    if allowed.contains(&AxiomPermission::All) {
        return ALL_PERMISSIONS.iter().copied().collect();
    }

    let mut calculated = BTreeSet::new();
    // ALL_PERMISSIONS is in ordinal order, so a parent is always decided before its children.
    for &value in ALL_PERMISSIONS.iter() {
        if allowed.contains(&value) {
            calculated.insert(value);
        } else if !denied.contains(&value) {
            if let Some(parent) = value.parent() {
                if calculated.contains(&parent) {
                    calculated.insert(value);
                }
            }
        }
    }
    calculated
}

#[cfg(test)]
mod tests {
    use super::*;
    use AxiomPermission::*;

    fn set(items: &[AxiomPermission]) -> BTreeSet<AxiomPermission> {
        items.iter().copied().collect()
    }

    #[test]
    fn all_grants_everything() {
        let s = AxiomPermissionSet::all();
        for p in ALL_PERMISSIONS {
            assert!(s.contains(p), "missing {:?}", p);
        }
    }

    #[test]
    fn none_grants_nothing() {
        let s = AxiomPermissionSet::none();
        assert!(s.granted().is_empty());
        assert!(!s.contains(Default));
    }

    #[test]
    fn default_wildcard_grants_its_children_only() {
        let s = AxiomPermissionSet::default_set();
        assert!(s.contains(Default));
        assert!(s.contains(Use));
        assert!(s.contains(BuildPlace));
        assert!(s.contains(ToolMelt));
        assert!(s.contains(BuildertoolMove));
        // Non-default permissions are not implied by the default node.
        assert!(!s.contains(EntitySpawn));
        assert!(!s.contains(Blueprint));
        assert!(!s.contains(BuildDangerous));
    }

    #[test]
    fn explicit_child_without_parent_is_granted() {
        let s = AxiomPermissionSet::new(set(&[EntitySpawn]), BTreeSet::new());
        assert!(s.contains(EntitySpawn));
        assert!(!s.contains(EntityManipulate));
        assert!(!s.contains(Entity));
    }

    #[test]
    fn denied_child_is_removed_from_inherited_grant() {
        let s = AxiomPermissionSet::new(set(&[Default]), set(&[ToolMelt]));
        assert!(s.contains(Default));
        assert!(s.contains(ToolFloodfill));
        assert!(!s.contains(ToolMelt));
    }

    #[test]
    fn denied_parent_blocks_children_that_are_not_explicit() {
        let s = AxiomPermissionSet::new(set(&[Default]), set(&[Tool]));
        assert!(!s.contains(Tool));
        assert!(!s.contains(ToolMelt));
        // Not a child of TOOL, so unaffected.
        assert!(s.contains(BuildPlace));
    }

    #[test]
    fn explicit_allow_wins_over_explicit_deny_on_same_node() {
        let s = AxiomPermissionSet::new(set(&[Default, Use]), set(&[Use]));
        assert!(s.contains(Use));
    }

    #[test]
    fn explicit_allow_of_child_survives_denied_parent() {
        let s = AxiomPermissionSet::new(set(&[ToolMelt]), set(&[Tool]));
        assert!(s.contains(ToolMelt));
        assert!(!s.contains(ToolRuler));
    }

    #[test]
    fn explicit_lists_are_kept_as_given() {
        let s = AxiomPermissionSet::new(set(&[Default]), set(&[Tool]));
        assert_eq!(s.explicitly_allowed(), &set(&[Default]));
        assert_eq!(s.explicitly_denied(), &set(&[Tool]));
    }

    #[test]
    fn internal_names_match_java() {
        assert_eq!(Default.internal_name(), "axiom.*");
        assert_eq!(All.internal_name(), "axiom.all");
        assert_eq!(Use.internal_name(), "axiom.use");
        assert_eq!(BuildPlace.node(), "axiom.build.place");
        assert_eq!(BuildPlace.parent(), Some(Build));
        assert_eq!(Default.parent(), None);
        assert_eq!(ALL_PERMISSIONS.len(), 95);
    }
}
