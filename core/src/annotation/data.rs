// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd):
//   annotations/data/AnnotationData.java, annotations/data/{Line,Text,Image,FreehandOutline,
//   LinesOutline,BoxOutline}AnnotationData.java, annotations/AnnotationUpdateAction.java
//
//! Annotation payloads and the `axiom:annotation_update` action list.

use crate::buf::{BufError, Reader, Writer, DEFAULT_STRING_MAX_CHARS};

const TYPE_LINE: u8 = 0;
const TYPE_TEXT: u8 = 1;
const TYPE_IMAGE: u8 = 2;
const TYPE_FREEHAND: u8 = 3;
const TYPE_LINES: u8 = 4;
const TYPE_BOX: u8 = 5;

const ACTION_CREATE: u8 = 0;
const ACTION_DELETE: u8 = 1;
const ACTION_MOVE: u8 = 2;
const ACTION_CLEAR_ALL: u8 = 3;
const ACTION_ROTATE: u8 = 4;

/// Java's record constructors force the alpha byte: `colour = 0xFF000000 | colour`.
/// Text and image annotations do not do this in Java, so only the other kinds call it.
fn opaque(colour: i32) -> i32 {
    ((colour as u32) | 0xFF00_0000) as i32
}

/// Mojang's `Direction.from3DDataValue`: the index wraps as `Math.abs(index % 6)`.
/// The stored value is the normalised 3D data value (0..=5), which is what Java writes back.
pub fn direction_from_raw(raw: i8) -> u8 {
    ((raw as i32) % 6).unsigned_abs() as u8
}

fn write_block_xyz(w: &mut Writer, v: [i32; 3]) {
    for c in v {
        w.write_varint(c);
    }
}

fn read_block_xyz(r: &mut Reader) -> Result<[i32; 3], BufError> {
    Ok([r.read_varint()?, r.read_varint()?, r.read_varint()?])
}

fn write_vec3(w: &mut Writer, v: [f32; 3]) {
    for c in v {
        w.write_f32(c);
    }
}

fn read_vec3(r: &mut Reader) -> Result<[f32; 3], BufError> {
    Ok([r.read_f32()?, r.read_f32()?, r.read_f32()?])
}

fn write_quat(w: &mut Writer, v: [f32; 4]) {
    for c in v {
        w.write_f32(c);
    }
}

fn read_quat(r: &mut Reader) -> Result<[f32; 4], BufError> {
    Ok([r.read_f32()?, r.read_f32()?, r.read_f32()?, r.read_f32()?])
}

/// One annotation. Colours are ARGB. Line, freehand, lines and box colours are forced opaque on read.
#[derive(Clone, Debug, PartialEq)]
pub enum AnnotationData {
    Line {
        start: [i32; 3],
        line_width: f32,
        colour: i32,
        offsets: Vec<u8>,
    },
    Text {
        text: String,
        position: [f32; 3],
        rotation: [f32; 4],
        /// 3D data value of the Mojang `Direction`, 0..=5.
        direction: u8,
        fallback_yaw: f32,
        scale: f32,
        billboard_mode: i8,
        colour: i32,
        shadow: bool,
    },
    Image {
        image_url: String,
        position: [f32; 3],
        rotation: [f32; 4],
        direction: u8,
        fallback_yaw: f32,
        width: f32,
        opacity: f32,
        billboard_mode: i8,
    },
    FreehandOutline {
        start: [i32; 3],
        offset_count: i32,
        colour: i32,
        offsets: Vec<u8>,
    },
    LinesOutline {
        positions: Vec<i64>,
        colour: i32,
    },
    BoxOutline {
        from: [i32; 3],
        to: [i32; 3],
        colour: i32,
    },
}

impl AnnotationData {
    /// `setPosition` in Java. Only text and image annotations move; the other kinds ignore it.
    pub fn set_position(&mut self, to: [f32; 3]) {
        match self {
            Self::Text { position, .. } | Self::Image { position, .. } => *position = to,
            _ => {}
        }
    }

    /// `setRotation` in Java. Only text and image annotations rotate.
    pub fn set_rotation(&mut self, to: [f32; 4]) {
        match self {
            Self::Text { rotation, .. } | Self::Image { rotation, .. } => *rotation = to,
            _ => {}
        }
    }

