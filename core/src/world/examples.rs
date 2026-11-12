// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): WorldPropertiesExample.java.
//
// Sample properties for a "how to add world properties" reference. As in Java, nothing registers these by
// default (the listener is commented out in AxiomPaper.onEnable). The glue can add them from
// `on_create_world_properties`.

use super::properties::{Category, PropValue, PropertyUpdateResult, Widget, WorldProperty};
use super::WorldError;

/// The "Examples" category with checkbox, slider, textbox and button. Each handler echoes its value to the
/// dimension, like the Java example.
pub fn example_category() -> Result<(Category, Vec<WorldProperty>), WorldError> {
    let checkbox = WorldProperty::new(
        "axiom:checkbox",
        "Checkbox",
        false,
        Widget::Checkbox,
        |_host, _dim| PropValue::Bool(false),
        |host, _player, dim, value| {
            if let PropValue::Bool(on) = value {
                host.send_chat_to_dimension(dim, &format!("Checkbox: {on}"));
            }
            PropertyUpdateResult::UpdateAndSync
        },
    )?;

    let slider = WorldProperty::new(
        "axiom:slider",
        "Slider",
        false,
        Widget::Slider { min: 0, max: 8 },
        |_host, _dim| PropValue::Int(4),
        |host, _player, dim, value| {
            if let PropValue::Int(n) = value {
                host.send_chat_to_dimension(dim, &format!("Slider: {n}"));
            }
            PropertyUpdateResult::UpdateAndSync
        },
    )?;

    let textbox = WorldProperty::new(
        "axiom:textbox",
        "Textbox",
        false,
        Widget::Textbox,
        |_host, _dim| PropValue::Text("Hello".to_string()),
        |host, _player, dim, value| {
            if let PropValue::Text(text) = value {
                host.send_chat_to_dimension(dim, &format!("Textbox: {text}"));
            }
            PropertyUpdateResult::UpdateAndSync
        },
    )?;

    let button = WorldProperty::new(
        "axiom:button",
        "Button",
        false,
        Widget::Button,
        |_host, _dim| PropValue::Unit,
        |host, _player, dim, _value| {
            host.send_chat_to_dimension(dim, "Button pressed");
            PropertyUpdateResult::UpdateAndSync
        },
    )?;

    Ok((
        Category {
            name: "Examples".to_string(),
            localize_name: false,
        },
        vec![checkbox, slider, textbox, button],
    ))
}
