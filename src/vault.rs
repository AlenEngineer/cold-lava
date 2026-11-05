//! Scans an Obsidian vault: finds notes and extracts their links and tags.

use anyhow::{Context, Result};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Text normalised for searching: lowercase, without accents or Hawaiian ʻokina,
/// so "kil" finds "Kīlauea" and "cafe" finds "Café".
pub fn search_key(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    s.nfd()
        .filter(|c| {
            !unicode_normalization::char::is_combining_mark(*c) && !matches!(c, 'ʻ' | 'ʼ' | '‘')
        })
        .collect::<String>()
        .to_lowercase()
}

pub struct Note {
    /// Path relative to the vault root, without `.md`, using `/` separators.
    pub rel: String,
    /// File stem, shown as the node label.
    pub name: String,
    pub path: PathBuf,
    /// Raw link targets (already stripped of `#heading`, `|alias`, `.md`).
    pub links: Vec<String>,
    /// The same links with where they are: for backlinks.
    pub refs: Vec<Ref>,
    pub tags: Vec<String>,
    /// Frontmatter `aliases:` (other names the quick switcher finds the note by).
    pub aliases: Vec<String>,
    /// The note's full text, for full-text search.
    pub text: String,
}

/// One link in a note.
#[derive(Clone, Debug, PartialEq)]
pub struct Ref {
    pub target: String,
    /// 1-based line number.
    pub line: usize,
    /// The line it's on, trimmed (for showing backlinks in context).
    pub context: String,
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
        let (refs, tags, aliases) = parse(&text, &p);
        let links = refs.iter().map(|r| r.target.clone()).collect();
        notes.push(Note {
            rel,
            name,
            path: path.to_path_buf(),
            links,
            refs,
            tags,
            aliases,
            text,
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

fn parse(text: &str, p: &Parsers) -> (Vec<Ref>, Vec<String>, Vec<String>) {
    let mut refs = Vec::new();
    let mut tags = Vec::new();
    let mut aliases: Vec<String> = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    let add = |refs: &mut Vec<Ref>, target: Option<String>, i: usize, line: &str| {
        if let Some(target) = target {
            let mut context: String = line.trim().chars().take(240).collect();
            if line.trim().chars().count() > 240 {
                context.push('…');
            }
            refs.push(Ref {
                target,
                line: i + 1,
                context,
            });
        }
    };

    // YAML frontmatter: only `tags:` matters here.
    if lines.first().map(|l| l.trim_end()) == Some("---") {
        i = 1;
        let mut in_tags = false;
        let mut in_aliases = false;
        while i < lines.len() {
            let line = lines[i];
            let t = line.trim();
            if t == "---" || t == "..." {
                i += 1;
                break;
            }
            if let Some(rest) = t
                .strip_prefix("aliases:")
                .or_else(|| t.strip_prefix("alias:"))
            {
                in_aliases = true;
                in_tags = false;
                let rest = rest.trim().trim_start_matches('[').trim_end_matches(']');
                aliases.extend(rest.split(',').filter_map(clean_alias));
            } else if in_aliases && t.starts_with("- ") {
                aliases.extend(clean_alias(&t[2..]));
            } else if let Some(rest) = t.strip_prefix("tags:").or_else(|| t.strip_prefix("tag:")) {
                in_tags = true;
                in_aliases = false;
                let rest = rest.trim().trim_start_matches('[').trim_end_matches(']');
                for tag in rest.split(',') {
                    push_tag(&mut tags, tag);
                }
            } else if in_tags && t.starts_with("- ") {
                push_tag(&mut tags, &t[2..]);
            } else if !t.is_empty() {
                in_tags = false;
                in_aliases = false;
            }
            // Wikilinks inside frontmatter properties count too, like in Obsidian.
            for c in p.wikilink.captures_iter(line) {
                add(&mut refs, clean_target(&c[1]), i, line);
            }
            i += 1;
        }
    }

    let mut in_fence = false;
    for (n, raw) in lines.iter().enumerate().skip(i) {
        let t = raw.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let line = p.inline_code.replace_all(raw, "");
        for c in p.wikilink.captures_iter(&line) {
            add(&mut refs, clean_target(&c[1]), n, raw);
        }
        for c in p.mdlink.captures_iter(&line) {
            add(&mut refs, clean_target(&c[1]), n, raw);
        }
        for c in p.tag.captures_iter(&line) {
            push_tag(&mut tags, &c[1]);
        }
    }
    (refs, tags, aliases)
}

fn clean_alias(raw: &str) -> Option<String> {
    let a = raw.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    (!a.is_empty()).then(|| a.to_string())
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
        let (refs, tags, _) = parse(text, &Parsers::new());
        (refs.into_iter().map(|r| r.target).collect(), tags)
    }

    #[test]
    fn link_positions() {
        let (refs, _, _) = parse(
            "---\nup: \"[[Parent]]\"\n---\n\nSee [[A]] and\n[[B|bee]] here",
            &Parsers::new(),
        );
        let pos: Vec<(&str, usize, &str)> = refs
            .iter()
            .map(|r| (r.target.as_str(), r.line, r.context.as_str()))
            .collect();
        assert_eq!(
            pos,
            [
                ("Parent", 2, "up: \"[[Parent]]\""),
                ("A", 5, "See [[A]] and"),
                ("B", 6, "[[B|bee]] here")
            ]
        );
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
    fn frontmatter_aliases() {
        let (_, _, a) = parse(
            "---\naliases: [Big Island, \"Hawaiʻi\"]\ntags: x\n---\n",
            &Parsers::new(),
        );
        assert_eq!(a, ["Big Island", "Hawaiʻi"]);
        let (_, t, a) = parse(
            "---\naliases:\n  - One\n  - 'Two'\ntags:\n  - t\n---\n",
            &Parsers::new(),
        );
        assert_eq!(
            (a, t),
            (
                vec!["One".to_string(), "Two".to_string()],
                vec!["t".to_string()]
            )
        );
    }

    #[test]
    fn frontmatter_list_tags() {
        let (_, tags) = links_and_tags("---\ntags:\n  - one\n  - \"two\"\ntitle: x\n---\n");
        assert_eq!(tags, ["one", "two"]);
    }

    #[test]
    fn scans_example_vault() {
        let v = scan(&Path::new(env!("CARGO_MANIFEST_DIR")).join("example-vault")).unwrap();
        assert_eq!(v.name, "example-vault");
        assert_eq!(v.notes.len(), 33);
        // Hidden folders (.obsidian) are skipped; subfolders and non-ASCII names are included.
        assert!(v.notes.iter().all(|n| !n.rel.starts_with('.')));
        let kilauea = v
            .notes
            .iter()
            .find(|n| n.rel == "Volcanoes/Kīlauea")
            .unwrap();
        assert_eq!(kilauea.tags, ["volcano/shield"]);
        assert!(
            !kilauea.links.iter().any(|l| l.ends_with(".jpg")),
            "attachments are not links"
        );
    }
}

#[cfg(test)]
mod search_tests {
    #[test]
    fn search_ignores_accents_and_case() {
        use super::search_key as k;
        assert!(k("Volcanoes/Kīlauea.md").contains(&k("kil")));
        assert!(k("Eyjafjallajökull").contains(&k("jokull")));
        assert!(k("ʻAʻā").contains("aa"));
        assert!(k("Café Notes").contains(&k("cafe")));
    }
}