    pub fn write(&self, w: &mut Writer) {
        match self {
            Self::Line { start, line_width, colour, offsets } => {
                w.write_u8(TYPE_LINE);
                write_block_xyz(w, *start);
                w.write_f32(*line_width);
                w.write_i32(*colour);
                w.write_byte_array(offsets);
            }
            Self::Text {
                text,
                position,
                rotation,
                direction,
                fallback_yaw,
                scale,
                billboard_mode,
                colour,
                shadow,
            } => {
                w.write_u8(TYPE_TEXT);
                w.write_string(text);
                write_vec3(w, *position);
                write_quat(w, *rotation);
                w.write_u8(*direction);
                w.write_f32(*fallback_yaw);
                w.write_f32(*scale);
                w.write_i8(*billboard_mode);
                w.write_i32(*colour);
                w.write_bool(*shadow);
            }
            Self::Image {
                image_url,
                position,
                rotation,
                direction,
                fallback_yaw,
                width,
                opacity,
                billboard_mode,
            } => {
                w.write_u8(TYPE_IMAGE);
                w.write_string(image_url);
                write_vec3(w, *position);
                write_quat(w, *rotation);
                w.write_u8(*direction);
                w.write_f32(*fallback_yaw);
                w.write_f32(*width);
                w.write_f32(*opacity);
                w.write_i8(*billboard_mode);
            }
            Self::FreehandOutline { start, offset_count, colour, offsets } => {
                w.write_u8(TYPE_FREEHAND);
                write_block_xyz(w, *start);
                w.write_varint(*offset_count);
                w.write_i32(*colour);
                w.write_byte_array(offsets);
            }
            Self::LinesOutline { positions, colour } => {
                w.write_u8(TYPE_LINES);
                w.write_varint(positions.len() as i32);
                for p in positions {
                    w.write_i64(*p);
                }
                w.write_i32(*colour);
            }
            Self::BoxOutline { from, to, colour } => {
                w.write_u8(TYPE_BOX);
                write_block_xyz(w, *from);
                write_block_xyz(w, *to);
                w.write_i32(*colour);
            }
        }
    }

    /// Java `AnnotationData.read`. `Ok(None)` for an unknown type byte (Java returns null).
    pub fn read(r: &mut Reader) -> Result<Option<Self>, BufError> {
        let data = match r.read_u8()? {
            TYPE_LINE => {
                let start = read_block_xyz(r)?;
                let line_width = r.read_f32()?;
                let colour = opaque(r.read_i32()?);
                let offsets = r.read_byte_array(r.remaining())?;
                Self::Line { start, line_width, colour, offsets }
            }
            TYPE_TEXT => {
                let text = r.read_string(DEFAULT_STRING_MAX_CHARS)?;
                let position = read_vec3(r)?;
                let rotation = read_quat(r)?;
                let direction = direction_from_raw(r.read_i8()?);
                let fallback_yaw = r.read_f32()?;
                let scale = r.read_f32()?;
                let billboard_mode = r.read_i8()?;
                let colour = r.read_i32()?;
                let shadow = r.read_bool()?;
                Self::Text {
                    text,
                    position,
                    rotation,
                    direction,
                    fallback_yaw,
                    scale,
                    billboard_mode,
                    colour,
                    shadow,
                }
            }
            TYPE_IMAGE => {
                let image_url = r.read_string(DEFAULT_STRING_MAX_CHARS)?;
                let position = read_vec3(r)?;
                let rotation = read_quat(r)?;
                let direction = direction_from_raw(r.read_i8()?);
                let fallback_yaw = r.read_f32()?;
                let width = r.read_f32()?;
                let opacity = r.read_f32()?;
                let billboard_mode = r.read_i8()?;
                Self::Image {
                    image_url,
                    position,
                    rotation,
                    direction,
                    fallback_yaw,
                    width,
                    opacity,
                    billboard_mode,
                }
            }
            TYPE_FREEHAND => {
                let start = read_block_xyz(r)?;
                let offset_count = r.read_varint()?;
                let colour = opaque(r.read_i32()?);
                let offsets = r.read_byte_array(r.remaining())?;
                Self::FreehandOutline { start, offset_count, colour, offsets }
            }
            TYPE_LINES => {
                let count = r.read_varint()?;
                if count < 0 {
                    return Err(BufError::NegativeLength(count));
                }
                // Each position is 8 bytes. Refuse counts the buffer cannot hold before allocating.
                if count as usize > r.remaining() / 8 {
                    return Err(BufError::CollectionTooLarge { count: count as usize, limit: r.remaining() / 8 });
                }
                let mut positions = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    positions.push(r.read_i64()?);
                }
                let colour = opaque(r.read_i32()?);
                Self::LinesOutline { positions, colour }
            }
            TYPE_BOX => {
                let from = read_block_xyz(r)?;
                let to = read_block_xyz(r)?;
                let colour = opaque(r.read_i32()?);
                Self::BoxOutline { from, to, colour }
            }
            _ => return Ok(None),
        };
        Ok(Some(data))
    }
}

