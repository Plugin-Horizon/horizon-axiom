//! The game's numeric block state ids, which Axiom's wire format uses (the block buffer palette, section reads,
//! blueprint states). The engine's `BlockState` is process-local, so the numbering is derived at runtime from the
//! engine's own block registry: vanilla numbers states by walking the blocks in registry order and, per block, its
//! states in property-permutation order (properties sorted by name, the first one varying slowest, values in the
//! order the block's definition lists them). No game data table ships with the mod.
//!
//! The table is built lazily by [`BlockTable::ensure`] from the first host with a complete registry and cached for
//! the process. Only `minecraft:` blocks are numbered: a mod's blocks come after vanilla's in the registry and the
//! client has no id for them, so they stay unmapped.

use std::collections::HashMap;
use std::sync::OnceLock;

/// One block as the host reports it: the registry id (`minecraft:oak_stairs`) and its properties with the legal
/// values, in the host's order.
pub struct RegistryBlock {
    pub id: String,
    pub properties: Vec<(String, Vec<String>)>,
}

static TABLE_ONCE: OnceLock<BlockTable> = OnceLock::new();

/// One block of the registry.
pub struct Block {
    /// The id without the namespace (`oak_stairs`).
    pub name: String,
    /// The first state id of the block.
    pub base: u32,
    /// Properties sorted by name with their legal values in state order.
    pub properties: Vec<(String, Vec<String>)>,
}

impl Block {
    fn count(&self) -> u32 {
        self.properties.iter().map(|(_, v)| v.len() as u32).product()
    }

    /// Whether the block has `property` with exactly the values `values`.
    pub fn has_property(&self, property: &str, values: &[&str]) -> bool {
        self.properties
            .iter()
            .any(|(name, legal)| name == property && legal.len() == values.len() && values.iter().all(|v| legal.iter().any(|l| l == v)))
    }
}

/// The vanilla block state numbering.
pub struct BlockTable {
    blocks: Vec<Block>,
    by_name: HashMap<String, usize>,
    total: u32,
}

