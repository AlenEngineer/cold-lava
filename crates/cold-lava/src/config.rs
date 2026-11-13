//! `~/.config/cold-lava/config.toml`, read once at startup. A mistake in it never stops the
//! app: the entry falls back to its default and the problem is listed in `errors`.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::commands::{self, BOARD, Entry, GRAPH, Key, Keymap, MenuCommand, PANE, READER};
use crate::markdown::{BUILT_IN, Flavour, Syntax};
use crate::problem::Problem;

#[derive(Default)]
pub struct Config {
    /// Keys by command id, only for ids that exist (the extensions' included) and keys that
    /// parse.
    pub keys: BTreeMap<String, Vec<Key>>,
    /// `[markdown] flavour`, resolved. `None` lets the workspace decide (`markdown::flavour_for`).
    pub flavour: Option<Flavour>,
    /// `[types]` as written, for the engine to check (`types::Layer::config`).
    pub types: toml::Table,
    /// `[attributes]` as written: how Markdown writes attributes, for the engine to check
    /// (`owner::markdown::attributes::Layer::config`).
    pub attributes: toml::Table,
    /// `[[shapes]]` and `[glyphs]` as written, for the engine to check (`shapes::Layer::config`).
    pub shapes: Vec<toml::Table>,
    pub glyphs: toml::Table,
    /// `[style]`, for the engine (`style::set_configured`).
    pub style: crate::style::Overrides,
    /// The problems in the file, each in words a person reads.
    pub errors: Vec<Problem>,
    /// One line per setting that works but should be rewritten, such as an old command name.
    pub warnings: Vec<String>,
}

/// Each line as a problem of its own.
fn problems(lines: Vec<String>) -> impl Iterator<Item = Problem> {
    lines.into_iter().map(Problem::from)
}

/// A problem in `[section]` of the file: said under `[section]`, and about it, so that a
/// command can pick a section's problems out (`cold-lava style` takes `[style]`'s). Every
/// problem with a section is made here.
fn in_section(section: &str, why: impl Into<Problem>) -> Problem {
    why.into().under(format!("[{section}]")).about(section)
}

/// The file as written. Other top-level sections are ignored, for later use.
#[derive(Deserialize)]
struct File {
    #[serde(default)]
    keys: BTreeMap<String, toml::Value>,
    #[serde(default)]
    markdown: toml::Table,
    #[serde(default)]
    types: toml::Table,
    #[serde(default)]
    attributes: toml::Table,
    #[serde(default)]
    shapes: Vec<toml::Table>,
    #[serde(default)]
    glyphs: toml::Table,
    #[serde(default)]
    style: toml::Table,
}

/// `[markdown.flavours.NAME]`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Custom {
    #[serde(default = "commonmark")]
    base: String,
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove: Vec<String>,
}

fn commonmark() -> String {
    "commonmark".to_string()
}

/// What a problem with `[style]` is about, so `cold-lava style` can pick those out.
pub const STYLE: &str = "style";

static CONFIG: OnceLock<Config> = OnceLock::new();

pub fn path() -> Option<PathBuf> {
    crate::paths::config_dir().map(|c| c.join("cold-lava/config.toml"))
}

/// Reads the file, checking `[keys]` against every screen's commands, with `menu` as the menu's:
/// the core's and the loaded extensions'.
pub fn load(menu: &[Entry<MenuCommand>]) {
    let _ = CONFIG.set(read(path(), menu));
    crate::markdown::set_configured(get().flavour.clone());
    crate::style::set_configured(get().style.clone());
}

fn read(path: Option<PathBuf>, menu: &[Entry<MenuCommand>]) -> Config {
    match path.map(std::fs::read_to_string) {
        Some(Ok(text)) => parse(&text, menu),
        Some(Err(e)) if e.kind() != std::io::ErrorKind::NotFound => Config {
            errors: vec![format!("can't read it: {e}").into()],
            ..Config::default()
        },
        _ => Config::default(),
    }
}

/// The loaded config, or the defaults when `load()` never ran (tests).
pub fn get() -> &'static Config {
    CONFIG.get_or_init(Config::default)
}

/// Commands renamed after a release, old id first. Keys set under the old id still apply.
const RENAMED: &[(&str, &str)] = &[
    ("vault.add", "workspace.add"),
    ("vault.rescan", "workspace.rescan"),
    ("vault.tidy", "workspace.tidy"),
];

