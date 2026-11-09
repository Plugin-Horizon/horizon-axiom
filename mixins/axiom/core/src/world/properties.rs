// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd):
// world_properties/{WorldPropertyDataType, WorldPropertyWidgetType, WorldPropertyCategory,
// PropertyUpdateHandler}.java, world_properties/server/{ServerWorldPropertiesRegistry, ServerWorldPropertyBase,
// ServerWorldProperty, ServerWorldPropertyHolder, PropertyUpdateResult}.java.
//
// Not ported: WorldPropertyDataType.ITEM and BLOCK (type ids 3 and 4). No widget produces them in Axiom, so no
// property can ever carry them.

use std::collections::{BTreeMap, BTreeSet};

use crate::buf::{Reader, Writer};

use super::{parse_identifier, write_byte_array, WorldError, WorldHost, CH_SET_WORLD_PROPERTY};

/// Data type of a property value (`WorldPropertyDataType`). The id is the wire type id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataType {
    Boolean,
    Integer,
    Text,
    Empty,
}

impl DataType {
    pub fn type_id(self) -> i32 {
        match self {
            DataType::Boolean => 0,
            DataType::Integer => 1,
            DataType::Text => 2,
            DataType::Empty => 5,
        }
    }

    /// Value bytes as sent on the wire. A value of the wrong shape encodes like Java's null (false, 0, empty).
    pub(crate) fn serialize(self, value: &PropValue) -> Vec<u8> {
        match (self, value) {
            (DataType::Boolean, PropValue::Bool(b)) => vec![u8::from(*b)],
            (DataType::Boolean, _) => vec![0],
            (DataType::Integer, PropValue::Int(i)) => {
                let mut w = Writer::new();
                w.write_varint(*i);
                w.into_bytes()
            }
            (DataType::Integer, _) => vec![0],
            (DataType::Text, PropValue::Text(s)) => s.as_bytes().to_vec(),
            (DataType::Text, _) => Vec::new(),
            (DataType::Empty, _) => Vec::new(),
        }
    }

    fn deserialize(self, bytes: &[u8]) -> Result<PropValue, WorldError> {
        Ok(match self {
            DataType::Boolean => {
                PropValue::Bool(*bytes.first().ok_or(WorldError::InvalidValue)? != 0)
            }
            DataType::Integer => PropValue::Int(Reader::new(bytes).read_varint()?),
            DataType::Text => PropValue::Text(String::from_utf8_lossy(bytes).into_owned()),
            DataType::Empty => PropValue::Unit,
        })
    }
}

/// A property value. The variant always matches the property's `DataType`.
#[derive(Clone, Debug, PartialEq)]
pub enum PropValue {
    Bool(bool),
    Int(i32),
    Text(String),
    Unit,
}

/// Widget shown in the editor (`WorldPropertyWidgetType`). Writes its wire kind id and extra fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Widget {
    Checkbox,
    Slider { min: i32, max: i32 },
    Textbox,
    Time,
    Button,
    ButtonArray { other_buttons: Vec<String> },
}

impl Widget {
    pub fn data_type(&self) -> DataType {
        match self {
            Widget::Checkbox => DataType::Boolean,
            Widget::Slider { .. } | Widget::Time | Widget::ButtonArray { .. } => DataType::Integer,
            Widget::Textbox => DataType::Text,
            Widget::Button => DataType::Empty,
        }
    }

    pub(crate) fn write(&self, w: &mut Writer) {
        match self {
            Widget::Checkbox => w.write_varint(0),
            Widget::Slider { min, max } => {
                w.write_varint(1);
                w.write_i32(*min);
                w.write_i32(*max);
            }
            Widget::Textbox => w.write_varint(2),
            Widget::Time => w.write_varint(3),
            Widget::Button => w.write_varint(4),
            Widget::ButtonArray { other_buttons } => {
                w.write_varint(5);
                w.write_varint(other_buttons.len() as i32);
                for label in other_buttons {
                    w.write_string(label);
                }
            }
        }
    }
}

/// Editor category (`WorldPropertyCategory`). `name` is a translation key when `localize_name` is set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Category {
    pub name: String,
    pub localize_name: bool,
}

impl Category {
    fn write(&self, w: &mut Writer) {
        w.write_string(&self.name);
        w.write_bool(self.localize_name);
    }
}

/// What the handler tells the holder to do with a client update (`PropertyUpdateResult`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropertyUpdateResult {
    UpdateAndSync,
    UpdateWithoutSync,
    Cancel,
}

impl PropertyUpdateResult {
    pub fn is_update(self) -> bool {
        !matches!(self, PropertyUpdateResult::Cancel)
    }

    pub fn is_sync(self) -> bool {
        matches!(self, PropertyUpdateResult::UpdateAndSync)
    }
}

type DefaultFn = Box<dyn Fn(&dyn WorldHost, &str) -> PropValue>;
type UpdateFn = Box<dyn Fn(&mut dyn WorldHost, u128, &str, &PropValue) -> PropertyUpdateResult>;

