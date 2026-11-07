//! Scans an Obsidian vault: finds notes and extracts their links and tags.

use crate::items::{self, Item};
use anyhow::{Context, Result};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
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
    /// Its items, read on first use (`Vault::items`). A refreshed note is a new `Note`.
    pub items: OnceLock<Vec<Item>>,
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
    /// What items depend on (flavour, Tasks and daily-note settings), read on first use.
    pub item_settings: OnceLock<items::Settings>,
}

/// The files besides notes that links lead to, by extension, with what each is.
const ATTACHMENTS: &[(&str, &[&str])] = &[
    (
        "image",
        &["png", "jpg", "jpeg", "gif", "svg", "webp", "bmp", "avif"],
    ),
    ("audio", &["mp3", "wav", "m4a", "ogg", "flac", "3gp"]),
    ("video", &["mp4", "webm", "ogv", "mov", "mkv"]),
    ("document", &["pdf"]),
    ("whiteboard", &["canvas"]),
    ("database", &["base"]),
];

/// What kind of file a link target names when it isn't a note (`photo.jpg` → `image`).
/// `Mount St. Helens` is a note: only the extensions above make an attachment.
pub fn attachment_kind(target: &str) -> Option<&'static str> {
    let file = target.rsplit('/').next().unwrap_or(target);
    let ext = file.rsplit_once('.')?.1.to_ascii_lowercase();
    ATTACHMENTS
        .iter()
        .find(|(_, exts)| exts.contains(&ext.as_str()))
        .map(|(kind, _)| *kind)
}

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
            tag: Regex::new(crate::markdown::TAG).unwrap(),
            inline_code: Regex::new(r"`[^`\n]*`").unwrap(),
        }
    }
}

fn parsers() -> &'static Parsers {
    static P: OnceLock<Parsers> = OnceLock::new();
    P.get_or_init(Parsers::new)
}

/// Is this vault-relative path a note cold-lava reads (a `.md` file outside hidden folders)?
pub fn is_note_path(rel: &Path) -> bool {
    rel.extension().is_some_and(|e| e == "md")
        && !rel.components().any(|c| {
            let c = c.as_os_str().to_string_lossy();
            c.starts_with('.') || c == "node_modules"
        })
}

/// Read and parse one note. `None` if it isn't a note, is outside the vault, or can't be read.
pub fn read_note(root: &Path, path: &Path) -> Option<Note> {
    let rel_path = path.strip_prefix(root).ok()?;
    if !is_note_path(rel_path) {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;
    let rel = rel_path
        .with_extension("")
        .to_string_lossy()
        .replace('\\', "/");
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (refs, tags, aliases) = parse(&text, parsers());
    let links = refs.iter().map(|r| r.target.clone()).collect();
    Some(Note {
        rel,
        name,
        path: path.to_path_buf(),
        links,
        refs,
        tags,
        aliases,
        text,
        items: OnceLock::new(),
    })
}

pub fn scan(root: &Path) -> Result<Vault> {
    let root = root
        .canonicalize()
        .with_context(|| format!("vault path not found: {}", root.display()))?;
    let name = root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "vault".into());
    let walker = WalkDir::new(&root).into_iter().filter_entry(|e| {
        let n = e.file_name().to_string_lossy();
        // Skip .obsidian, .trash, .git and friends.
        e.depth() == 0 || !(n.starts_with('.') || n == "node_modules")
    });
    let mut notes: Vec<Note> = walker
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| read_note(&root, e.path()))
        .collect();
    notes.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(Vault {
        root,
        name,
        notes,
        item_settings: OnceLock::new(),
    })
}

/// What happened to a note when the vault was told a path changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoteChange {
    Created(String),
    Changed(String),
    Removed(String),
}

impl Vault {
    pub fn item_settings(&self) -> &items::Settings {
        self.item_settings
            .get_or_init(|| items::Settings::load(&self.root))
    }