pub fn parse(text: &str, menu: &[Entry<MenuCommand>]) -> Config {
    let file: File = match toml::from_str(text) {
        Ok(f) => f,
        Err(e) => {
            return Config {
                errors: vec![Problem::toml_file(&e, text)],
                ..Config::default()
            };
        }
    };
    let mut ids: HashSet<&str> = commands::ids().collect();
    ids.extend(menu.iter().map(|c| c.id.as_str()));
    let mut config = Config::default();
    for (mut id, value) in file.keys {
        if let Some(&(old, new)) = RENAMED.iter().find(|(old, _)| *old == id) {
            config.warnings.push(format!(
                "[keys] {old:?} is now called {new:?}; the keys still apply"
            ));
            id = new.to_string();
        }
        if !ids.contains(id.as_str()) {
            (config.errors).push(in_section("keys", format!("{id:?} is not a command")));
            continue;
        }
        match parse_keys(&value) {
            Ok(keys) if keys.is_empty() && id == "pane.leave" => config.errors.push(in_section(
                "keys",
                format!("{id:?} = [] would leave no way out of a pane; it keeps its default keys"),
            )),
            Ok(keys) => {
                config.keys.insert(id, keys);
            }
            Err(e) => (config.errors).push(in_section("keys", format!("{id:?}: {e}"))),
        }
    }
    config.flavour = markdown(file.markdown, &mut config.errors);
    config.types = file.types;
    config.attributes = file.attributes;
    config.shapes = file.shapes;
    config.glyphs = file.glyphs;
    match crate::style::Overrides::from_table(file.style) {
        Ok(o) => config.style = o,
        Err(e) => config.errors.push(in_section(STYLE, e)),
    }
    config
        .errors
        .extend(problems(Keymap::with(menu, &config.keys).1));
    let keys = &config.keys;
    config
        .errors
        .extend(problems(Keymap::with(&commands::entries(GRAPH), keys).1));
    config
        .errors
        .extend(problems(Keymap::with(&commands::entries(BOARD), keys).1));
    config
        .errors
        .extend(problems(Keymap::with(&commands::entries(READER), keys).1));
    config
        .errors
        .extend(problems(Keymap::with(&commands::entries(PANE), keys).1));
    config.errors.extend(problems(
        Keymap::with(&commands::entries(commands::SHAPE_EDITOR), keys).1,
    ));
    config
}

fn markdown(mut section: toml::Table, errors: &mut Vec<Problem>) -> Option<Flavour> {
    let mut custom = BTreeMap::new();
    // Broken flavours are reported once, not again as unknown when chosen.
    let mut broken = BTreeSet::new();
    match section.remove("flavours") {
        None => {}
        Some(toml::Value::Table(flavours)) => {
            for (name, value) in flavours {
                match custom_flavour(&name, value) {
                    Ok(f) => {
                        custom.insert(name, f);
                    }
                    Err(problem) => {
                        errors.push(in_section(&format!("markdown.flavours.{name}"), problem));
                        broken.insert(name);
                    }
                }
            }
        }
        Some(v) => errors.push(in_section(
            "markdown",
            format!("flavours: expected a table, got {v}"),
        )),
    }
    let chosen = match section.remove("flavour") {
        None => None,
        Some(toml::Value::String(name)) => {
            let found = custom.remove(&name).or_else(|| Flavour::built_in(&name));
            if found.is_none() && !broken.contains(&name) {
                let built_in: Vec<&str> = BUILT_IN.iter().map(|f| f.0).collect();
                let built_in = built_in.join(", ");
                errors.push(in_section(
                    "markdown",
                    format!(
                        "flavour {name:?} is not built in ({built_in}) or defined under \
                         [markdown.flavours]"
                    ),
                ));
            }
            found
        }
        Some(v) => {
            errors.push(in_section(
                "markdown",
                format!("flavour: expected a name, got {v}"),
            ));
            None
        }
    };
    for key in section.keys() {
        errors.push(in_section("markdown", format!("{key:?} is not a setting")));
    }
    chosen
}

