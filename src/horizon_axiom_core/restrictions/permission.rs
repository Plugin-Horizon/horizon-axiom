// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): restrictions/AxiomPermission.java
// Generated from the Java enum table; variant order equals Java ordinal order (BTreeSet order relies on it).

/// Axiom permission nodes. `parent` is the wildcard node that implies this one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AxiomPermission {
    /// `axiom.all`
    All,
    /// `axiom.entity.*`
    Entity,
    /// `axiom.entity.spawn`
    EntitySpawn,
    /// `axiom.entity.manipulate`
    EntityManipulate,
    /// `axiom.entity.delete`
    EntityDelete,
    /// `axiom.entity.request_data`
    EntityRequestdata,
    /// `axiom.blueprint.*`
    Blueprint,
    /// `axiom.blueprint.upload`
    BlueprintUpload,
    /// `axiom.blueprint.request`
    BlueprintRequest,
    /// `axiom.blueprint.manifest`
    BlueprintManifest,
    /// `axiom.annotation.*`
    Annotation,
    /// `axiom.annotation.create`
    AnnotationCreate,
    /// `axiom.annotation.clear_all`
    AnnotationClearall,
    /// `axiom.build_dangerous.*`
    BuildDangerous,
    /// `axiom.build_dangerous.tick`
    BuildDangerousTick,
    /// `axiom.default`
    Default,
    /// `axiom.use`
    Use,
    /// `axiom.allow_copying_other_plots`
    AllowCopyingOtherPlots,
    /// `axiom.can_import_blocks`
    CanImportBlocks,
    /// `axiom.can_export_blocks`
    CanExportBlocks,
    /// `axiom.chunk.*`
    Chunk,
    /// `axiom.chunk.request`
    ChunkRequest,
    /// `axiom.chunk.request_block_entity`
    ChunkRequestblockentity,
    /// `axiom.build.*`
    Build,
    /// `axiom.build.place`
    BuildPlace,
    /// `axiom.build.section`
    BuildSection,
    /// `axiom.build.nbt`
    BuildNbt,
    /// `axiom.editor.*`
    Editor,
    /// `axiom.editor.use`
    EditorUse,
    /// `axiom.editor.views`
    EditorViews,
    /// `axiom.player.*`
    Player,
    /// `axiom.player.bypass_movement_restrictions`
    PlayerBypassMovementRestrictions,
    /// `axiom.player.speed`
    PlayerSpeed,
    /// `axiom.player.teleport`
    PlayerTeleport,
    /// `axiom.player.gamemode.*`
    PlayerGamemode,
    /// `axiom.player.gamemode.survival`
    PlayerGamemodeSurvival,
    /// `axiom.player.gamemode.creative`
    PlayerGamemodeCreative,
    /// `axiom.player.gamemode.spectator`
    PlayerGamemodeSpectator,
    /// `axiom.player.gamemode.adventure`
    PlayerGamemodeAdventure,
    /// `axiom.player.hotbar`
    PlayerHotbar,
    /// `axiom.player.set_no_physical_trigger`
    PlayerSetnophysicaltrigger,
    /// `axiom.world.*`
    World,
    /// `axiom.world.time`
    WorldTime,
    /// `axiom.world.property`
    WorldProperty,
    /// `axiom.capability.*`
    Capability,
    /// `axiom.capability.bulldozer`
    Bulldozer,
    /// `axiom.capability.replace_mode`
    ReplaceMode,
    /// `axiom.capability.force_place`
    ForcePlace,
    /// `axiom.capability.no_updates`
    NoUpdates,
    /// `axiom.capability.tinker`
    Tinker,
    /// `axiom.capability.infinite_reach`
    InfiniteReach,
    /// `axiom.capability.fast_place`
    FastPlace,
    /// `axiom.capability.angel_placement`
    AngelPlacement,
    /// `axiom.capability.no_clip`
    NoClip,
    /// `axiom.capability.phantom`
    Phantom,
    /// `axiom.tool.*`
    Tool,
    /// `axiom.tool.magic_select`
    ToolMagicselect,
    /// `axiom.tool.box_select`
    ToolBoxselect,
    /// `axiom.tool.freehand_select`
    ToolFreehandselect,
    /// `axiom.tool.lasso_select`
    ToolLassoselect,
    /// `axiom.tool.ruler`
    ToolRuler,
    /// `axiom.tool.annotation`
    ToolAnnotation,
    /// `axiom.tool.painter`
    ToolPainter,
    /// `axiom.tool.noise_painter`
    ToolNoisepainter,
    /// `axiom.tool.biome_painter`
    ToolBiomepainter,
    /// `axiom.tool.gradient_painter`
    ToolGradientpainter,
    /// `axiom.tool.script_brush`
    ToolScriptbrush,
    /// `axiom.tool.freehand_draw`
    ToolFreehanddraw,
    /// `axiom.tool.sculpt_draw`
    ToolSculptdraw,
    /// `axiom.tool.rock`
    ToolRock,
    /// `axiom.tool.weld`
    ToolWeld,
    /// `axiom.tool.melt`
    ToolMelt,
    /// `axiom.tool.stamp`
    ToolStamp,
    /// `axiom.tool.text`
    ToolText,
    /// `axiom.tool.shape`
    ToolShape,
    /// `axiom.tool.path`
    ToolPath,
    /// `axiom.tool.modelling`
    ToolModelling,
    /// `axiom.tool.floodfill`
    ToolFloodfill,
    /// `axiom.tool.fluidball`
    ToolFluidball,
    /// `axiom.tool.elevation`
    ToolElevation,
    /// `axiom.tool.slope`
    ToolSlope,
    /// `axiom.tool.smooth`
    ToolSmooth,
    /// `axiom.tool.distort`
    ToolDistort,
    /// `axiom.tool.roughen`
    ToolRoughen,
    /// `axiom.tool.shatter`
    ToolShatter,
    /// `axiom.tool.extrude`
    ToolExtrude,
    /// `axiom.tool.modify`
    ToolModify,
    /// `axiom.builder_tool.*`
    Buildertool,
    /// `axiom.builder_tool.move`
    BuildertoolMove,
    /// `axiom.builder_tool.clone`
    BuildertoolClone,
    /// `axiom.builder_tool.stack`
    BuildertoolStack,
    /// `axiom.builder_tool.smear`
    BuildertoolSmear,
    /// `axiom.builder_tool.extrude`
    BuildertoolExtrude,
    /// `axiom.builder_tool.erase`
    BuildertoolErase,
    /// `axiom.builder_tool.setup_symmetry`
    BuildertoolSetupsymmetry,
}

