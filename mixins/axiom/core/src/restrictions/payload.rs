// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): restrictions/Restrictions.java
// (payload layout and coordinate clamping). Channel: `axiom:restrictions`.

use std::collections::BTreeSet;

use crate::buf::Writer;

use super::permission::AxiomPermission;

/// Clientbound channel carrying a player's restrictions.
pub const CHANNEL_RESTRICTIONS: &str = "axiom:restrictions";

/// Clamp limits for the x/z axes (Java `Restrictions.send`).
const HORIZONTAL_LIMIT: i32 = 33554431;
/// Clamp limits for the y axis.
const VERTICAL_LIMIT: i32 = 2047;

/// An axis-aligned box a player may edit (a PlotSquared plot, for example). Corners are inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlotBox {
    pub min: [i32; 3],
    pub max: [i32; 3],
}

impl PlotBox {
    /// The single-block box at the origin that Java uses to deny editing outside a plot.
    pub const ORIGIN: PlotBox = PlotBox {
        min: [0, 0, 0],
        max: [0, 0, 0],
    };

    /// True when the box covers the whole integer range (Java treats this as "no bounds").
    pub fn is_unbounded(&self) -> bool {
        self.min == [i32::MIN; 3] && self.max == [i32::MAX; 3]
    }
}

/// Boxes of a plot in a named world.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlotBounds {
    pub world_name: String,
    pub boxes: BTreeSet<PlotBox>,
}

/// Permissions and limits a client is given. Equality decides whether a resend is needed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Restrictions {
    pub allowed_permissions: BTreeSet<AxiomPermission>,
    pub denied_permissions: BTreeSet<AxiomPermission>,
    pub infinite_reach_limit: i32,
    pub bounds: BTreeSet<PlotBox>,
}

impl Default for Restrictions {
    /// Java `new Restrictions()`: default permission only, no denials, reach -1, no bounds.
    fn default() -> Self {
        Self {
            allowed_permissions: BTreeSet::from([AxiomPermission::Default]),
            denied_permissions: BTreeSet::new(),
            infinite_reach_limit: -1,
            bounds: BTreeSet::new(),
        }
    }
}

