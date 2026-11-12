// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): packet/impl/SetTimePacketListener.java,
// SetGamemodePacketListener.java, SetFlySpeedPacketListener.java, TeleportPacketListener.java,
// SetNoPhysicalTriggerPacketListener.java, SetWorldPropertyListener.java.
//
// Every handler takes the decoded payload bytes of one plugin message. Permission checks come first and a
// failed check returns Ok without reading the rest, as in Java. An Err means a malformed packet: the glue
// kicks the player.

use crate::buf::{Reader, Writer};

use super::{
    can_use, read_byte_array, read_identifier, GameMode, Location, WorldError, WorldHost,
    WorldState, CH_ACK_WORLD_PROPERTIES, PERM_PLAYER_SET_NO_PHYSICAL_TRIGGER, PERM_PLAYER_SPEED,
    PERM_PLAYER_TELEPORT, PERM_WORLD_PROPERTY, PERM_WORLD_TIME,
};

/// `set_world_time`: `dim`, optional time of day, optional freeze flag. Both absent is a no-op.
pub fn set_world_time(
    host: &mut dyn WorldHost,
    player: u128,
    data: &[u8],
) -> Result<(), WorldError> {
    if !can_use(host, player, PERM_WORLD_TIME) {
        return Ok(());
    }

    let mut r = Reader::new(data);
    let key = read_identifier(&mut r)?;
    let time = r.read_nullable(|r| r.read_i32())?;
    let freeze = r.read_nullable(|r| r.read_bool())?;

    if time.is_none() && freeze.is_none() {
        return Ok(());
    }

    // The packet must name the dimension the player is in.
    let Some(dim) = host.player_dimension(player) else {
        return Ok(());
    };
    if dim != key {
        return Ok(());
    }
    if host.is_plot_world(&dim) {
        return Ok(());
    }
    if !host.can_modify_world(player, &dim) {
        return Ok(());
    }
    if !host.on_time_change(player, time, freeze) {
        return Ok(());
    }

    if let Some(time) = time {
        host.set_day_time(&dim, time);
    }
    if let Some(frozen) = freeze {
        host.set_time_frozen(&dim, frozen);
    }
    Ok(())
}

/// `set_gamemode`: one byte. Permission depends on the requested mode.
pub fn set_gamemode(host: &mut dyn WorldHost, player: u128, data: &[u8]) -> Result<(), WorldError> {
    let mode = GameMode::from_id(Reader::new(data).read_i8()?);
    if !can_use(host, player, mode.permission_node()) {
        return Ok(());
    }
    if !host.on_game_mode_change(player, mode) {
        return Ok(());
    }
    host.set_game_mode(player, mode);
    Ok(())
}

/// Clamps to [-1, 1]. Same as Java `Math.max(-1, Math.min(1, x))`: NaN passes through unchanged
/// (Java does the same; the glue may want to reject it).
pub fn clamp_fly_speed(speed: f32) -> f32 {
    let upper = if speed > 1.0 { 1.0 } else { speed };
    if upper < -1.0 {
        -1.0
    } else {
        upper
    }
}

/// `set_fly_speed`: one float, clamped to [-1, 1].
pub fn set_fly_speed(
    host: &mut dyn WorldHost,
    player: u128,
    data: &[u8],
) -> Result<(), WorldError> {
    if !can_use(host, player, PERM_PLAYER_SPEED) {
        return Ok(());
    }
    let speed = clamp_fly_speed(Reader::new(data).read_f32()?);
    if !host.on_fly_speed_change(player, speed) {
        return Ok(());
    }
    host.set_fly_speed(player, speed);
    Ok(())
}

/// `teleport`: dimension key, x, y, z (doubles), yaw and pitch (floats).
pub fn teleport(host: &mut dyn WorldHost, player: u128, data: &[u8]) -> Result<(), WorldError> {
    if !can_use(host, player, PERM_PLAYER_TELEPORT) {
        return Ok(());
    }

    let mut r = Reader::new(data);
    let key = read_identifier(&mut r)?;
    let at = Location {
        x: r.read_f64()?,
        y: r.read_f64()?,
        z: r.read_f64()?,
        yaw: r.read_f32()?,
        pitch: r.read_f32()?,
    };

    let allow_between = host.allow_teleport_between_worlds();
    let Some(current) = host.player_dimension(player) else {
        return Ok(());
    };
    if !allow_between && current != key {
        return Ok(());
    }

    if !host.on_unknown_teleport(player, &key, at) {
        return Ok(());
    }

    // The target world must be loaded.
    if !host.world_exists(&key) {
        return Ok(());
    }
    if !allow_between && current != key {
        return Ok(());
    }

    if !host.on_teleport(player, &key, at) {
        return Ok(());
    }
    host.teleport_player(player, &key, at);
    Ok(())
}

impl WorldState {
    /// `set_no_physical_trigger`: one bool, stored per player.
    pub fn on_set_no_physical_trigger(
        &mut self,
        host: &mut dyn WorldHost,
        player: u128,
        data: &[u8],
    ) -> Result<(), WorldError> {
        if !can_use(host, player, PERM_PLAYER_SET_NO_PHYSICAL_TRIGGER) {
            return Ok(());
        }
        let on = Reader::new(data).read_bool()?;
        self.set_no_physical_trigger(player, on);
        Ok(())
    }

    /// `set_world_property`: id, value type id, value bytes, update id. Always acknowledged with the update id
    /// (except when the permission check fails, as in Java). The value is applied only when the id exists and
    /// the type id matches the property's data type.
    pub fn on_set_world_property(
        &mut self,
        host: &mut dyn WorldHost,
        player: u128,
        data: &[u8],
    ) -> Result<(), WorldError> {
        if !can_use(host, player, PERM_WORLD_PROPERTY) {
            return Ok(());
        }

        let mut r = Reader::new(data);
        let id = read_identifier(&mut r)?;
        let type_id = r.read_varint()?;
        let value = read_byte_array(&mut r)?;
        let update_id = r.read_varint()?;

        let Some(dim) = host.player_dimension(player) else {
            return Ok(());
        };

        if !host.can_modify_world(player, &dim) || host.is_plot_world(&dim) {
            send_ack(host, player, update_id);
            return Ok(());
        }

        let Some(registry) = self.registry_or_create(host, &dim) else {
            send_ack(host, player, update_id);
            return Ok(());
        };

        if let Some(holder) = registry.get_mut(&id) {
            if holder.data_type_id() == type_id {
                holder.update(host, player, &dim, &value)?;
            }
        }

        send_ack(host, player, update_id);
        Ok(())
    }
}

fn send_ack(host: &mut dyn WorldHost, player: u128, update_id: i32) {
    let mut w = Writer::new();
    w.write_varint(update_id);
    host.send_payload(player, CH_ACK_WORLD_PROPERTIES, w.into_bytes());
}