/// Every permission in ordinal order.
pub const ALL_PERMISSIONS: [AxiomPermission; 95] = [
    AxiomPermission::All,
    AxiomPermission::Entity,
    AxiomPermission::EntitySpawn,
    AxiomPermission::EntityManipulate,
    AxiomPermission::EntityDelete,
    AxiomPermission::EntityRequestdata,
    AxiomPermission::Blueprint,
    AxiomPermission::BlueprintUpload,
    AxiomPermission::BlueprintRequest,
    AxiomPermission::BlueprintManifest,
    AxiomPermission::Annotation,
    AxiomPermission::AnnotationCreate,
    AxiomPermission::AnnotationClearall,
    AxiomPermission::BuildDangerous,
    AxiomPermission::BuildDangerousTick,
    AxiomPermission::Default,
    AxiomPermission::Use,
    AxiomPermission::AllowCopyingOtherPlots,
    AxiomPermission::CanImportBlocks,
    AxiomPermission::CanExportBlocks,
    AxiomPermission::Chunk,
    AxiomPermission::ChunkRequest,
    AxiomPermission::ChunkRequestblockentity,
    AxiomPermission::Build,
    AxiomPermission::BuildPlace,
    AxiomPermission::BuildSection,
    AxiomPermission::BuildNbt,
    AxiomPermission::Editor,
    AxiomPermission::EditorUse,
    AxiomPermission::EditorViews,
    AxiomPermission::Player,
    AxiomPermission::PlayerBypassMovementRestrictions,
    AxiomPermission::PlayerSpeed,
    AxiomPermission::PlayerTeleport,
    AxiomPermission::PlayerGamemode,
    AxiomPermission::PlayerGamemodeSurvival,
    AxiomPermission::PlayerGamemodeCreative,
    AxiomPermission::PlayerGamemodeSpectator,
    AxiomPermission::PlayerGamemodeAdventure,
    AxiomPermission::PlayerHotbar,
    AxiomPermission::PlayerSetnophysicaltrigger,
    AxiomPermission::World,
    AxiomPermission::WorldTime,
    AxiomPermission::WorldProperty,
    AxiomPermission::Capability,
    AxiomPermission::Bulldozer,
    AxiomPermission::ReplaceMode,
    AxiomPermission::ForcePlace,
    AxiomPermission::NoUpdates,
    AxiomPermission::Tinker,
    AxiomPermission::InfiniteReach,
    AxiomPermission::FastPlace,
    AxiomPermission::AngelPlacement,
    AxiomPermission::NoClip,
    AxiomPermission::Phantom,
    AxiomPermission::Tool,
    AxiomPermission::ToolMagicselect,
    AxiomPermission::ToolBoxselect,
    AxiomPermission::ToolFreehandselect,
    AxiomPermission::ToolLassoselect,
    AxiomPermission::ToolRuler,
    AxiomPermission::ToolAnnotation,
    AxiomPermission::ToolPainter,
    AxiomPermission::ToolNoisepainter,
    AxiomPermission::ToolBiomepainter,
    AxiomPermission::ToolGradientpainter,
    AxiomPermission::ToolScriptbrush,
    AxiomPermission::ToolFreehanddraw,
    AxiomPermission::ToolSculptdraw,
    AxiomPermission::ToolRock,
    AxiomPermission::ToolWeld,
    AxiomPermission::ToolMelt,
    AxiomPermission::ToolStamp,
    AxiomPermission::ToolText,
    AxiomPermission::ToolShape,
    AxiomPermission::ToolPath,
    AxiomPermission::ToolModelling,
    AxiomPermission::ToolFloodfill,
    AxiomPermission::ToolFluidball,
    AxiomPermission::ToolElevation,
    AxiomPermission::ToolSlope,
    AxiomPermission::ToolSmooth,
    AxiomPermission::ToolDistort,
    AxiomPermission::ToolRoughen,
    AxiomPermission::ToolShatter,
    AxiomPermission::ToolExtrude,
    AxiomPermission::ToolModify,
    AxiomPermission::Buildertool,
    AxiomPermission::BuildertoolMove,
    AxiomPermission::BuildertoolClone,
    AxiomPermission::BuildertoolStack,
    AxiomPermission::BuildertoolSmear,
    AxiomPermission::BuildertoolExtrude,
    AxiomPermission::BuildertoolErase,
    AxiomPermission::BuildertoolSetupsymmetry,
];