/// One entry of the `axiom:annotation_update` payload (`AnnotationUpdateAction` in Java).
#[derive(Clone, Debug, PartialEq)]
pub enum AnnotationUpdateAction {
    Create { uuid: u128, data: AnnotationData },
    Delete { uuid: u128 },
    Move { uuid: u128, to: [f32; 3] },
    ClearAll,
    Rotate { uuid: u128, to: [f32; 4] },
}

impl AnnotationUpdateAction {
    pub fn write(&self, w: &mut Writer) {
        match self {
            Self::Create { uuid, data } => {
                w.write_u8(ACTION_CREATE);
                w.write_uuid(*uuid);
                data.write(w);
            }
            Self::Delete { uuid } => {
                w.write_u8(ACTION_DELETE);
                w.write_uuid(*uuid);
            }
            Self::Move { uuid, to } => {
                w.write_u8(ACTION_MOVE);
                w.write_uuid(*uuid);
                write_vec3(w, *to);
            }
            Self::ClearAll => w.write_u8(ACTION_CLEAR_ALL),
            Self::Rotate { uuid, to } => {
                w.write_u8(ACTION_ROTATE);
                w.write_uuid(*uuid);
                write_quat(w, *to);
            }
        }
    }

    /// Java `AnnotationUpdateAction.read`. `Ok(None)` for an unknown type (Java returns null and
    /// the caller skips it). Like Java, nothing after the type byte is consumed in that case.
    pub fn read(r: &mut Reader) -> Result<Option<Self>, BufError> {
        let action = match r.read_u8()? {
            ACTION_CREATE => {
                let uuid = r.read_uuid()?;
                match AnnotationData::read(r)? {
                    Some(data) => Self::Create { uuid, data },
                    None => return Ok(None),
                }
            }
            ACTION_DELETE => Self::Delete { uuid: r.read_uuid()? },
            ACTION_MOVE => {
                let uuid = r.read_uuid()?;
                Self::Move { uuid, to: read_vec3(r)? }
            }
            ACTION_CLEAR_ALL => Self::ClearAll,
            ACTION_ROTATE => {
                let uuid = r.read_uuid()?;
                Self::Rotate { uuid, to: read_quat(r)? }
            }
            _ => return Ok(None),
        };
        Ok(Some(action))
    }
}

/// `NetworkHelper.writeCollection`: varint count, then each action.
pub fn write_action_list(actions: &[AnnotationUpdateAction]) -> Vec<u8> {
    let mut w = Writer::new();
    w.write_varint(actions.len() as i32);
    for action in actions {
        action.write(&mut w);
    }
    w.into_bytes()
}