impl Restrictions {
    /// Java `Restrictions.toString()`, the text `/axiompaperdebug getRestrictions` shows: the permission sets by their
    /// Java enum names (the variants keep Java's order), the reach, and the plot boxes as the PlotSquared record prints
    /// them. The bounds are in the port's order, which Java's set does not fix.
    pub fn java_text(&self) -> String {
        let names = |set: &BTreeSet<AxiomPermission>| {
            set.iter()
                .map(|p| java_enum_name(&format!("{p:?}")))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let boxes = self
            .bounds
            .iter()
            .map(|b| {
                format!(
                    "PlotBox[min=({}, {}, {}), max=({}, {}, {})]",
                    b.min[0], b.min[1], b.min[2], b.max[0], b.max[1], b.max[2]
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "Restrictions{{allowedPermissions=[{}], deniedPermissions=[{}], infiniteReachLimit={}, bounds=[{}]}}",
            names(&self.allowed_permissions),
            names(&self.denied_permissions),
            self.infinite_reach_limit,
            boxes
        )
    }

    /// Restrictions granting `axiom.all` (what operators and `axiom.all` holders get).
    pub fn all(infinite_reach_limit: i32) -> Self {
        Self {
            allowed_permissions: BTreeSet::from([AxiomPermission::All]),
            denied_permissions: BTreeSet::new(),
            infinite_reach_limit,
            bounds: BTreeSet::new(),
        }
    }

    /// Encodes the `axiom:restrictions` payload.
    ///
    /// Layout: varint n, n x string (allowed, internal names); varint m, m x string (denied);
    /// i32 big-endian infinite reach limit; varint b, then b pairs of block positions (min, max),
    /// each clamped like the Java code.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();

        w.write_varint(self.allowed_permissions.len() as i32);
        for permission in &self.allowed_permissions {
            w.write_string(permission.internal_name());
        }

        w.write_varint(self.denied_permissions.len() as i32);
        for permission in &self.denied_permissions {
            w.write_string(permission.internal_name());
        }

        w.write_bytes(&self.infinite_reach_limit.to_be_bytes());

        w.write_varint(self.bounds.len() as i32);
        for bound in &self.bounds {
            let min = clamp_corner(bound.min);
            let max = clamp_corner(bound.max);
            w.write_block_pos((min[0], min[1], min[2]));
            w.write_block_pos((max[0], max[1], max[2]));
        }

        w.into_bytes()
    }
}

/// `EntitySpawn` to `ENTITY_SPAWN`, the Java constant name of a variant.
fn java_enum_name(variant: &str) -> String {
    let mut out = String::new();
    for (i, c) in variant.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.extend(c.to_uppercase());
    }
    out
}

fn clamp_corner(corner: [i32; 3]) -> [i32; 3] {
    [
        corner[0].clamp(-HORIZONTAL_LIMIT, HORIZONTAL_LIMIT),
        corner[1].clamp(-VERTICAL_LIMIT, VERTICAL_LIMIT),
        corner[2].clamp(-HORIZONTAL_LIMIT, HORIZONTAL_LIMIT),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buf::Reader;

    fn read_all_strings(r: &mut Reader, count: i32) -> Vec<String> {
        (0..count).map(|_| r.read_string(256).unwrap()).collect()
    }

    #[test]
    fn default_restrictions_payload_layout() {
        let bytes = Restrictions::default().encode();
        let mut r = Reader::new(&bytes);

        let allowed = r.read_varint().unwrap();
        assert_eq!(
            read_all_strings(&mut r, allowed),
            vec!["axiom.*".to_string()]
        );
        assert_eq!(r.read_varint().unwrap(), 0);
        assert_eq!(r.read_bytes(4).unwrap(), (-1i32).to_be_bytes().to_vec());
        assert_eq!(r.read_varint().unwrap(), 0);
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn all_restrictions_and_denials_are_written_in_ordinal_order() {
        let mut restrictions = Restrictions::all(256);
        restrictions.denied_permissions =
            BTreeSet::from([AxiomPermission::BuildPlace, AxiomPermission::EntitySpawn]);
        let bytes = restrictions.encode();
        let mut r = Reader::new(&bytes);

        assert_eq!(r.read_varint().unwrap(), 1);
        assert_eq!(r.read_string(256).unwrap(), "axiom.all");
        assert_eq!(r.read_varint().unwrap(), 2);
        // EntitySpawn (ordinal before BuildPlace) comes first.
        assert_eq!(r.read_string(256).unwrap(), "axiom.entity.spawn");
        assert_eq!(r.read_string(256).unwrap(), "axiom.build.place");
        assert_eq!(r.read_bytes(4).unwrap(), 256i32.to_be_bytes().to_vec());
        assert_eq!(r.read_varint().unwrap(), 0);
    }

    #[test]
    fn bounds_are_written_as_block_position_pairs_and_clamped() {
        let mut restrictions = Restrictions::default();
        restrictions.bounds = BTreeSet::from([PlotBox {
            min: [-100, -5000, 0],
            max: [i32::MAX, 5000, -33554432],
        }]);
        let bytes = restrictions.encode();
        let mut r = Reader::new(&bytes);

        assert_eq!(r.read_varint().unwrap(), 1);
        r.read_string(256).unwrap();
        assert_eq!(r.read_varint().unwrap(), 0);
        r.read_bytes(4).unwrap();
        assert_eq!(r.read_varint().unwrap(), 1);
        assert_eq!(r.read_block_pos().unwrap(), (-100, -2047, 0));
        assert_eq!(r.read_block_pos().unwrap(), (33554431, 2047, -33554431));
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn unbounded_box_detection() {
        let all = PlotBox {
            min: [i32::MIN; 3],
            max: [i32::MAX; 3],
        };
        assert!(all.is_unbounded());
        assert!(!PlotBox::ORIGIN.is_unbounded());
    }

    #[test]
    fn equal_restrictions_encode_equally() {
        let a = Restrictions::all(7);
        let b = Restrictions::all(7);
        assert_eq!(a, b);
        assert_eq!(a.encode(), b.encode());
        assert_ne!(a.encode(), Restrictions::all(8).encode());
    }

    #[test]
    fn java_text_names_permissions_like_the_enum_constants() {
        assert_eq!(
            Restrictions::all(-1).java_text(),
            "Restrictions{allowedPermissions=[ALL], deniedPermissions=[], infiniteReachLimit=-1, bounds=[]}"
        );
        let mut r = Restrictions::default();
        r.denied_permissions.insert(AxiomPermission::EntitySpawn);
        r.infinite_reach_limit = 8;
        r.bounds.insert(PlotBox { min: [0, 64, 0], max: [15, 80, 15] });
        assert_eq!(
            r.java_text(),
            "Restrictions{allowedPermissions=[DEFAULT], deniedPermissions=[ENTITY_SPAWN], infiniteReachLimit=8, \
             bounds=[PlotBox[min=(0, 64, 0), max=(15, 80, 15)]]}"
        );
    }
}