    /// The note's items, read the first time they're asked for.
    pub fn items<'a>(&self, note: &'a Note) -> &'a [Item] {
        note.items.get_or_init(|| {
            items::extract(
                &format!("{}.md", note.rel),
                &note.text,
                self.item_settings(),
            )
        })
    }

    /// Bring one path up to date with the disk: read it again, add it, or drop it.
    /// Returns what changed, if anything did (rel paths without `.md`).
    pub fn refresh_path(&mut self, path: &Path) -> Option<NoteChange> {
        let pos = self.notes.iter().position(|n| n.path == path);
        match (read_note(&self.root, path), pos) {
            (Some(note), Some(i)) => {
                if self.notes[i].text == note.text {
                    return None;
                }
                let rel = note.rel.clone();
                self.notes[i] = note;
                Some(NoteChange::Changed(rel))
            }
            (Some(note), None) => {
                let rel = note.rel.clone();
                let at = self.notes.partition_point(|n| n.rel < note.rel);
                self.notes.insert(at, note);
                Some(NoteChange::Created(rel))
            }
            (None, Some(i)) => {
                // Still on disk but unreadable (e.g. mid-write): keep the last good version.
                if path.exists() {
                    return None;
                }
                Some(NoteChange::Removed(self.notes.remove(i).rel))
            }
            (None, None) => None,
        }
    }
}

pub(crate) fn clean_target(t: &str) -> Option<String> {
    let t = t.trim().replace("%20", " ");
    let t = t.strip_suffix(".md").unwrap_or(&t).trim().to_string();
    if t.is_empty() || attachment_kind(&t).is_some() || t.contains("://") {
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
        for target in line_links(raw) {
            add(&mut refs, Some(target), n, raw);
        }
        let line = p.inline_code.replace_all(raw, "");
        for c in p.tag.captures_iter(&line) {
            push_tag(&mut tags, &c[1]);
        }
    }
    (refs, tags, aliases)
}

/// The link targets on one line outside inline code: wikilinks, then Markdown links.
pub fn line_links(line: &str) -> Vec<String> {
    let p = parsers();
    let line = p.inline_code.replace_all(line, "");
    let wiki = p.wikilink.captures_iter(&line);
    let md = p.mdlink.captures_iter(&line);
    wiki.chain(md).filter_map(|c| clean_target(&c[1])).collect()
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

/// Find the vault for a path: the nearest ancestor containing `.obsidian`, else the folder itself.
pub fn find_vault_root(p: &Path) -> PathBuf {
    let abs = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let start = if abs.is_dir() {
        abs.clone()
    } else {
        abs.parent().map(Path::to_path_buf).unwrap_or(abs.clone())
    };
    start
        .ancestors()
        .find(|a| a.join(".obsidian").is_dir())
        .map(Path::to_path_buf)
        .unwrap_or(start)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links_and_tags(text: &str) -> (Vec<String>, Vec<String>) {
        let (refs, tags, _) = parse(text, parsers());
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
    fn attachments_are_known_file_types_and_dotted_note_names_are_notes() {
        assert_eq!(attachment_kind("kilauea-lava-lake.jpg"), Some("image"));
        assert_eq!(attachment_kind("Maps/Hawaii.PDF"), Some("document"));
        assert_eq!(attachment_kind("Welcome.canvas"), Some("whiteboard"));
        assert_eq!(attachment_kind("Mount St. Helens"), None);
        assert_eq!(attachment_kind("David A. Johnston"), None);
        assert_eq!(attachment_kind("v1.2/notes"), None);
        assert_eq!(attachment_kind("Lahar"), None);
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
    fn items_are_read_on_first_use_and_again_after_a_change() {
        let dir = std::env::temp_dir().join(format!("cold-lava-items-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Todo.md"), "- [ ] one\n").unwrap();
        let mut v = scan(&dir).unwrap();
        assert!(
            v.item_settings.get().is_none() && v.notes[0].items.get().is_none(),
            "scanning reads no items"
        );
        assert_eq!(v.items(&v.notes[0]).len(), 1);
        assert!(v.notes[0].items.get().is_some(), "cached");
        fs::write(dir.join("Todo.md"), "- [ ] one\n- [ ] two\n").unwrap();
        let path = v.notes[0].path.clone();
        v.refresh_path(&path);
        assert_eq!(v.items(&v.notes[0]).len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn scans_example_vault() {
        let v = scan(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-vault")).unwrap();
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