impl AxiomPermission {
    /// Permission node as checked against the server's permission system.
    pub const fn node(self) -> &'static str {
        match self {
            AxiomPermission::All => "axiom.all",
            AxiomPermission::Entity => "axiom.entity.*",
            AxiomPermission::EntitySpawn => "axiom.entity.spawn",
            AxiomPermission::EntityManipulate => "axiom.entity.manipulate",
            AxiomPermission::EntityDelete => "axiom.entity.delete",
            AxiomPermission::EntityRequestdata => "axiom.entity.request_data",
            AxiomPermission::Blueprint => "axiom.blueprint.*",
            AxiomPermission::BlueprintUpload => "axiom.blueprint.upload",
            AxiomPermission::BlueprintRequest => "axiom.blueprint.request",
            AxiomPermission::BlueprintManifest => "axiom.blueprint.manifest",
            AxiomPermission::Annotation => "axiom.annotation.*",
            AxiomPermission::AnnotationCreate => "axiom.annotation.create",
            AxiomPermission::AnnotationClearall => "axiom.annotation.clear_all",
            AxiomPermission::BuildDangerous => "axiom.build_dangerous.*",
            AxiomPermission::BuildDangerousTick => "axiom.build_dangerous.tick",
            AxiomPermission::Default => "axiom.default",
            AxiomPermission::Use => "axiom.use",
            AxiomPermission::AllowCopyingOtherPlots => "axiom.allow_copying_other_plots",
            AxiomPermission::CanImportBlocks => "axiom.can_import_blocks",
            AxiomPermission::CanExportBlocks => "axiom.can_export_blocks",
            AxiomPermission::Chunk => "axiom.chunk.*",
            AxiomPermission::ChunkRequest => "axiom.chunk.request",
            AxiomPermission::ChunkRequestblockentity => "axiom.chunk.request_block_entity",
            AxiomPermission::Build => "axiom.build.*",
            AxiomPermission::BuildPlace => "axiom.build.place",
            AxiomPermission::BuildSection => "axiom.build.section",
            AxiomPermission::BuildNbt => "axiom.build.nbt",
            AxiomPermission::Editor => "axiom.editor.*",
            AxiomPermission::EditorUse => "axiom.editor.use",
            AxiomPermission::EditorViews => "axiom.editor.views",
            AxiomPermission::Player => "axiom.player.*",
            AxiomPermission::PlayerBypassMovementRestrictions => {
                "axiom.player.bypass_movement_restrictions"
            }
            AxiomPermission::PlayerSpeed => "axiom.player.speed",
            AxiomPermission::PlayerTeleport => "axiom.player.teleport",
            AxiomPermission::PlayerGamemode => "axiom.player.gamemode.*",
            AxiomPermission::PlayerGamemodeSurvival => "axiom.player.gamemode.survival",
            AxiomPermission::PlayerGamemodeCreative => "axiom.player.gamemode.creative",
            AxiomPermission::PlayerGamemodeSpectator => "axiom.player.gamemode.spectator",
            AxiomPermission::PlayerGamemodeAdventure => "axiom.player.gamemode.adventure",
            AxiomPermission::PlayerHotbar => "axiom.player.hotbar",
            AxiomPermission::PlayerSetnophysicaltrigger => "axiom.player.set_no_physical_trigger",
            AxiomPermission::World => "axiom.world.*",
            AxiomPermission::WorldTime => "axiom.world.time",
            AxiomPermission::WorldProperty => "axiom.world.property",
            AxiomPermission::Capability => "axiom.capability.*",
            AxiomPermission::Bulldozer => "axiom.capability.bulldozer",
            AxiomPermission::ReplaceMode => "axiom.capability.replace_mode",
            AxiomPermission::ForcePlace => "axiom.capability.force_place",
            AxiomPermission::NoUpdates => "axiom.capability.no_updates",
            AxiomPermission::Tinker => "axiom.capability.tinker",
            AxiomPermission::InfiniteReach => "axiom.capability.infinite_reach",
            AxiomPermission::FastPlace => "axiom.capability.fast_place",
            AxiomPermission::AngelPlacement => "axiom.capability.angel_placement",
            AxiomPermission::NoClip => "axiom.capability.no_clip",
            AxiomPermission::Phantom => "axiom.capability.phantom",
            AxiomPermission::Tool => "axiom.tool.*",
            AxiomPermission::ToolMagicselect => "axiom.tool.magic_select",
            AxiomPermission::ToolBoxselect => "axiom.tool.box_select",
            AxiomPermission::ToolFreehandselect => "axiom.tool.freehand_select",
            AxiomPermission::ToolLassoselect => "axiom.tool.lasso_select",
            AxiomPermission::ToolRuler => "axiom.tool.ruler",
            AxiomPermission::ToolAnnotation => "axiom.tool.annotation",
            AxiomPermission::ToolPainter => "axiom.tool.painter",
            AxiomPermission::ToolNoisepainter => "axiom.tool.noise_painter",
            AxiomPermission::ToolBiomepainter => "axiom.tool.biome_painter",
            AxiomPermission::ToolGradientpainter => "axiom.tool.gradient_painter",
            AxiomPermission::ToolScriptbrush => "axiom.tool.script_brush",
            AxiomPermission::ToolFreehanddraw => "axiom.tool.freehand_draw",
            AxiomPermission::ToolSculptdraw => "axiom.tool.sculpt_draw",
            AxiomPermission::ToolRock => "axiom.tool.rock",
            AxiomPermission::ToolWeld => "axiom.tool.weld",
            AxiomPermission::ToolMelt => "axiom.tool.melt",
            AxiomPermission::ToolStamp => "axiom.tool.stamp",
            AxiomPermission::ToolText => "axiom.tool.text",
            AxiomPermission::ToolShape => "axiom.tool.shape",
            AxiomPermission::ToolPath => "axiom.tool.path",
            AxiomPermission::ToolModelling => "axiom.tool.modelling",
            AxiomPermission::ToolFloodfill => "axiom.tool.floodfill",
            AxiomPermission::ToolFluidball => "axiom.tool.fluidball",
            AxiomPermission::ToolElevation => "axiom.tool.elevation",
            AxiomPermission::ToolSlope => "axiom.tool.slope",
            AxiomPermission::ToolSmooth => "axiom.tool.smooth",
            AxiomPermission::ToolDistort => "axiom.tool.distort",
            AxiomPermission::ToolRoughen => "axiom.tool.roughen",
            AxiomPermission::ToolShatter => "axiom.tool.shatter",
            AxiomPermission::ToolExtrude => "axiom.tool.extrude",
            AxiomPermission::ToolModify => "axiom.tool.modify",
            AxiomPermission::Buildertool => "axiom.builder_tool.*",
            AxiomPermission::BuildertoolMove => "axiom.builder_tool.move",
            AxiomPermission::BuildertoolClone => "axiom.builder_tool.clone",
            AxiomPermission::BuildertoolStack => "axiom.builder_tool.stack",
            AxiomPermission::BuildertoolSmear => "axiom.builder_tool.smear",
            AxiomPermission::BuildertoolExtrude => "axiom.builder_tool.extrude",
            AxiomPermission::BuildertoolErase => "axiom.builder_tool.erase",
            AxiomPermission::BuildertoolSetupsymmetry => "axiom.builder_tool.setup_symmetry",
        }
    }

    /// The wildcard permission that implies this one, if any (Java `parent`).
    pub const fn parent(self) -> Option<AxiomPermission> {
        match self {
            AxiomPermission::All => None,
            AxiomPermission::Entity => None,
            AxiomPermission::EntitySpawn => Some(AxiomPermission::Entity),
            AxiomPermission::EntityManipulate => Some(AxiomPermission::Entity),
            AxiomPermission::EntityDelete => Some(AxiomPermission::Entity),
            AxiomPermission::EntityRequestdata => Some(AxiomPermission::Entity),
            AxiomPermission::Blueprint => None,
            AxiomPermission::BlueprintUpload => Some(AxiomPermission::Blueprint),
            AxiomPermission::BlueprintRequest => Some(AxiomPermission::Blueprint),
            AxiomPermission::BlueprintManifest => Some(AxiomPermission::Blueprint),
            AxiomPermission::Annotation => None,
            AxiomPermission::AnnotationCreate => Some(AxiomPermission::Annotation),
            AxiomPermission::AnnotationClearall => Some(AxiomPermission::Annotation),
            AxiomPermission::BuildDangerous => None,
            AxiomPermission::BuildDangerousTick => None,
            AxiomPermission::Default => None,
            AxiomPermission::Use => Some(AxiomPermission::Default),
            AxiomPermission::AllowCopyingOtherPlots => Some(AxiomPermission::Default),
            AxiomPermission::CanImportBlocks => Some(AxiomPermission::Default),
            AxiomPermission::CanExportBlocks => Some(AxiomPermission::Default),
            AxiomPermission::Chunk => Some(AxiomPermission::Default),
            AxiomPermission::ChunkRequest => Some(AxiomPermission::Chunk),
            AxiomPermission::ChunkRequestblockentity => Some(AxiomPermission::Chunk),
            AxiomPermission::Build => Some(AxiomPermission::Default),
            AxiomPermission::BuildPlace => Some(AxiomPermission::Build),
            AxiomPermission::BuildSection => Some(AxiomPermission::Build),
            AxiomPermission::BuildNbt => Some(AxiomPermission::Build),
            AxiomPermission::Editor => Some(AxiomPermission::Default),
            AxiomPermission::EditorUse => Some(AxiomPermission::Editor),
            AxiomPermission::EditorViews => Some(AxiomPermission::Editor),
            AxiomPermission::Player => Some(AxiomPermission::Default),
            AxiomPermission::PlayerBypassMovementRestrictions => Some(AxiomPermission::Player),
            AxiomPermission::PlayerSpeed => Some(AxiomPermission::Player),
            AxiomPermission::PlayerTeleport => Some(AxiomPermission::Player),
            AxiomPermission::PlayerGamemode => Some(AxiomPermission::Player),
            AxiomPermission::PlayerGamemodeSurvival => Some(AxiomPermission::PlayerGamemode),
            AxiomPermission::PlayerGamemodeCreative => Some(AxiomPermission::PlayerGamemode),
            AxiomPermission::PlayerGamemodeSpectator => Some(AxiomPermission::PlayerGamemode),
            AxiomPermission::PlayerGamemodeAdventure => Some(AxiomPermission::PlayerGamemode),
            AxiomPermission::PlayerHotbar => Some(AxiomPermission::Player),
            AxiomPermission::PlayerSetnophysicaltrigger => Some(AxiomPermission::Player),
            AxiomPermission::World => Some(AxiomPermission::Default),
            AxiomPermission::WorldTime => Some(AxiomPermission::World),
            AxiomPermission::WorldProperty => Some(AxiomPermission::World),
            AxiomPermission::Capability => Some(AxiomPermission::Default),
            AxiomPermission::Bulldozer => Some(AxiomPermission::Capability),
            AxiomPermission::ReplaceMode => Some(AxiomPermission::Capability),
            AxiomPermission::ForcePlace => Some(AxiomPermission::Capability),
            AxiomPermission::NoUpdates => Some(AxiomPermission::Capability),
            AxiomPermission::Tinker => Some(AxiomPermission::Capability),
            AxiomPermission::InfiniteReach => Some(AxiomPermission::Capability),
            AxiomPermission::FastPlace => Some(AxiomPermission::Capability),
            AxiomPermission::AngelPlacement => Some(AxiomPermission::Capability),
            AxiomPermission::NoClip => Some(AxiomPermission::Capability),
            AxiomPermission::Phantom => Some(AxiomPermission::Capability),
            AxiomPermission::Tool => Some(AxiomPermission::Default),
            AxiomPermission::ToolMagicselect => Some(AxiomPermission::Tool),
            AxiomPermission::ToolBoxselect => Some(AxiomPermission::Tool),
            AxiomPermission::ToolFreehandselect => Some(AxiomPermission::Tool),
            AxiomPermission::ToolLassoselect => Some(AxiomPermission::Tool),
            AxiomPermission::ToolRuler => Some(AxiomPermission::Tool),
            AxiomPermission::ToolAnnotation => Some(AxiomPermission::Tool),
            AxiomPermission::ToolPainter => Some(AxiomPermission::Tool),
            AxiomPermission::ToolNoisepainter => Some(AxiomPermission::Tool),
            AxiomPermission::ToolBiomepainter => Some(AxiomPermission::Tool),
            AxiomPermission::ToolGradientpainter => Some(AxiomPermission::Tool),
            AxiomPermission::ToolScriptbrush => Some(AxiomPermission::Tool),
            AxiomPermission::ToolFreehanddraw => Some(AxiomPermission::Tool),
            AxiomPermission::ToolSculptdraw => Some(AxiomPermission::Tool),
            AxiomPermission::ToolRock => Some(AxiomPermission::Tool),
            AxiomPermission::ToolWeld => Some(AxiomPermission::Tool),
            AxiomPermission::ToolMelt => Some(AxiomPermission::Tool),
            AxiomPermission::ToolStamp => Some(AxiomPermission::Tool),
            AxiomPermission::ToolText => Some(AxiomPermission::Tool),
            AxiomPermission::ToolShape => Some(AxiomPermission::Tool),
            AxiomPermission::ToolPath => Some(AxiomPermission::Tool),
            AxiomPermission::ToolModelling => Some(AxiomPermission::Tool),
            AxiomPermission::ToolFloodfill => Some(AxiomPermission::Tool),
            AxiomPermission::ToolFluidball => Some(AxiomPermission::Tool),
            AxiomPermission::ToolElevation => Some(AxiomPermission::Tool),
            AxiomPermission::ToolSlope => Some(AxiomPermission::Tool),
            AxiomPermission::ToolSmooth => Some(AxiomPermission::Tool),
            AxiomPermission::ToolDistort => Some(AxiomPermission::Tool),
            AxiomPermission::ToolRoughen => Some(AxiomPermission::Tool),
            AxiomPermission::ToolShatter => Some(AxiomPermission::Tool),
            AxiomPermission::ToolExtrude => Some(AxiomPermission::Tool),
            AxiomPermission::ToolModify => Some(AxiomPermission::Tool),
            AxiomPermission::Buildertool => Some(AxiomPermission::Default),
            AxiomPermission::BuildertoolMove => Some(AxiomPermission::Buildertool),
            AxiomPermission::BuildertoolClone => Some(AxiomPermission::Buildertool),
            AxiomPermission::BuildertoolStack => Some(AxiomPermission::Buildertool),
            AxiomPermission::BuildertoolSmear => Some(AxiomPermission::Buildertool),
            AxiomPermission::BuildertoolExtrude => Some(AxiomPermission::Buildertool),
            AxiomPermission::BuildertoolErase => Some(AxiomPermission::Buildertool),
            AxiomPermission::BuildertoolSetupsymmetry => Some(AxiomPermission::Buildertool),
        }
    }
}

impl AxiomPermission {
    /// Name as sent in the `axiom:restrictions` payload. The DEFAULT node is sent as the wildcard `axiom.*`.
    pub fn internal_name(self) -> &'static str {
        if self == AxiomPermission::Default {
            "axiom.*"
        } else {
            self.node()
        }
    }
}
