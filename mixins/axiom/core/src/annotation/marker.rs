// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): marker/MarkerData.java
//
//! Marker snapshot used by `axiom:marker_data`, parsed from the marker's custom data (NBT).

use crate::buf::{BufError, Nbt, Reader, Writer};

#[derive(Clone, Debug, PartialEq)]
pub struct MarkerData {
    pub uuid: u128,
    pub position: [f64; 3],
    pub name: Option<String>,
    pub min_region: Option<[f64; 3]>,
    pub max_region: Option<[f64; 3]>,
    pub line_argb: i32,
    pub line_thickness: f32,
    pub face_argb: i32,
}

fn read_vec3d(r: &mut Reader) -> Result<[f64; 3], BufError> {
    Ok([r.read_f64()?, r.read_f64()?, r.read_f64()?])
}

fn write_vec3d(w: &mut Writer, v: [f64; 3]) {
    for c in v {
        w.write_f64(c);
    }
}

/// Java `String.trim()`: strips every char up to and including ' '.
fn java_trim(s: &str) -> &str {
    s.trim_matches(|c: char| c <= ' ')
}

/// Java `Double.parseDouble`, which also accepts a trailing d/D/f/F. Input is trimmed by the caller.
fn parse_java_double(s: &str) -> Option<f64> {
    let s = match s.as_bytes().last() {
        Some(b'd' | b'D' | b'f' | b'F') => &s[..s.len() - 1],
        _ => s,
    };
    s.parse::<f64>().ok()
}

/// Java `calculateCoordinate`: a leading `~` is relative to the marker. Invalid input gives NaN.
fn calculate_coordinate(text: &str, position: f64) -> f64 {
    let trimmed = java_trim(text);
    let (relative, body) = match trimmed.strip_prefix('~') {
        Some(rest) => (true, java_trim(rest)),
        None => (false, trimmed),
    };
    match parse_java_double(body) {
        Some(value) if relative => position + value,
        Some(value) => value,
        None => f64::NAN,
    }
}

/// Custom data lookups with the Java semantics.
/// The list getters return nothing when the element type differs (`VersionHelper.getList`).
/// `get_int` accepts any numeric tag and truncates it (`getIntOr`).
fn get_string(data: &Nbt, key: &str) -> Option<String> {
    data.get(key).and_then(Nbt::as_str).map(str::to_owned)
}

fn get_int(data: &Nbt, key: &str) -> Option<i32> {
    match data.get(key)? {
        Nbt::Byte(v) => Some(*v as i32),
        Nbt::Short(v) => Some(*v as i32),
        Nbt::Int(v) => Some(*v),
        Nbt::Long(v) => Some(*v as i32),
        Nbt::Float(v) => Some(*v as i32),
        Nbt::Double(v) => Some(*v as i32),
        _ => None,
    }
}

fn get_doubles(data: &Nbt, key: &str) -> Vec<f64> {
    match data.get(key).and_then(Nbt::as_list) {
        Some(items) => items
            .iter()
            .map(|item| match item {
                Nbt::Double(v) => Some(*v),
                _ => None,
            })
            .collect::<Option<Vec<f64>>>()
            .unwrap_or_default(),
        None => Vec::new(),
    }
}

fn get_strings(data: &Nbt, key: &str) -> Vec<String> {
    match data.get(key).and_then(Nbt::as_list) {
        Some(items) => items
            .iter()
            .map(|item| item.as_str().map(str::to_owned))
            .collect::<Option<Vec<String>>>()
            .unwrap_or_default(),
        None => Vec::new(),
    }
}

/// One region corner: a list of three doubles, or else three strings (`~` relative).
fn read_region(data: &Nbt, key: &str, position: [f64; 3]) -> Option<[f64; 3]> {
    let numeric = get_doubles(data, key);
    if numeric.len() == 3 {
        return Some([numeric[0], numeric[1], numeric[2]]);
    }
    let text = get_strings(data, key);
    if text.len() == 3 {
        let coords = [
            calculate_coordinate(&text[0], position[0]),
            calculate_coordinate(&text[1], position[1]),
            calculate_coordinate(&text[2], position[2]),
        ];
        if coords.iter().all(|c| c.is_finite()) {
            return Some(coords);
        }
    }
    None
}

