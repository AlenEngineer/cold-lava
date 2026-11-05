//! Scans an Obsidian vault: finds notes and extracts their links and tags.

use anyhow::{Context, Result};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub struct Note {
    /// Path relative to the vault root, without `.md`, using `/` separators.
    pub rel: String,
    /// File stem, shown as the node label.
    pub name: String,
    pub path: PathBuf,
    /// Raw link targets (already stripped of `#heading`, `|alias`, `.md`).
    pub links: Vec<String>,
    pub tags: Vec<String>,
}

pub struct Vault {
    pub root: PathBuf,
    pub name: String,
    pub notes: Vec<Note>,
}

const ATTACHMENT_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "svg", "webp", "bmp", "avif", "pdf", "mp3", "mp4", "webm", "wav",
    "ogg", "m4a", "flac", "mov", "mkv", "canvas", "base",
];

struct Parsers {
    wikilink: Regex,
    mdlink: Regex,
    tag: Regex,
    inline_code: Regex,
}

impl Parsers {
    fn new() -> Self {
        Self {
            // [[target]], [[target#heading]], [[target|alias]], ![[embed]]
            wikilink: Regex::new(r"\[\[([^\]\|#\^]*)[^\]]*\]\]").unwrap(),
            // [text](some/note.md) — local markdown links only
            mdlink: Regex::new(r"\]\(([^)\s]+?\.md)(?:#[^)]*)?\)").unwrap(),
            tag: Regex::new(r"(?:^|[\s(,])#([\p{L}\p{N}_/\-]+)").unwrap(),
            inline_code: Regex::new(r"`[^`\n]*`").unwrap(),
        }
    }
}

pub fn scan(root: &Path) -> Result<Vault> {
    let root = root
        .canonicalize()
        .with_context(|| format!("vault path not found: {}", root.display()))?;
    let name = root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "vault".into());
    let p = Parsers::new();
    let mut notes = Vec::new();

    let walker = WalkDir::new(&root).into_iter().filter_entry(|e| {
        let n = e.file_name().to_string_lossy();
        // Skip .obsidian, .trash, .git and friends.
        e.depth() == 0 || !(n.starts_with('.') || n == "node_modules")
    });
    for entry in walker.filter_map(|e| e.ok()) {
        let path = entry.path();
        if !entry.file_type().is_file() || path.extension().is_none_or(|e| e != "md") {
            continue;
        }
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .with_extension("")
            .to_string_lossy()
            .replace('\\', "/");
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (links, tags) = parse(&text, &p);
        notes.push(Note {
            rel,
            name,
            path: path.to_path_buf(),
            links,
            tags,
        });
    }
    notes.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(Vault { root, name, notes })
}

fn is_attachment(target: &str) -> bool {
    let file = target.rsplit('/').next().unwrap_or(target);
    match file.rsplit_once('.') {
        Some((_, ext)) => ATTACHMENT_EXTS.contains(&ext.to_ascii_lowercase().as_str()),
        None => false,
    }
}

fn clean_target(t: &str) -> Option<String> {
    let t = t.trim().replace("%20", " ");
    let t = t.strip_suffix(".md").unwrap_or(&t).trim().to_string();
    if t.is_empty() || is_attachment(&t) || t.contains("://") {
        None
    } else {
        Some(t)
    }
}

fn parse(text: &str, p: &Parsers) -> (Vec<String>, Vec<String>) {
    let mut links = Vec::new();
    let mut tags = Vec::new();
    let mut lines = text.lines().peekable();

    // YAML frontmatter: only `tags:` matters here.
    if lines.peek().map(|l| l.trim_end()) == Some("---") {
        lines.next();
        let mut in_tags = false;
        for line in lines.by_ref() {
            let t = line.trim();
            if t == "---" || t == "..." {
                break;
            }
            if let Some(rest) = t.strip_prefix("tags:").or_else(|| t.strip_prefix("tag:")) {
                in_tags = true;
                let rest = rest.trim().trim_start_matches('[').trim_end_matches(']');
                for tag in rest.split(',') {
                    push_tag(&mut tags, tag);
                }
            } else if in_tags && t.starts_with("- ") {
                push_tag(&mut tags, &t[2..]);
            } else if !t.is_empty() {
                in_tags = false;
            }
            // Wikilinks inside frontmatter properties count too, like in Obsidian.
            for c in p.wikilink.captures_iter(line) {
                links.extend(clean_target(&c[1]));
            }
        }
    }

    let mut in_fence = false;
    for line in lines {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let line = p.inline_code.replace_all(line, "");
        for c in p.wikilink.captures_iter(&line) {
            links.extend(clean_target(&c[1]));
        }
        for c in p.mdlink.captures_iter(&line) {
            links.extend(clean_target(&c[1]));
        }
        for c in p.tag.captures_iter(&line) {
            push_tag(&mut tags, &c[1]);
        }
    }
    (links, tags)
}

fn push_tag(tags: &mut Vec<String>, raw: &str) {
    let t = raw
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .trim_start_matches('#');
    // A tag needs at least one non-digit character (`#123` is not a tag).
    if !t.is_empty() && !t.chars().all(|c| c.is_ascii_digit()) {
        let t = t.to_lowercase();
        if !tags.contains(&t) {
            tags.push(t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links_and_tags(text: &str) -> (Vec<String>, Vec<String>) {
        parse(text, &Parsers::new())
    }

    #[test]
    fn wikilink_forms() {
        let (links, _) = links_and_tags(
            "[[Plain]] [[With Alias|shown]] [[Heading#Section]] ![[Embedded]] [[Block^abc]] [[folder/Deep]]",
        );
        assert_eq!(
            links,
            [
                "Plain",
                "With Alias",
                "Heading",
                "Embedded",
                "Block",
                "folder/Deep"
            ]
        );
    }

    #[test]
    fn markdown_links_and_attachments() {
        let (links, _) = links_and_tags(
            "[a](Other%20Note.md) [b](https://example.com/x.md) ![[photo.png]] [[file.pdf]] [[v1.2 notes]]",
        );
        // Wikilinks are collected before markdown links on each line.
        assert_eq!(links, ["v1.2 notes", "Other Note"]);
    }

    #[test]
    fn code_is_ignored() {
        let (links, tags) = links_and_tags("```\n[[InFence]] #fenced\n```\n`[[Inline]]` [[Real]]");
        assert_eq!(links, ["Real"]);
        assert!(tags.is_empty());
    }

    #[test]
    fn tags_inline_and_frontmatter() {
        let text = "---\ntags: [Project, area/work]\naliases:\n  - x\n---\n# Heading\nText #Idea and #123 and #nested/tag.";
        let (_, tags) = links_and_tags(text);
        assert_eq!(tags, ["project", "area/work", "idea", "nested/tag"]);
    }

    #[test]
    fn frontmatter_list_tags() {
        let (_, tags) = links_and_tags("---\ntags:\n  - one\n  - \"two\"\ntitle: x\n---\n");
        assert_eq!(tags, ["one", "two"]);
    }

    #[test]
    fn scans_demo_vault() {
        let v = scan(&Path::new(env!("CARGO_MANIFEST_DIR")).join("demo-vault")).unwrap();
        assert_eq!(v.name, "demo-vault");
        assert!(v.notes.len() >= 30);
        // Hidden folders (.obsidian) are skipped; subfolders are included.
        assert!(v.notes.iter().all(|n| !n.rel.starts_with('.')));
        assert!(v.notes.iter().any(|n| n.rel.starts_with("Projects/")));
    }
}
