//! `/axiompapermigrateconfig` (AxiomPaperPlugin `migrateConfig`): the default `config.yml` with the values the
//! current file sets. Works on the text of the top-level keys, so comments and nested blocks survive.

/// The default config AxiomPaper ships.
pub const DEFAULT_CONFIG: &str = include_str!("default_config.yml");

struct Block {
    key: String,
    /// Comment and blank lines in front of the key.
    comments: Vec<String>,
    /// The key line and everything nested under it.
    body: Vec<String>,
}

fn blocks(text: &str) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    for line in text.lines() {
        let nested = line.starts_with(char::is_whitespace) && !line.trim().is_empty();
        if nested || line.starts_with("- ") {
            if let Some(block) = out.last_mut() {
                block.body.append(&mut pending);
                block.body.push(line.to_owned());
                continue;
            }
        }
        if line.trim().is_empty() || line.starts_with('#') {
            pending.push(line.to_owned());
        } else if let Some((key, _)) = line.split_once(':') {
            out.push(Block {
                key: key.trim().to_owned(),
                comments: std::mem::take(&mut pending),
                body: vec![line.to_owned()],
            });
        }
    }
    out
}

/// `(added, removed)`: top-level keys of the default that `current` lacks, and keys of `current` the default lacks.
pub fn outdated(default: &str, current: &str) -> (usize, usize) {
    let (d, c) = (blocks(default), blocks(current));
    let added = d.iter().filter(|b| !c.iter().any(|x| x.key == b.key)).count();
    let removed = c.iter().filter(|b| !d.iter().any(|x| x.key == b.key)).count();
    (added, removed)
}

/// The default config in its own order and with its comments, each key carrying the value `current` sets.
pub fn migrate(default: &str, current: &str) -> String {
    let c = blocks(current);
    let mut lines: Vec<String> = Vec::new();
    for block in blocks(default) {
        lines.extend(block.comments);
        match c.iter().find(|x| x.key == block.key) {
            Some(set) => lines.extend(set.body.iter().cloned()),
            None => lines.extend(block.body),
        }
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT: &str = "# a\nalpha: 1\n\n# b\nbeta: true\n# c\nlimits:\n  example:\n    rate: 20\n";

    #[test]
    fn counts_added_and_removed_keys() {
        assert_eq!(outdated(DEFAULT, "alpha: 5\nold: 1\n"), (2, 1));
        assert_eq!(outdated(DEFAULT, DEFAULT), (0, 0));
    }

    #[test]
    fn keeps_set_values_adds_new_keys_and_drops_removed_ones() {
        let current = "alpha: 5\nold: 1\nlimits:\n  vip:\n    rate: 50\n";
        assert_eq!(
            migrate(DEFAULT, current),
            "# a\nalpha: 5\n\n# b\nbeta: true\n# c\nlimits:\n  vip:\n    rate: 50\n"
        );
    }

    #[test]
    fn the_shipped_default_is_current_with_itself() {
        assert_eq!(outdated(DEFAULT_CONFIG, DEFAULT_CONFIG), (0, 0));
        assert_eq!(migrate(DEFAULT_CONFIG, DEFAULT_CONFIG), DEFAULT_CONFIG);
    }
}
