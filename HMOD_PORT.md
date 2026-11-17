# hmod port: axiom
Round 1
- Fixed: src/lib.rs `entity_type_id`, `saved_entity`, `entity_info`: `EntityTypeKey` now mapped to its id string with `kind.id(host)` (was `as_str` / used as `String`).
- Fixed: src/lib.rs `Inbox` payload handler: `handle` returns `PayloadHandleError` (import added), context error wrapped in `PayloadHandleError::Failed`.
- Fixed: src/lib.rs `kick_player`: `PlayerRef::kick` is now `PlayerRef::disconnect` (returns `Result<bool, TextFormError>`).
- Fixed: src/lib.rs `permission_fallback`: `PlayerRef::has_permission` removed; operator-level fallback (level 2) kept via `permission_level`.
- Fixed: src/lib.rs `RestrictionsHost::permission_value`: `PlayerRef::permission_value` removed, always `None` (see gap).
- Permission nodes: none declared. String nodes (`axiom.*`, `axiomlimits.<key>`) are not migrated to typed nodes; see gap.
- Gaps: HMOD-GAP(permission-provider-answers): a typed node query (`permission_bool`) needs `&mut dyn ServerHost`, but every permission check in the core runs through `&self` host trait methods (`RestrictionsHost`, `EntityHost`, `RequestHost`, `AnnotationHost`, ...). Provider answers (LuckPerms) are lost. Only the operator-level fallback (level 2) remains, so non-operators get no axiom.* permission and Axiom is effectively operator-only. This guts the non-operator path; the fix needs the core traits to take `&mut self` (a refactor) or interior mutability of the host, which is not done here.
- Unsure:
  - `PayloadHandleError` is imported from the `mod_api` root (assumed re-exported there, as worldedit does).
  - `PlayerRef::disconnect` and `EntityTypeKey::id` are inferred from the generator's `player_<x>` / `entity_type_key_id` naming, not compiled.
  - The gap stubs keep the old operator-level-2 fallback in `permission_fallback`; that level was taken from the old comment.
