//! Loads original AxiomPaper `config.yml` files with the port's own loader and prints only key paths and counts:
//! the keys of each file the loader reads, the keys of the file it does not read, the keys it asks for that the
//! file lacks (they read as the default), and parse errors (line number only). Values are never printed.
//!
//! `cargo run --example check_original -- <path to config.yml>...`

use horizon_axiom_core::restrictions::Config;
use horizon_axiom_core::yaml::{self, Node};
use std::collections::BTreeSet;
use std::path::Path;

/// Every key path of the tree, maps included (`limits`, `limits.example`, ...).
fn all_paths(node: &Node, prefix: &str, out: &mut BTreeSet<String>) {
    for entry in node.entries() {
        let path = if prefix.is_empty() { entry.key.clone() } else { format!("{prefix}.{}", entry.key) };
        if entry.value.is_map() {
            all_paths(&entry.value, &path, out);
        }
        out.insert(path);
    }
}

/// The leaf key paths of the tree (lists and scalars); a map is not a leaf.
fn leaves(node: &Node, prefix: &str, out: &mut BTreeSet<String>) {
    for entry in node.entries() {
        let path = if prefix.is_empty() { entry.key.clone() } else { format!("{prefix}.{}", entry.key) };
        if entry.value.is_map() {
            leaves(&entry.value, &path, out);
        } else {
            out.insert(path);
        }
    }
}

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: check_original <config.yml>...");
        std::process::exit(2);
    }
    let mut failed = false;
    for path in paths {
        // The file name only: the folder of a server is not printed.
        let name = Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        println!("== file {name}");
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                println!("  read error: {}", e.kind());
                failed = true;
                continue;
            }
        };
        let root = match yaml::parse(&text) {
            Ok(root) => root,
            Err(e) => {
                println!("  parse error on line {}", e.line);
                failed = true;
                continue;
            }
        };
        let (config, read) = match Config::from_original_yaml(&text) {
            Ok(v) => v,
            Err(e) => {
                println!("  loader error on line {}", e.line);
                failed = true;
                continue;
            }
        };
        let mut in_file = BTreeSet::new();
        all_paths(&root, "", &mut in_file);
        let mut leaves_in_file = BTreeSet::new();
        leaves(&root, "", &mut leaves_in_file);
        let not_read: Vec<&String> = leaves_in_file.iter().filter(|k| !read.contains(*k)).collect();
        let absent: Vec<&String> = read.iter().filter(|k| !in_file.contains(*k)).collect();
        println!(
            "  keys in file: {}, read: {}, not read: {}",
            leaves_in_file.len(),
            leaves_in_file.len() - not_read.len(),
            not_read.len()
        );
        for k in &not_read {
            println!("  NOT READ {k}");
        }
        println!("  asked for, absent from file (default used): {}", absent.len());
        for k in &absent {
            println!("  ABSENT {k}");
        }
        println!("  loaded: {} limit override(s), {} keys read", config.limits.len(), read.len());
    }
    if failed {
        std::process::exit(1);
    }
}
