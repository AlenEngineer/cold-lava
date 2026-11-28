use super::pulldown;
use crate::configured::Configured;
use crate::cst;
use crate::items::tests::{TRICKY, every_type};
use crate::items::{Item, Settings, extract};
use crate::types;
use std::path::Path;

/// Markdown pulldown-cmark read otherwise than Obsidian shows it, where the items differ
/// from the ones it gave.
const DIFFERENT: &[&str] = &[
    // A frontmatter whose first line is blank, as the Kanban plugin writes it, was a rule
    // and Markdown.
    "---\n\nkanban-plugin: basic\n- [ ] YAML\n\n---\n\n- [ ] Pack\n",
    // A `---` line after a blank line, with a `---` or `...` line below it, opened YAML in
    // the middle of a note, hiding what was between.
    "Intro\n\n---\n- [ ] between the rules\n---\n\n- [ ] after\n",
    // A byte order mark made the first line text.
    "\u{feff}- [ ] first\n- [ ] second\n",
    // `[[#Heading]]` held the tag `Heading`.
    "See [[#Heading]].\n",
    // A highlight was text to pulldown-cmark, so the tag right after its `==` was none. It
    // is one in Obsidian.
    "Read ==#marked==.\n",
    // `[[A\|B]]` in a table cell, where the pipe before the alias is escaped so that it
    // doesn't end the cell, linked to `A\`. The note is `A`.
    "| [[A\\|B]] | c |\n| --- | --- |\n| d | [[E\\|F]] |\n",
];

/// How the tree's items and pulldown-cmark's differ for one note, if they do.
fn difference(name: &str, text: &str, s: &Settings) -> Option<String> {
    let (tree, old) = (extract(name, text, s), pulldown::extract(name, text, s));
    if tree == old {
        return None;
    }
    let short = |i: Option<&Item>| format!("{i:?}").chars().take(400).collect::<String>();
    let mut out = format!(
        "{name} ({}): {} items, pulldown-cmark {}",
        s.flavour.name,
        tree.len(),
        old.len()
    );
    let n = tree.len().max(old.len());
    for k in (0..n).filter(|&k| tree.get(k) != old.get(k)).take(3) {
        out += &format!(
            "\n  tree: {}\n  then: {}",
            short(tree.get(k)),
            short(old.get(k))
        );
    }
    Some(out)
}

/// The walk through the tree gives every note the items pulldown-cmark gave it: the example
/// workspace as `cold-lava items` reads it, then every Markdown file of the repository and
/// the tricky cases in each flavour, with types that read every kind of block.
#[test]
fn the_tree_reads_items_as_pulldown_cmark_did() {
    let mut differ = Vec::new();
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-workspace");
    let s = Settings::load(&workspace, &Configured::default());
    for e in walkdir::WalkDir::new(&workspace).into_iter().flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "md") {
            let name = p.strip_prefix(&workspace).unwrap().to_string_lossy();
            let text = std::fs::read_to_string(p).unwrap();
            differ.extend(difference(&name, &text, &s));
        }
    }
    // The one way they differ: pulldown-cmark took a frontmatter whose first line is blank,
    // as the Kanban plugin writes it, for a rule and Markdown. The tree reads frontmatter,
    // as Obsidian does (`a_frontmatter_with_a_blank_first_line_is_frontmatter`).
    let corpus: Vec<(String, String)> = cst::corpus()
        .into_iter()
        .filter(|(_, text)| !text.starts_with("---\n\n"))
        .collect();
    for flavour in ["obsidian", "github", "commonmark"] {
        let s = every_type(flavour);
        for (name, text) in &corpus {
            differ.extend(difference(name, text, &s));
        }
        for (k, text) in TRICKY.iter().enumerate() {
            differ.extend(difference(&format!("Tricky {k}.md"), text, &s));
        }
    }
    let s = every_type("obsidian");
    for text in DIFFERENT {
        if difference("Different.md", text, &s).is_none() {
            differ.push(format!("reads alike now, so it isn't DIFFERENT: {text:?}"));
        }
    }
    assert!(
        differ.is_empty(),
        "{} read differently:\n{}",
        differ.len(),
        differ.join("\n")
    );
}

/// What reading every note's items costs, pulldown-cmark's pass against the tree's walk,
/// with the workspace's types and with none (the pass and the ids alone). Run on
/// `scripts/bench-items.py`'s workspace: `COLD_LAVA_BENCH_WS=DIR cargo test --release -p
/// cold-lava-engine items_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn items_cost() {
    use std::time::{Duration, Instant};
    let dir = std::env::var("COLD_LAVA_BENCH_WS").expect("COLD_LAVA_BENCH_WS");
    let root = Path::new(&dir);
    let none = Configured::default();
    let typed = Settings::load(root, &none);
    let plain = Settings::load_with(root, None, &[], types::load(Vec::<types::Layer>::new()));
    let notes: Vec<(String, String)> = walkdir::WalkDir::new(root)
        .into_iter()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .filter_map(|e| {
            let name = e.path().strip_prefix(root).ok()?;
            let name = name.to_string_lossy().to_string();
            Some((name, std::fs::read_to_string(e.path()).ok()?))
        })
        .collect();
    let bytes: usize = notes.iter().map(|n| n.1.len()).sum();
    type Read = fn(&str, &str, &Settings) -> Vec<Item>;
    let sides: [(&str, Read, &Settings); 4] = [
        ("pulldown-cmark, with the types", pulldown::extract, &typed),
        ("the tree, with the types", extract, &typed),
        ("pulldown-cmark, no types", pulldown::extract, &plain),
        ("the tree, no types", extract, &plain),
    ];
    let mut runs: Vec<Vec<Duration>> = vec![Vec::new(); sides.len()];
    let mut items = vec![0usize; sides.len()];
    // The sides take turns, so drift and caches favour none.
    for _ in 0..9 {
        for (k, (_, read, s)) in sides.iter().enumerate() {
            let t = Instant::now();
            items[k] = notes
                .iter()
                .map(|(name, text)| read(name, text, s).len())
                .sum();
            runs[k].push(t.elapsed());
        }
    }
    println!(
        "items_cost: {} notes, {} KiB; median (min-max) of 9:",
        notes.len(),
        bytes / 1024
    );
    for (k, (what, ..)) in sides.iter().enumerate() {
        runs[k].sort();
        println!(
            "  {what}: {:?} ({:?}-{:?}), {} items",
            runs[k][4], runs[k][0], runs[k][8], items[k]
        );
    }
}
