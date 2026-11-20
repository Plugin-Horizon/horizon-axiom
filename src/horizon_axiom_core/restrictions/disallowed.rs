// Ported from AxiomPaperPlugin (MIT, Copyright (c) 2023 Moulberry Pty Ltd): DisallowedBlocks.java.
//
// `disallowed-blocks` in config.yml: a block state a client may not use. Java maps every disallowed state to the
// empty state (void_air) on the client's registry, so a client that sends one places air. An entry is a block state
// string such as `minecraft:oak_stairs[waterlogged=true]`; a bare block name matches every state of that block.
// Tag entries (`#namespace:tag`) are not matched: the port has no block tag table (see the Known gaps in README.md).

/// One parsed entry: a block name and the property values it requires (sorted as written, may be empty).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Disallowed {
    block: String,
    properties: Vec<(String, String)>,
}

impl Disallowed {
    /// Parses one config entry. `None` for a tag and for text BlockStateParser would refuse (Java drops such entries).
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.starts_with('#') {
            return None;
        }
        let (name, rest) = match text.find('[') {
            Some(at) => (&text[..at], Some(&text[at..])),
            None => (text, None),
        };
        let block = normalise_name(name)?;
        let properties = match rest {
            None => Vec::new(),
            Some(rest) => parse_properties(rest.strip_prefix('[')?.strip_suffix(']')?)?,
        };
        Some(Self { block, properties })
    }

    /// Whether the state string `state` (`minecraft:name[k=v,...]`, as `BlockTable::state_string` writes it) is
    /// disallowed by this entry: same block, and every property the entry names has the same value.
    pub fn matches(&self, state: &str) -> bool {
        let (name, props) = split_state(state);
        if name != self.block {
            return false;
        }
        self.properties
            .iter()
            .all(|(key, value)| props.iter().any(|(k, v)| k == key && v == value))
    }
}

/// Every parseable entry of the config list, in order.
pub fn parse_all(entries: &[String]) -> Vec<Disallowed> {
    entries.iter().filter_map(|e| Disallowed::parse(e)).collect()
}

/// Whether any entry disallows the state string.
pub fn is_disallowed(entries: &[Disallowed], state: &str) -> bool {
    entries.iter().any(|entry| entry.matches(state))
}

/// `name` with the `minecraft:` namespace filled in as BlockStateParser does (a bare name is in `minecraft`).
fn normalise_name(name: &str) -> Option<String> {
    let name = name.trim();
    let (namespace, path) = name.split_once(':').unwrap_or(("minecraft", name));
    let ok = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-./".contains(&b));
    (ok(namespace) && ok(path)).then(|| format!("{namespace}:{path}"))
}

/// `[k=v,k2=v2]` contents (without the brackets) as pairs; `None` for an empty pair or one without `=`.
fn parse_properties(inner: &str) -> Option<Vec<(String, String)>> {
    if inner.trim().is_empty() {
        return Some(Vec::new());
    }
    inner
        .split(',')
        .map(|pair| {
            let (key, value) = pair.split_once('=')?;
            let (key, value) = (key.trim(), value.trim());
            (!key.is_empty() && !value.is_empty()).then(|| (key.to_owned(), value.to_owned()))
        })
        .collect()
}

/// A state string split into its block name and its `(key, value)` properties.
fn split_state(state: &str) -> (&str, Vec<(String, String)>) {
    match state.find('[') {
        None => (state, Vec::new()),
        Some(at) => {
            let inner = state[at + 1..].trim_end_matches(']');
            let props = inner
                .split(',')
                .filter_map(|pair| pair.split_once('='))
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect();
            (&state[..at], props)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(list: &[&str]) -> Vec<Disallowed> {
        let owned: Vec<String> = list.iter().map(|s| (*s).to_owned()).collect();
        parse_all(&owned)
    }

    #[test]
    fn a_bare_name_disallows_every_state_of_the_block() {
        let list = entries(&["minecraft:wheat"]);
        assert!(is_disallowed(&list, "minecraft:wheat[age=3]"));
        assert!(is_disallowed(&list, "minecraft:wheat"));
        assert!(!is_disallowed(&list, "minecraft:wheat_seeds"));
        assert!(!is_disallowed(&list, "minecraft:stone"));
    }

    #[test]
    fn a_name_without_namespace_is_minecraft() {
        let list = entries(&["wheat"]);
        assert!(is_disallowed(&list, "minecraft:wheat[age=7]"));
    }

    #[test]
    fn named_properties_must_all_match() {
        let list = entries(&["minecraft:oak_stairs[waterlogged=true]"]);
        assert!(is_disallowed(&list, "minecraft:oak_stairs[facing=north,half=bottom,waterlogged=true,shape=straight]"));
        assert!(!is_disallowed(&list, "minecraft:oak_stairs[facing=north,waterlogged=false]"));
        assert!(!is_disallowed(&list, "minecraft:oak_stairs"));
        let two = entries(&["minecraft:oak_stairs[waterlogged=true,half=top]"]);
        assert!(!is_disallowed(&two, "minecraft:oak_stairs[half=bottom,waterlogged=true]"));
        assert!(is_disallowed(&two, "minecraft:oak_stairs[half=top,waterlogged=true]"));
    }

    #[test]
    fn tags_and_malformed_entries_are_dropped() {
        let list = entries(&["#minecraft:logs", "minecraft:oak_stairs[waterlogged]", "minecraft:[x=y]", "Stone", "minecraft:stone[a=]"]);
        assert!(list.is_empty());
        assert!(!is_disallowed(&list, "minecraft:stone"));
    }

    #[test]
    fn empty_list_allows_everything() {
        assert!(!is_disallowed(&[], "minecraft:tnt[unstable=true]"));
    }

    #[test]
    fn a_state_string_without_properties_splits_cleanly() {
        assert_eq!(split_state("minecraft:air"), ("minecraft:air", Vec::new()));
        assert_eq!(
            split_state("minecraft:door[open=true]"),
            ("minecraft:door", vec![("open".to_owned(), "true".to_owned())])
        );
    }
}