impl BlockTable {
    /// The table built by [`BlockTable::ensure`]; an empty table (every id unknown) until a host built one.
    pub fn get() -> &'static BlockTable {
        static EMPTY: OnceLock<BlockTable> = OnceLock::new();
        TABLE_ONCE.get().unwrap_or_else(|| EMPTY.get_or_init(|| BlockTable::build(Vec::new())))
    }

    /// Builds the table from `registry` on the first call that passes a non-empty registry; later calls do nothing.
    /// `registry` is only asked while no table exists.
    pub fn ensure(registry: impl FnOnce() -> Vec<RegistryBlock>) {
        if TABLE_ONCE.get().is_some() {
            return;
        }
        let blocks = registry();
        if !blocks.is_empty() {
            let _ = TABLE_ONCE.set(BlockTable::build(blocks));
        }
    }

    /// Numbers `registry` (registry order): the `minecraft:` blocks only, each block's properties sorted by name.
    pub fn build(registry: Vec<RegistryBlock>) -> BlockTable {
        let mut blocks = Vec::new();
        let mut by_name = HashMap::new();
        let mut next = 0u32;
        for entry in registry {
            let Some(name) = entry.id.strip_prefix("minecraft:") else { continue };
            let mut properties = entry.properties;
            properties.sort_by(|a, b| a.0.cmp(&b.0));
            let block = Block { name: name.to_owned(), base: next, properties };
            next += block.count();
            by_name.insert(block.name.clone(), blocks.len());
            blocks.push(block);
        }
        BlockTable { blocks, by_name, total: next }
    }

    /// The number of states (`Block.BLOCK_STATE_REGISTRY.size()`).
    pub fn total(&self) -> u32 {
        self.total
    }

    /// The bit width of the global palette (`Mth.ceillog2(size)`).
    pub fn global_bits(&self) -> u8 {
        (u32::BITS - self.total.saturating_sub(1).leading_zeros()) as u8
    }

    /// The index of the block `id` belongs to, `None` past the last state.
    pub fn block_index(&self, id: u32) -> Option<usize> {
        if id >= self.total {
            return None;
        }
        Some(self.blocks.partition_point(|b| b.base <= id) - 1)
    }

    /// The block at `index`.
    pub fn block(&self, index: usize) -> &Block {
        &self.blocks[index]
    }

    /// The index of the block named `name` (`stone` or `minecraft:stone`).
    pub fn block_by_name(&self, name: &str) -> Option<usize> {
        self.by_name.get(name.strip_prefix("minecraft:").unwrap_or(name)).copied()
    }

    /// The block of `id` and its property values in the block's property order.
    pub fn describe(&self, id: u32) -> Option<(&Block, Vec<(&str, &str)>)> {
        let block = &self.blocks[self.block_index(id)?];
        let mut rest = id - block.base;
        let mut values = Vec::with_capacity(block.properties.len());
        for (name, legal) in block.properties.iter().rev() {
            let n = legal.len() as u32;
            values.push((name.as_str(), legal[(rest % n) as usize].as_str()));
            rest /= n;
        }
        values.reverse();
        Some((block, values))
    }

    /// The id of the state of `name` with `properties`; every property of the block has to be given.
    pub fn id_of(&self, name: &str, properties: &[(&str, &str)]) -> Option<u32> {
        let block = &self.blocks[self.block_by_name(name)?];
        if properties.len() != block.properties.len() {
            return None;
        }
        let mut offset = 0u32;
        for (property, legal) in &block.properties {
            let value = properties.iter().find(|(k, _)| *k == property.as_str())?.1;
            let index = legal.iter().position(|v| v == value)? as u32;
            offset = offset * legal.len() as u32 + index;
        }
        Some(block.base + offset)
    }

    /// The `minecraft:name[prop=value,...]` form `BlockStateParser` reads, the properties sorted by name.
    pub fn state_string(&self, id: u32) -> Option<String> {
        let (block, values) = self.describe(id)?;
        let mut out = format!("minecraft:{}", block.name);
        if !values.is_empty() {
            out.push('[');
            for (i, (k, v)) in values.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(k);
                out.push('=');
                out.push_str(v);
            }
            out.push(']');
        }
        Some(out)
    }

    /// The three air blocks, which `is_air` and the buffers' empty state use.
    pub fn is_air_block(name: &str) -> bool {
        matches!(name, "air" | "cave_air" | "void_air")
    }

    /// `BlockState.isAir()`.
    pub fn is_air(&self, id: u32) -> bool {
        self.block_index(id).is_some_and(|i| Self::is_air_block(&self.blocks[i].name))
    }

    /// The state id of the empty-state sentinel of block buffers (`void_air`).
    pub fn void_air(&self) -> u32 {
        self.block_by_name("void_air").map_or(0, |i| self.blocks[i].base)
    }

    /// `blockState.getBlock() instanceof LiquidBlock`: water and lava.
    pub fn is_liquid_block(&self, id: u32) -> bool {
        self.block_index(id).is_some_and(|i| matches!(self.blocks[i].name.as_str(), "water" | "lava"))
    }

    /// `blockState.getFluidState().isEmpty()`: no fluid block, not waterlogged, and not one of the blocks that hold
    /// still water (bubble column, kelp, seagrass).
    pub fn fluid_is_empty(&self, id: u32) -> bool {
        let Some((block, values)) = self.describe(id) else { return true };
        if matches!(
            block.name.as_str(),
            "water" | "lava" | "bubble_column" | "kelp" | "kelp_plant" | "seagrass" | "tall_seagrass"
        ) {
            return false;
        }
        !values.iter().any(|(k, v)| *k == "waterlogged" && *v == "true")
    }

    /// `BedBlock`, `DoublePlantBlock` or `DoorBlock`: the beds (`part` head/foot) and every block whose `half` is
    /// upper/lower (doors, tall plants).
    pub fn is_bed_double_plant_or_door(&self, id: u32) -> bool {
        self.block_index(id).is_some_and(|i| {
            let block = &self.blocks[i];
            block.has_property("part", &["foot", "head"]) || block.has_property("half", &["upper", "lower"])
        })
    }

    /// `blockEntity instanceof GameMasterBlock`: the command blocks, jigsaw and structure blocks.
    pub fn is_game_master_block(&self, id: u32) -> bool {
        self.block_index(id).is_some_and(|i| {
            matches!(
                self.blocks[i].name.as_str(),
                "command_block"
                    | "chain_command_block"
                    | "repeating_command_block"
                    | "jigsaw"
                    | "structure_block"
                    | "test_block"
                    | "test_instance_block"
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(id: &str, props: &[(&str, &[&str])]) -> RegistryBlock {
        RegistryBlock {
            id: id.to_owned(),
            properties: props
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.iter().map(|s| (*s).to_owned()).collect()))
                .collect(),
        }
    }

    const BOOL: &[&str] = &["true", "false"];

    /// A synthetic registry (not the game's): properties deliberately listed out of name order.
    fn table() -> BlockTable {
        BlockTable::build(vec![
            block("minecraft:air", &[]),
            block("minecraft:stone", &[]),
            block("minecraft:void_air", &[]),
            block("minecraft:water", &[("level", &["0", "1", "2"])]),
            block(
                "minecraft:stairs",
                &[("waterlogged", BOOL), ("half", &["top", "bottom"]), ("facing", &["north", "south"])],
            ),
            block("minecraft:chest", &[("type", &["single", "left"]), ("facing", &["north", "south"])]),
            block("minecraft:door", &[("half", &["upper", "lower"])]),
            block("minecraft:bed", &[("part", &["head", "foot"])]),
            block("minecraft:command_block", &[]),
            block("mymod:shaft", &[("lit", BOOL)]),
        ])
    }

    #[test]
    fn counts_follow_the_registry() {
        let t = table();
        // 1+1+1+3+8+4+2+2+1 states; the mod block is not numbered.
        assert_eq!(t.total(), 23);
        assert_eq!(t.global_bits(), 5);
        assert_eq!(t.block_by_name("mymod:shaft"), None);
        assert_eq!(BlockTable::build(Vec::new()).global_bits(), 0);
    }

    #[test]
    fn known_ids() {
        let t = table();
        assert_eq!(t.id_of("air", &[]), Some(0));
        assert_eq!(t.id_of("minecraft:stone", &[]), Some(1));
        let base = t.block(t.block_by_name("stairs").unwrap()).base;
        // Sorted names: facing (slowest), half, waterlogged (fastest).
        let id = |f, h, w| t.id_of("stairs", &[("waterlogged", w), ("facing", f), ("half", h)]);
        assert_eq!(id("north", "top", "true"), Some(base));
        assert_eq!(id("north", "top", "false"), Some(base + 1));
        assert_eq!(id("north", "bottom", "true"), Some(base + 2));
        assert_eq!(id("south", "top", "true"), Some(base + 4));
        assert_eq!(
            t.state_string(base + 5).as_deref(),
            Some("minecraft:stairs[facing=south,half=top,waterlogged=false]")
        );
    }

    #[test]
    fn chest_is_numbered_by_sorted_property_names() {
        let t = table();
        let first = t.block(t.block_by_name("chest").unwrap()).base;
        assert_eq!(t.describe(first + 1).unwrap().1, vec![("facing", "north"), ("type", "left")]);
        assert_eq!(t.id_of("chest", &[("type", "left"), ("facing", "north")]), Some(first + 1));
    }

    #[test]
    fn every_id_round_trips() {
        let t = table();
        for id in 0..t.total() {
            let (block, values) = t.describe(id).unwrap();
            assert_eq!(t.id_of(&block.name, &values), Some(id), "{id}");
        }
        assert!(t.describe(t.total()).is_none());
        assert_eq!(t.id_of("stone", &[("facing", "north")]), None);
        assert_eq!(t.id_of("not_a_block", &[]), None);
    }

    #[test]
    fn block_classes() {
        let t = table();
        let id = |n: &str| t.block(t.block_by_name(n).unwrap()).base;
        assert!(t.is_air(id("air")) && t.is_air(id("void_air")) && !t.is_air(id("stone")));
        assert!(t.is_liquid_block(id("water")) && !t.is_liquid_block(id("stone")));
        assert!(!t.fluid_is_empty(id("water")) && t.fluid_is_empty(id("stone")));
        let wet = t.id_of("stairs", &[("facing", "north"), ("half", "top"), ("waterlogged", "true")]).unwrap();
        assert!(!t.fluid_is_empty(wet));
        assert!(t.is_bed_double_plant_or_door(id("door")) && t.is_bed_double_plant_or_door(id("bed")));
        assert!(!t.is_bed_double_plant_or_door(id("stairs")));
        assert!(t.is_game_master_block(id("command_block")) && !t.is_game_master_block(id("chest")));
        assert_eq!(t.void_air(), id("void_air"));
    }

    #[test]
    fn ensure_builds_once_and_ignores_an_empty_registry() {
        BlockTable::ensure(Vec::new);
        assert_eq!(BlockTable::get().total(), 0);
        BlockTable::ensure(|| vec![block("minecraft:air", &[])]);
        BlockTable::ensure(|| panic!("asked again"));
        assert_eq!(BlockTable::get().total(), 1);
    }
}