/// Reads the list the way `UpdateAnnotationPacketListener` does: unknown entries are skipped.
pub fn read_action_list(r: &mut Reader) -> Result<Vec<AnnotationUpdateAction>, BufError> {
    let count = r.read_varint()?;
    if count < 0 {
        return Err(BufError::NegativeLength(count));
    }
    let mut out = Vec::with_capacity((count as usize).min(256));
    for _ in 0..count {
        if let Some(action) = AnnotationUpdateAction::read(r)? {
            out.push(action);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(data: &AnnotationData) -> AnnotationData {
        let mut w = Writer::new();
        data.write(&mut w);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        let back = AnnotationData::read(&mut r).unwrap().unwrap();
        assert_eq!(r.remaining(), 0, "annotation must consume its whole encoding");
        back
    }

    fn sample_text() -> AnnotationData {
        AnnotationData::Text {
            text: "hi \u{00e4}".into(),
            position: [1.5, -2.0, 3.25],
            rotation: [0.0, 0.0, 0.0, 1.0],
            direction: 2,
            fallback_yaw: 90.0,
            scale: 0.5,
            billboard_mode: -1,
            colour: 0x1234_5678,
            shadow: true,
        }
    }

    #[test]
    fn every_kind_roundtrips() {
        let samples = vec![
            AnnotationData::Line { start: [1, -70, 300], line_width: 2.5, colour: 0xFF00_FF00u32 as i32, offsets: vec![1, 2, 3] },
            sample_text(),
            AnnotationData::Image {
                image_url: "https://example.invalid/a.png".into(),
                position: [0.0, 64.0, 0.0],
                rotation: [0.0, 0.7, 0.0, 0.7],
                direction: 5,
                fallback_yaw: 0.0,
                width: 4.0,
                opacity: 0.5,
                billboard_mode: 2,
            },
            AnnotationData::FreehandOutline { start: [0, 0, 0], offset_count: 3, colour: 0xFF11_2233u32 as i32, offsets: vec![9; 6] },
            AnnotationData::LinesOutline { positions: vec![-1, 0, i64::MAX], colour: 0xFF44_5566u32 as i32 },
            AnnotationData::BoxOutline { from: [-5, 0, -5], to: [5, 10, 5], colour: 0xFF77_8899u32 as i32 },
        ];
        for s in &samples {
            assert_eq!(&roundtrip(s), s);
        }
    }

    #[test]
    fn non_text_colours_are_forced_opaque_on_read() {
        let box_data = AnnotationData::BoxOutline { from: [0; 3], to: [1; 3], colour: 0x0011_2233 };
        match roundtrip(&box_data) {
            AnnotationData::BoxOutline { colour, .. } => assert_eq!(colour as u32, 0xFF11_2233),
            other => panic!("unexpected {other:?}"),
        }
        // Text keeps its alpha byte as sent, like Java's TextAnnotationData.
        let mut text = sample_text();
        if let AnnotationData::Text { colour, .. } = &mut text {
            *colour = 0x00AB_CDEF;
        }
        match roundtrip(&text) {
            AnnotationData::Text { colour, .. } => assert_eq!(colour as u32, 0x00AB_CDEF),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn direction_wraps_like_mojang() {
        assert_eq!(direction_from_raw(0), 0);
        assert_eq!(direction_from_raw(5), 5);
        assert_eq!(direction_from_raw(6), 0);
        assert_eq!(direction_from_raw(-1), 1);
        assert_eq!(direction_from_raw(-7), 1);
    }

    #[test]
    fn move_and_rotate_only_touch_text_and_image() {
        let mut text = sample_text();
        text.set_position([9.0, 9.0, 9.0]);
        text.set_rotation([0.0, 1.0, 0.0, 0.0]);
        match &text {
            AnnotationData::Text { position, rotation, .. } => {
                assert_eq!(*position, [9.0, 9.0, 9.0]);
                assert_eq!(*rotation, [0.0, 1.0, 0.0, 0.0]);
            }
            _ => unreachable!(),
        }
        let mut boxed = AnnotationData::BoxOutline { from: [0; 3], to: [1; 3], colour: 0xFF00_0000u32 as i32 };
        let before = boxed.clone();
        boxed.set_position([5.0; 3]);
        boxed.set_rotation([1.0; 4]);
        assert_eq!(boxed, before);
    }

    #[test]
    fn unknown_annotation_type_is_none() {
        let mut r = Reader::new(&[42u8, 0, 0]);
        assert!(AnnotationData::read(&mut r).unwrap().is_none());
    }

    #[test]
    fn negative_or_oversized_lines_count_is_rejected() {
        let mut w = Writer::new();
        w.write_u8(TYPE_LINES);
        w.write_varint(-1);
        let bytes = w.into_bytes();
        assert!(AnnotationData::read(&mut Reader::new(&bytes)).is_err());

        let mut w = Writer::new();
        w.write_u8(TYPE_LINES);
        w.write_varint(1000);
        let bytes = w.into_bytes();
        assert!(AnnotationData::read(&mut Reader::new(&bytes)).is_err());
    }

    #[test]
    fn truncated_annotation_is_an_error() {
        let mut w = Writer::new();
        sample_text().write(&mut w);
        let bytes = w.into_bytes();
        assert!(AnnotationData::read(&mut Reader::new(&bytes[..bytes.len() - 1])).is_err());
    }

    #[test]
    fn action_list_roundtrips_and_skips_unknown_entries() {
        let uuid = 0x0123_4567_89ab_cdef_0011_2233_4455_6677u128;
        let actions = vec![
            AnnotationUpdateAction::Create {
                uuid,
                data: AnnotationData::BoxOutline { from: [0; 3], to: [2; 3], colour: 0xFF00_0001u32 as i32 },
            },
            AnnotationUpdateAction::Move { uuid, to: [1.0, 2.0, 3.0] },
            AnnotationUpdateAction::Rotate { uuid, to: [0.0, 0.0, 0.0, 1.0] },
            AnnotationUpdateAction::Delete { uuid },
            AnnotationUpdateAction::ClearAll,
        ];
        let bytes = write_action_list(&actions);
        assert_eq!(read_action_list(&mut Reader::new(&bytes)).unwrap(), actions);

        // Unknown action type 9 (no payload) is skipped, the following ClearAll is still read.
        let mut w = Writer::new();
        w.write_varint(2);
        w.write_u8(9);
        AnnotationUpdateAction::ClearAll.write(&mut w);
        let bytes = w.into_bytes();
        assert_eq!(read_action_list(&mut Reader::new(&bytes)).unwrap(), vec![AnnotationUpdateAction::ClearAll]);
    }

    #[test]
    fn negative_action_count_is_rejected() {
        let mut w = Writer::new();
        w.write_varint(-3);
        let bytes = w.into_bytes();
        assert!(read_action_list(&mut Reader::new(&bytes)).is_err());
    }
}
