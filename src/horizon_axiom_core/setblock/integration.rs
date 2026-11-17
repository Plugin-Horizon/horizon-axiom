// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): integration/Integration.java

//! Protection checks. The bypass permission is handled here, the plugin-specific checks come from the host.

use super::host::{BlockPos, SetBlockHost};
use super::permission::SectionPermissionChecker;

/// Bukkit permission that skips all region checks (Java `axiomadmin.bypass_region_checks`).
pub const BYPASS_REGION_CHECKS: &str = "axiomadmin.bypass_region_checks";

/// Java `Integration.canBreakBlock(player, block)`.
pub fn can_break_block(host: &dyn SetBlockHost, player: u128, dim: &str, pos: BlockPos) -> bool {
    if host.has_bukkit_permission(player, BYPASS_REGION_CHECKS) {
        return true;
    }
    host.region_can_break(player, dim, pos)
}

/// Java `Integration.canPlaceBlock(player, location)`.
pub fn can_place_block(host: &dyn SetBlockHost, player: u128, dim: &str, pos: BlockPos) -> bool {
    if host.has_bukkit_permission(player, BYPASS_REGION_CHECKS) {
        return true;
    }
    host.region_can_place(player, dim, pos)
}

/// Java `Integration.checkSection(player, world, cx, cy, cz)`.
pub fn check_section(host: &dyn SetBlockHost, player: u128, dim: &str, cx: i32, cy: i32, cz: i32) -> SectionPermissionChecker {
    if host.has_bukkit_permission(player, BYPASS_REGION_CHECKS) {
        return SectionPermissionChecker::All;
    }
    host.region_section(player, dim, cx, cy, cz)
}