impl MarkerData {
    /// Java `MarkerData.read`.
    pub fn read(r: &mut Reader) -> Result<Self, BufError> {
        let uuid = r.read_uuid()?;
        let position = read_vec3d(r)?;
        let name = if r.read_bool()? { Some(r.read_string(crate::buf::DEFAULT_STRING_MAX_CHARS)?) } else { None };

        let mut min_region = None;
        let mut max_region = None;
        let mut line_argb = 0;
        let mut line_thickness = 0.0;
        let mut face_argb = 0;

        let flags = r.read_u8()?;
        if flags != 0 {
            min_region = Some(read_vec3d(r)?);
            max_region = Some(read_vec3d(r)?);
            if flags & 2 != 0 {
                line_argb = r.read_i32()?;
            }
            if flags & 4 != 0 {
                line_thickness = r.read_f32()?;
            }
            if flags & 8 != 0 {
                face_argb = r.read_i32()?;
            }
        }

        Ok(Self { uuid, position, name, min_region, max_region, line_argb, line_thickness, face_argb })
    }

    /// Java `MarkerData.write`.
    pub fn write(&self, w: &mut Writer) {
        w.write_uuid(self.uuid);
        write_vec3d(w, self.position);
        match &self.name {
            Some(name) => {
                w.write_bool(true);
                w.write_string(name);
            }
            None => w.write_bool(false),
        }

        match (self.min_region, self.max_region) {
            (Some(min), Some(max)) => {
                let mut flags = 1u8;
                if self.line_argb != 0 {
                    flags |= 2;
                }
                if self.line_thickness != 0.0 {
                    flags |= 4;
                }
                if self.face_argb != 0 {
                    flags |= 8;
                }
                w.write_u8(flags);

                write_vec3d(w, min);
                write_vec3d(w, max);
                if self.line_argb != 0 {
                    w.write_i32(self.line_argb);
                }
                if self.line_thickness != 0.0 {
                    w.write_f32(self.line_thickness);
                }
                if self.face_argb != 0 {
                    w.write_i32(self.face_argb);
                }
            }
            _ => w.write_u8(0),
        }
    }