/// A world property definition (`ServerWorldProperty`): id, label, widget, default value and update handler.
pub struct WorldProperty {
    id: String,
    name: String,
    localize_name: bool,
    widget: Widget,
    default_value: DefaultFn,
    update: UpdateFn,
}

impl WorldProperty {
    /// `id` is `namespace:path` (or a bare path in `minecraft`). The default value is computed when the
    /// property is added to a registry; the update handler runs on each client change.
    pub fn new(
        id: &str,
        name: &str,
        localize_name: bool,
        widget: Widget,
        default_value: impl Fn(&dyn WorldHost, &str) -> PropValue + 'static,
        update: impl Fn(&mut dyn WorldHost, u128, &str, &PropValue) -> PropertyUpdateResult + 'static,
    ) -> Result<Self, WorldError> {
        Ok(Self {
            id: parse_identifier(id)?,
            name: name.to_string(),
            localize_name,
            widget,
            default_value: Box::new(default_value),
            update: Box::new(update),
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

/// A property inside a registry with its current value (`ServerWorldPropertyHolder`).
pub struct Holder {
    property: WorldProperty,
    value: PropValue,
    /// Value changed without a sync (`UPDATE_WITHOUT_SYNC`); the next setValue syncs it.
    unsynced: bool,
}

impl Holder {
    pub fn id(&self) -> &str {
        &self.property.id
    }

    pub fn value(&self) -> &PropValue {
        &self.value
    }

    fn data_type(&self) -> DataType {
        self.property.widget.data_type()
    }

    /// Wire type id of the value (what a client must send with an update).
    pub(crate) fn data_type_id(&self) -> i32 {
        self.data_type().type_id()
    }

    /// `ServerWorldPropertyHolder.update`: a client sent a new value. Malformed bytes are an error.
    pub(crate) fn update(
        &mut self,
        host: &mut dyn WorldHost,
        player: u128,
        dim: &str,
        data: &[u8],
    ) -> Result<(), WorldError> {
        let new_value = self.data_type().deserialize(data)?;
        let result = (self.property.update)(host, player, dim, &new_value);
        if result.is_update() {
            self.value = new_value;
            if result.is_sync() {
                self.sync(host, dim);
            } else {
                self.unsynced = true;
            }
        }
        Ok(())
    }

    /// `ServerWorldPropertyHolder.setValue`: syncs when the value changed or an earlier change was unsynced.
    pub(crate) fn set_value(&mut self, host: &mut dyn WorldHost, dim: &str, value: PropValue) {
        let sync = self.unsynced || value != self.value;
        self.value = value;
        if sync {
            self.sync(host, dim);
        }
    }

    /// Sends `axiom:set_world_property` to every Axiom-active player in the dimension.
    fn sync(&mut self, host: &mut dyn WorldHost, dim: &str) {
        let mut w = Writer::new();
        w.write_identifier(&self.property.id);
        w.write_varint(self.data_type().type_id());
        write_byte_array(&mut w, &self.data_type().serialize(&self.value));
        let data = w.into_bytes();

        let candidates = host.players_in_dimension(dim);
        let targets: Vec<u128> = candidates
            .into_iter()
            .filter(|p| host.is_axiom_active(*p))
            .collect();
        for player in targets {
            host.send_payload(player, CH_SET_WORLD_PROPERTY, data.clone());
        }
        self.unsynced = false;
    }

    /// One entry of the register payload.
    fn write(&self, w: &mut Writer) {
        w.write_identifier(&self.property.id);
        w.write_string(&self.property.name);
        w.write_bool(self.property.localize_name);
        self.property.widget.write(w);
        write_byte_array(w, &self.data_type().serialize(&self.value));
    }
}

struct CategoryBlock {
    category: Category,
    holders: Vec<Holder>,
}

/// The world properties of one world (`ServerWorldPropertiesRegistry`). Categories keep insertion order.
pub struct WorldPropertiesRegistry {
    categories: Vec<CategoryBlock>,
    /// id -> (category index, holder index)
    index: BTreeMap<String, (usize, usize)>,
}

const CATEGORY_TIME: &str = "axiom.editorui.window.world_properties.time";
const CATEGORY_WEATHER: &str = "axiom.editorui.window.world_properties.weather";

impl WorldPropertiesRegistry {
    /// Creates a registry with the built-in time and weather properties (`registerDefault`).
    pub fn new(host: &dyn WorldHost, dim: &str) -> Self {
        let mut registry = Self {
            categories: Vec::new(),
            index: BTreeMap::new(),
        };
        registry.push_category(
            host,
            dim,
            Category {
                name: CATEGORY_TIME.to_string(),
                localize_name: true,
            },
            vec![time_property()],
        );
        registry.push_category(
            host,
            dim,
            Category {
                name: CATEGORY_WEATHER.to_string(),
                localize_name: true,
            },
            vec![pause_weather_property(), weather_type_property()],
        );
        registry
    }

    /// `AxiomCreateWorldPropertiesEvent.addCategory`. Duplicate ids are rejected before anything is added
    /// (Java throws midway and leaves a partial registry).
    pub fn add_category(
        &mut self,
        host: &dyn WorldHost,
        dim: &str,
        category: Category,
        properties: Vec<WorldProperty>,
    ) -> Result<(), WorldError> {
        let mut seen = BTreeSet::new();
        for property in &properties {
            if self.index.contains_key(&property.id) || !seen.insert(property.id.as_str()) {
                return Err(WorldError::DuplicateProperty);
            }
        }
        self.push_category(host, dim, category, properties);
        Ok(())
    }

    fn push_category(
        &mut self,
        host: &dyn WorldHost,
        dim: &str,
        category: Category,
        properties: Vec<WorldProperty>,
    ) {
        let category_index = self.categories.len();
        let mut holders = Vec::with_capacity(properties.len());
        for (holder_index, property) in properties.into_iter().enumerate() {
            let value = (property.default_value)(host, dim);
            self.index
                .insert(property.id.clone(), (category_index, holder_index));
            holders.push(Holder {
                property,
                value,
                unsynced: false,
            });
        }
        self.categories.push(CategoryBlock { category, holders });
    }

    pub fn get(&self, id: &str) -> Option<&Holder> {
        let &(c, h) = self.index.get(id)?;
        self.categories.get(c)?.holders.get(h)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Holder> {
        let &(c, h) = self.index.get(id)?;
        self.categories.get_mut(c)?.holders.get_mut(h)
    }

    /// Body of `axiom:register_world_properties`: category count, then per category its name and holders.
    pub fn encode_registration(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_varint(self.categories.len() as i32);
        for block in &self.categories {
            block.category.write(&mut w);
            w.write_varint(block.holders.len() as i32);
            for holder in &block.holders {
                holder.write(&mut w);
            }
        }
        w.into_bytes()
    }
}

/// `ServerWorldPropertiesRegistry.TIME`: the value is only a marker; the time itself is set by the time packet.
fn time_property() -> WorldProperty {
    builtin(
        "axiom:time",
        "axiom.editorui.window.world_properties.time",
        true,
        Widget::Time,
        |_host, _dim| PropValue::Int(0),
        |_host, _player, _dim, _value| PropertyUpdateResult::UpdateWithoutSync,
    )
}

/// `ServerWorldPropertiesRegistry.PAUSE_WEATHER`: checkbox over gamerule `advance_weather`.
fn pause_weather_property() -> WorldProperty {
    builtin(
        "axiom:pause_weather",
        "axiom.editorui.window.world_properties.pause_weather",
        true,
        Widget::Checkbox,
        |host, dim| PropValue::Bool(host.weather_paused(dim)),
        |host, _player, dim, value| match value {
            PropValue::Bool(paused) => {
                host.set_weather_paused(dim, *paused);
                PropertyUpdateResult::UpdateWithoutSync
            }
            _ => PropertyUpdateResult::Cancel,
        },
    )
}

/// `ServerWorldPropertiesRegistry.WEATHER_TYPE`: index 0 clears, 1 starts rain, 2 starts a thunderstorm.
/// Durations are sampled with the same uniform ranges as vanilla `ServerLevel` (assumed, verify for 26.3).
fn weather_type_property() -> WorldProperty {
    builtin(
        "axiom:weather_type",
        "axiom.editorui.window.world_properties.clear_weather",
        true,
        Widget::ButtonArray {
            other_buttons: vec![
                "axiom.editorui.window.world_properties.rain_weather".to_string(),
                "axiom.editorui.window.world_properties.thunder_weather".to_string(),
            ],
        },
        |_host, _dim| PropValue::Int(0),
        |host, _player, dim, value| {
            let PropValue::Int(index) = value else {
                return PropertyUpdateResult::Cancel;
            };
            match *index {
                0 => {
                    let clear_time = host.random_int(12_000, 180_000);
                    host.set_weather(dim, clear_time, 0, false, false);
                }
                1 => {
                    let rain_time = host.random_int(12_000, 24_000);
                    host.set_weather(dim, 0, rain_time, true, false);
                }
                2 => {
                    let rain_time = host.random_int(3_600, 15_600);
                    host.set_weather(dim, 0, rain_time, true, true);
                }
                _ => {}
            }
            PropertyUpdateResult::UpdateWithoutSync
        },
    )
}

/// Built-in properties have literal, valid ids, so no validation is needed here.
fn builtin(
    id: &str,
    name: &str,
    localize_name: bool,
    widget: Widget,
    default_value: impl Fn(&dyn WorldHost, &str) -> PropValue + 'static,
    update: impl Fn(&mut dyn WorldHost, u128, &str, &PropValue) -> PropertyUpdateResult + 'static,
) -> WorldProperty {
    WorldProperty {
        id: id.to_string(),
        name: name.to_string(),
        localize_name,
        widget,
        default_value: Box::new(default_value),
        update: Box::new(update),
    }
}