/// The flavour `[markdown.flavours.NAME]` defines, or why it defines none.
fn custom_flavour(name: &str, value: toml::Value) -> Result<Flavour, Problem> {
    if Flavour::built_in(name).is_some() {
        return Err(Problem::line(format!(
            "{name:?} is a built-in flavour; give yours another name"
        )));
    }
    let custom: Custom = (value.try_into()).map_err(|e| Problem::toml(&e))?;
    let mut flavour = Flavour::built_in(&custom.base).ok_or_else(|| {
        Problem::line(format!("base {:?} is not a built-in flavour", custom.base))
    })?;
    flavour.name = name.to_string();
    let syntax = |s: &String| {
        let not_one = || Problem::line(format!("{s:?} is not a syntax ({})", Syntax::names()));
        Syntax::from_name(s).ok_or_else(not_one)
    };
    for s in &custom.add {
        flavour.syntax.insert(syntax(s)?);
    }
    for s in &custom.remove {
        flavour.syntax.remove(&syntax(s)?);
    }
    Ok(flavour)
}

/// `"G"`, `["p", "ctrl+p"]` or `[]`.
fn parse_keys(value: &toml::Value) -> Result<Vec<Key>, String> {
    let strings: Vec<&toml::Value> = match value {
        toml::Value::Array(a) => a.iter().collect(),
        v => vec![v],
    };
    strings
        .into_iter()
        .map(|v| {
            let s = v
                .as_str()
                .ok_or_else(|| format!("expected a key like \"g\" or \"ctrl+o\", got {v}"))?;
            Key::parse(s).ok_or_else(|| format!("{s:?} is not a key"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{MenuAction, PaneAction};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// The file read with no extensions loaded.
    fn parse(text: &str) -> Config {
        super::parse(text, &commands::menu([]).0)
    }

    /// A file that doesn't parse is shown as the parser shows it, line for line (but for
    /// the CR of a CR LF line end), and its message is one line whatever it quotes.
    #[test]
    fn a_file_that_doesnt_parse_keeps_the_parsers_lines_and_its_message_whole() {
        let texts = [
            "[keys\n",
            "[keys]\n\"graph.open\" = [\n",
            "keys = 7\n",
            "[keys]\n\"graph.open\" = \"g\"\n[markdown]\nflavour = [",
            "[keys]\n\t\"graph.open\" = = 1\n",
            "[keys]\n\"日本語のキー\" = = 1\n",
            "[keys]\r\n\"graph.open\" = [\r\n",
            "[keys]\n\"graph.open\" = [\r",
            "[keys]\n\"a\\nb\" = 1\n\"a\\nb\" = 2\n",
        ];
        for text in texts {
            let Err(e) = toml::from_str::<File>(text) else {
                panic!("{text:?} parses");
            };
            let problem = Problem::toml_file(&e, text);
            // Only the CR of a CR LF line end goes: one that no LF follows in the file is
            // the line's own and is shown.
            let as_written = e.to_string();
            let as_written = as_written.strip_suffix('\n').unwrap_or(&as_written);
            let as_written = match text.contains("\r\n") {
                true => as_written.replace("[\r\n", "[\n"),
                false => as_written.to_string(),
            };
            assert_eq!(problem.lines().join("\n"), as_written, "{text:?}");
            let ends_in_cr = problem.lines()[2].ends_with('\r');
            assert_eq!(ends_in_cr, text.ends_with('\r'), "{text:?}");
            let (message, place) = problem.lines().split_last().expect("lines");
            assert_eq!(message, e.message(), "{text:?}");
            assert_eq!(
                place.len(),
                4,
                "a header, a gutter, the line, a caret: {text:?}"
            );
            assert!(place.iter().all(|line| !line.contains('\n')), "{text:?}");
        }
    }

    /// A setting read from a table has no text to point into: the parser says what is
    /// wrong, then the keys that lead there on a line of their own. Those are two lines,
    /// and a key that holds a line break doesn't make a third.
    #[test]
    fn a_value_that_doesnt_fit_says_where_on_a_line_of_its_own() {
        let table: toml::Table = toml::from_str("base = 3\n").unwrap();
        let Err(e) = toml::Value::Table(table).try_into::<Custom>() else {
            panic!("a number is no base");
        };
        assert_eq!(
            Problem::toml(&e).lines(),
            ["invalid type: integer `3`, expected a string", "in `base`"]
        );

        let table: toml::Table = toml::from_str("\"a\\nFAKE\" = \"x\"\n").unwrap();
        let read = toml::Value::Table(table).try_into::<BTreeMap<String, u8>>();
        let e = read.expect_err("a string is no number");
        let lines = Problem::toml(&e).lines().to_vec();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(lines[0], e.message());
        assert_eq!(lines[1], "in `a\nFAKE`");
        let problem = in_section("markdown.flavours.x", Problem::toml(&e));
        assert_eq!(problem.lines(), lines);
        assert!(problem.said()[0].starts_with("[markdown.flavours.x] invalid type: "));
        assert!(problem.is_about("markdown.flavours.x"));
    }

    fn menu(text: &str) -> (Keymap<MenuCommand>, Vec<Problem>) {
        let c = parse(text);
        (Keymap::with(&commands::menu([]).0, &c.keys).0, c.errors)
    }

    fn core(a: MenuAction) -> Option<MenuCommand> {
        Some(MenuCommand::Core(a))
    }

    fn press(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn override_replaces_the_default_keys() {
        let (keys, errors) = menu("[keys]\n\"graph.open\" = \"G\"\n");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(keys.get(press('G')), core(MenuAction::Graph));
        assert_eq!(keys.get(press('g')), None);
        assert_eq!(keys.label("graph.open"), "G");
    }

    #[test]
    fn list_and_empty_list() {
        let (keys, errors) =
            menu("[keys]\n\"note.switch\" = [\"p\", \"ctrl+p\"]\n\"workspace.add\" = []\n");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(keys.label("note.switch"), "p / Ctrl-p");
        let ctrl_p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert_eq!(keys.get(ctrl_p), core(MenuAction::Switcher));
        assert_eq!(keys.label("workspace.add"), "");
        assert_eq!(keys.get(press('a')), None);
    }

    #[test]
    fn unknown_command_is_reported_and_the_rest_applies() {
        let (keys, errors) = menu("[keys]\n\"graph.opne\" = \"G\"\n\"workspace.add\" = \"V\"\n");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].first().contains("graph.opne"));
        assert_eq!(keys.label("graph.open"), "g");
        assert_eq!(keys.label("workspace.add"), "V");
    }

    #[test]
    fn an_old_command_name_still_binds_with_a_warning() {
        let text = "[keys]\n\"vault.add\" = \"V\"\n\"vault.tidy\" = []\n";
        let c = parse(text);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        assert_eq!(c.warnings.len(), 2, "{:?}", c.warnings);
        assert!(c.warnings[0].contains("\"vault.add\" is now called \"workspace.add\""));
        let (keys, _) = menu(text);
        assert_eq!(keys.label("workspace.add"), "V");
        assert_eq!(keys.label("workspace.tidy"), "");
    }

    #[test]
    fn the_new_command_name_wins_over_the_old_one() {
        let (keys, _) = menu("[keys]\n\"workspace.add\" = \"W\"\n\"vault.add\" = \"V\"\n");
        assert_eq!(keys.label("workspace.add"), "W");
    }

    #[test]
    fn bad_key_is_reported_and_the_default_kept() {
        let (keys, errors) = menu("[keys]\n\"graph.open\" = [\"G\", \"hyper+g\"]\n");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].first().contains("hyper+g"));
        assert_eq!(keys.label("graph.open"), "g");
        let (_, errors) = menu("[keys]\n\"graph.open\" = 7\n");
        assert_eq!(errors.len(), 1, "{errors:?}");
    }

    #[test]
    fn clash_in_one_screen_keeps_both_defaults() {
        let (keys, errors) = menu("[keys]\n\"graph.open\" = \"x\"\n\"boards.open\" = \"x\"\n");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].first().contains("graph.open") && errors[0].first().contains("boards.open")
        );
        assert_eq!(keys.label("graph.open"), "g");
        assert_eq!(keys.label("boards.open"), "w");

        // Taking another command's default key is a clash too.
        let (keys, errors) = menu("[keys]\n\"graph.open\" = \"n\"\n");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(keys.get(press('n')), core(MenuAction::NewNote));
        assert_eq!(keys.get(press('g')), core(MenuAction::Graph));
    }

    #[test]
    fn swapping_two_keys_is_not_a_clash() {
        let (keys, errors) = menu("[keys]\n\"graph.open\" = \"n\"\n\"note.new\" = \"g\"\n");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(keys.get(press('n')), core(MenuAction::Graph));
        assert_eq!(keys.get(press('g')), core(MenuAction::NewNote));
    }

    #[test]
    fn same_key_in_two_screens_is_fine() {
        let c = parse("[keys]\n\"graph.open\" = \"y\"\n\"board.fit\" = \"y\"\n");
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        assert_eq!(
            Keymap::with(&commands::entries(BOARD), &c.keys)
                .0
                .label("board.fit"),
            "y"
        );
    }

    #[test]
    fn pane_leave_moves_but_is_never_unbound() {
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        let c = parse("[keys]\n\"pane.leave\" = \"ctrl+g\"\n");
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let keys = Keymap::with(&commands::entries(PANE), &c.keys).0;
        assert_eq!(keys.get(ctrl('g')), Some(PaneAction::Leave));
        assert_eq!(keys.get(ctrl('q')), None, "Ctrl-q goes to the program now");

        let c = parse("[keys]\n\"pane.leave\" = []\n");
        assert_eq!(c.errors.len(), 1, "{:?}", c.errors);
        assert!(c.errors[0].first().contains("pane.leave"), "{:?}", c.errors);
        let keys = Keymap::with(&commands::entries(PANE), &c.keys).0;
        assert_eq!(keys.get(ctrl('q')), Some(PaneAction::Leave));
    }

    #[test]
    fn syntax_error_keeps_every_default() {
        let (keys, errors) = menu("[keys\n\"graph.open\" = \"G\"\n");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(keys.label("graph.open"), "g");
    }

    #[test]
    fn other_sections_are_allowed() {
        let (_, errors) = menu("[colours]\naccent = \"red\"\n");
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn markdown_flavour_built_in_or_custom() {
        let c = parse("[markdown]\nflavour = \"commonmark\"\n");
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        assert_eq!(c.flavour, Flavour::built_in("commonmark"));

        let c = parse(
            "[markdown]\nflavour = \"mine\"\n[markdown.flavours.mine]\nbase = \"github\"\n\
             add = [\"wikilinks\", \"highlights\"]\nremove = [\"footnotes\"]\n",
        );
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let mine = c.flavour.unwrap();
        assert_eq!(mine.name, "mine");
        let mut expected = Flavour::built_in("github").unwrap().syntax;
        expected.extend([Syntax::WikiLinks, Syntax::Highlights]);
        expected.remove(&Syntax::Footnotes);
        assert_eq!(mine.syntax, expected);
        let doc = crate::markdown::parse("==hi== [^1]", &mine);
        assert!(matches!(
            &doc.blocks[0].kind,
            crate::markdown::BlockKind::Paragraph(i)
                if matches!(i[0], crate::markdown::Inline::Highlight(_))
                    && i.contains(&crate::markdown::Inline::Text(" [^1]".into()))
        ));
    }

    #[test]
    fn markdown_mistakes_are_reported_and_fall_back() {
        for (text, needle) in [
            ("[markdown]\nflavour = \"gfm\"\n", "gfm"),
            ("[markdown]\nflavour = 3\n", "flavour"),
            ("[markdown]\nflavor = \"github\"\n", "flavor"),
            (
                "[markdown]\nflavour = \"x\"\n[markdown.flavours.x]\nadd = [\"wikilink\"]\n",
                "wikilink",
            ),
            (
                "[markdown]\nflavour = \"x\"\n[markdown.flavours.x]\nbase = \"x\"\n",
                "base",
            ),
            ("[markdown.flavours.github]\nadd = [\"tags\"]\n", "built-in"),
            (
                "[markdown.flavours.x]\nbase = \"github\"\nextra = 1\n",
                "extra",
            ),
        ] {
            let c = parse(text);
            assert_eq!(c.errors.len(), 1, "{text}: {:?}", c.errors);
            assert!(
                c.errors[0].first().contains(needle),
                "{text}: {:?}",
                c.errors
            );
            assert_eq!(c.flavour, None, "{text}");
        }
        // A shadowing flavour is ignored: `github` stays the built-in one.
        let c = parse(
            "[markdown]\nflavour = \"github\"\n[markdown.flavours.github]\nadd = [\"tags\"]\n",
        );
        assert_eq!(c.flavour, Flavour::built_in("github"));
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let c = read(
            Some(PathBuf::from("/nonexistent/cold-lava/config.toml")),
            &commands::menu([]).0,
        );
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        assert!(c.keys.is_empty());
    }
}