    /// Java `MarkerData.createFrom(Marker)`. The uuid, position and custom data come from the host.
    pub fn create_from(uuid: u128, position: [f64; 3], data: &Nbt) -> Self {
        let raw_name = get_string(data, "name").unwrap_or_default();
        let trimmed = java_trim(&raw_name);
        let name = if trimmed.is_empty() { None } else { Some(trimmed.to_owned()) };

        let mut min_region = None;
        let mut max_region = None;
        let mut line_argb = 0;
        let mut line_thickness = 0.0;
        let mut face_argb = 0;

        if data.get("min").is_some() && data.get("max").is_some() {
            min_region = read_region(data, "min", position);
            max_region = read_region(data, "max", position);

            if min_region.is_some() && max_region.is_some() {
                line_argb = get_int(data, "line_argb").unwrap_or(0);
                line_thickness = get_int(data, "line_thickness").unwrap_or(0) as f32;
                face_argb = get_int(data, "face_argb").unwrap_or(0);
            }
        }

        Self { uuid, position, name, min_region, max_region, line_argb, line_thickness, face_argb }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buf::NbtCompound;

    fn tag(entries: Vec<(&str, Nbt)>) -> Nbt {
        Nbt::Compound(entries.into_iter().map(|(k, v)| (k.to_owned(), v)).collect::<NbtCompound>())
    }

    fn strings(items: &[&str]) -> Nbt {
        Nbt::List(items.iter().map(|s| Nbt::String((*s).to_owned())).collect())
    }

    fn doubles(items: &[f64]) -> Nbt {
        Nbt::List(items.iter().map(|d| Nbt::Double(*d)).collect())
    }

    fn roundtrip(m: &MarkerData) -> MarkerData {
        let mut w = Writer::new();
        m.write(&mut w);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        let back = MarkerData::read(&mut r).unwrap();
        assert_eq!(r.remaining(), 0);
        back
    }

    #[test]
    fn plain_marker_roundtrips_with_flag_zero() {
        let m = MarkerData {
            uuid: 42,
            position: [1.0, 2.0, 3.0],
            name: Some("spawn".into()),
            min_region: None,
            max_region: None,
            line_argb: 0,
            line_thickness: 0.0,
            face_argb: 0,
        };
        let mut w = Writer::new();
        m.write(&mut w);
        let bytes = w.into_bytes();
        // uuid(16) + 3 doubles(24) + bool(1) + length(1) + "spawn"(5) + flags(1)
        assert_eq!(bytes.len(), 16 + 24 + 1 + 1 + 5 + 1);
        assert_eq!(*bytes.last().unwrap(), 0);
        assert_eq!(roundtrip(&m), m);
    }

    #[test]
    fn region_marker_roundtrips_with_all_optional_fields() {
        let full = MarkerData {
            uuid: 7,
            position: [0.5, 64.0, -9.0],
            name: None,
            min_region: Some([0.0, 60.0, -10.0]),
            max_region: Some([10.0, 70.0, 0.0]),
            line_argb: 0xFF00_FF00u32 as i32,
            line_thickness: 1.5,
            face_argb: 0x40FF_0000,
        };
        assert_eq!(roundtrip(&full), full);

        let only_face = MarkerData { line_argb: 0, line_thickness: 0.0, ..full.clone() };
        let mut w = Writer::new();
        only_face.write(&mut w);
        let bytes = w.into_bytes();
        // uuid, position, the absent name (one false byte), then flags: bit 1 (regions) and bit 8 (face).
        assert_eq!(bytes[16 + 24 + 1], 1 | 8);
        assert_eq!(roundtrip(&only_face), only_face);
    }

    #[test]
    fn create_from_reads_absolute_and_relative_regions() {
        let data = tag(vec![
            ("name", Nbt::String("  arena \u{1}".into())),
            ("min", strings(&["~-2", "60", " ~ 1d "])),
            ("max", doubles(&[5.0, 70.0, 5.0])),
            ("line_argb", Nbt::Int(0x1122_3344)),
            ("line_thickness", Nbt::Int(3)),
            ("face_argb", Nbt::Int(0x5566_7788)),
        ]);
        let m = MarkerData::create_from(9, [10.0, 64.0, 20.0], &data);
        assert_eq!(m.name.as_deref(), Some("arena"));
        assert_eq!(m.min_region, Some([8.0, 60.0, 21.0]));
        assert_eq!(m.max_region, Some([5.0, 70.0, 5.0]));
        assert_eq!(m.line_argb, 0x1122_3344);
        assert_eq!(m.line_thickness, 3.0);
        assert_eq!(m.face_argb, 0x5566_7788);
    }

    #[test]
    fn numeric_style_tags_are_truncated_like_get_int_or() {
        let data = tag(vec![
            ("min", doubles(&[0.0, 0.0, 0.0])),
            ("max", doubles(&[1.0, 1.0, 1.0])),
            ("line_argb", Nbt::Long(0x1_0000_0005)),
            ("line_thickness", Nbt::Float(2.9)),
            ("face_argb", Nbt::String("nope".into())),
        ]);
        let m = MarkerData::create_from(1, [0.0; 3], &data);
        assert_eq!(m.line_argb, 5);
        assert_eq!(m.line_thickness, 2.0);
        assert_eq!(m.face_argb, 0);
    }

    #[test]
    fn create_from_drops_region_on_bad_string_and_blank_name() {
        let data = tag(vec![
            ("name", Nbt::String("   ".into())),
            ("min", strings(&["0", "nope", "0"])),
            ("max", doubles(&[1.0, 1.0, 1.0])),
            ("line_argb", Nbt::Int(5)),
        ]);
        let m = MarkerData::create_from(1, [0.0; 3], &data);
        assert_eq!(m.name, None);
        assert_eq!(m.min_region, None);
        assert_eq!(m.max_region, Some([1.0, 1.0, 1.0]));
        // Without both regions the style fields stay at their defaults, as in Java.
        assert_eq!(m.line_argb, 0);
    }

    #[test]
    fn create_from_requires_both_keys_and_three_entries() {
        let only_min = tag(vec![("min", doubles(&[0.0, 0.0, 0.0]))]);
        assert_eq!(MarkerData::create_from(1, [0.0; 3], &only_min).min_region, None);

        let short = tag(vec![("min", doubles(&[0.0, 0.0])), ("max", doubles(&[1.0, 1.0, 1.0]))]);
        assert_eq!(MarkerData::create_from(1, [0.0; 3], &short).min_region, None);

        // A list of ints is not a list of doubles, so Java sees no region either.
        let ints = tag(vec![
            ("min", Nbt::List(vec![Nbt::Int(0), Nbt::Int(0), Nbt::Int(0)])),
            ("max", doubles(&[1.0, 1.0, 1.0])),
        ]);
        assert_eq!(MarkerData::create_from(1, [0.0; 3], &ints).min_region, None);
    }

    #[test]
    fn coordinate_parsing_matches_java_rules() {
        assert_eq!(calculate_coordinate("  ~ 2.5  ", 1.0), 3.5);
        assert!(calculate_coordinate("~", 4.0).is_nan());
        assert!(calculate_coordinate("", 0.0).is_nan());
        assert_eq!(calculate_coordinate("3f", 0.0), 3.0);
        assert!(calculate_coordinate("NaN", 0.0).is_nan());
        assert!(calculate_coordinate("1,5", 0.0).is_nan());
    }
}
