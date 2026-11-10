//! Back and forward across the app, like a browser's: the places you went to (a note in the
//! reader or the editor, a search, the graph, a board) and the ones you came back from.

use std::path::{Path, PathBuf};

use crate::commands::Key;
use crate::reader::Spot;

/// How far back the history goes.
const LIMIT: usize = 100;

#[derive(Clone, Debug, PartialEq)]
pub enum Place {
    /// A note in the right pane; `spot` is where the reader was, when it showed the note.
    Note {
        path: PathBuf,
        spot: Option<Spot>,
    },
    Search {
        query: String,
        sel: usize,
    },
    /// The graph view and the key of its selected node.
    Graph {
        selected: Option<String>,
    },
    Board {
        path: PathBuf,
    },
    /// A view an extension opened (`ext::view`), which keeps how it was while it is open;
    /// `label` is the extension's name.
    View {
        owner: crate::ext::ExtId,
        view: crate::ext::view::ViewId,
        label: String,
    },
}

impl Place {
    /// What the status line calls it: "⟵ Lava".
    pub fn label(&self) -> String {
        let stem = |p: &Path| {
            p.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        match self {
            Place::Note { path, .. } | Place::Board { path } => stem(path),
            Place::Search { query, .. } => format!("search “{query}”"),
            Place::Graph { .. } => "graph".to_string(),
            Place::View { label, .. } => label.clone(),
        }
    }
}

#[derive(Default)]
pub struct History {
    back: Vec<Place>,
    forward: Vec<Place>,
}

impl History {
    /// Going somewhere new from `from`: it becomes the way back, and what was ahead is dropped.
    pub fn visit(&mut self, from: Place) {
        self.forward.clear();
        if self.back.last() != Some(&from) {
            self.back.push(from);
        }
        if self.back.len() > LIMIT {
            self.back.remove(0);
        }
    }

    /// The place before `here`, which becomes the way forward. Places equal to `here` are
    /// skipped: the editor can return to a note on its own, without asking.
    pub fn back(&mut self, here: Option<Place>) -> Option<Place> {
        step(&mut self.back, &mut self.forward, here)
    }

    /// Undoes a `back`.
    pub fn forward(&mut self, here: Option<Place>) -> Option<Place> {
        step(&mut self.forward, &mut self.back, here)
    }

    pub fn peek_back(&self) -> Option<&Place> {
        self.back.last()
    }

    pub fn peek_forward(&self) -> Option<&Place> {
        self.forward.last()
    }

    /// Drop the places that show the note or board at `path`, or anything in the folder at
    /// `path` (it was deleted).
    pub fn forget(&mut self, path: &Path) {
        let keep = |p: &Place| !matches!(p, Place::Note { path: q, .. } | Place::Board { path: q } if q.starts_with(path));
        self.back.retain(keep);
        self.forward.retain(keep);
    }

    /// The note, board or folder at `from` is at `to` now: so are the places that show it.
    pub fn moved(&mut self, from: &Path, to: &Path) {
        for place in self.back.iter_mut().chain(self.forward.iter_mut()) {
            if let Place::Note { path, .. } | Place::Board { path } = place
                && let Ok(rest) = path.strip_prefix(from)
            {
                *path = match rest.as_os_str().is_empty() {
                    true => to.to_path_buf(),
                    false => to.join(rest),
                };
            }
        }
    }

    /// Drop the places `keep` doesn't keep (views that closed).
    pub fn retain(&mut self, keep: impl Fn(&Place) -> bool) {
        self.back.retain(&keep);
        self.forward.retain(&keep);
    }

    pub fn clear(&mut self) {
        self.back.clear();
        self.forward.clear();
    }
}

fn step(from: &mut Vec<Place>, to: &mut Vec<Place>, here: Option<Place>) -> Option<Place> {
    while from.last().is_some_and(|p| Some(p) == here.as_ref()) {
        from.pop();
    }
    let next = from.pop()?;
    to.extend(here);
    Some(next)
}

/// Where Back and Forward lead from a screen of its own (the graph, a board).
#[derive(Clone, Default)]
pub struct Nav {
    pub back: Option<String>,
    pub forward: Option<String>,
}

impl Nav {
    pub fn of(history: &History) -> Nav {
        Nav {
            back: history.peek_back().map(Place::label),
            forward: history.peek_forward().map(Place::label),
        }
    }

    /// "Ctrl-o ⟵ Lava · Alt-→ ⟶ Kīlauea" with the commands' first keys, or "" with no history.
    pub fn hint(&self, back_key: Option<&Key>, forward_key: Option<&Key>) -> String {
        let way = |to: &Option<String>, key: Option<&Key>, arrow: &str| {
            let key = key.map(|k| format!("{k} ")).unwrap_or_default();
            to.as_ref().map(|to| format!("{key}{arrow} {to}"))
        };
        let back = way(&self.back, back_key, "⟵");
        let forward = way(&self.forward, forward_key, "⟶");
        let parts: Vec<String> = back.into_iter().chain(forward).collect();
        parts.join(" · ")
    }
}

/// How a board was left.
#[derive(Clone, Debug, PartialEq)]
pub enum Exit {
    Quit,
    Back,
    Forward,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(name: &str) -> Place {
        Place::Note {
            path: PathBuf::from(format!("{name}.md")),
            spot: None,
        }
    }

    #[test]
    fn back_and_forward_retrace_the_way() {
        let mut h = History::default();
        h.visit(note("Kīlauea"));
        h.visit(note("Shield Volcano"));
        assert_eq!(h.back(Some(note("Lava"))), Some(note("Shield Volcano")));
        assert_eq!(h.back(Some(note("Shield Volcano"))), Some(note("Kīlauea")));
        assert_eq!(h.back(Some(note("Kīlauea"))), None);
        assert_eq!(
            h.forward(Some(note("Kīlauea"))),
            Some(note("Shield Volcano"))
        );
        assert_eq!(h.peek_forward(), Some(&note("Lava")));
    }

    #[test]
    fn going_somewhere_new_drops_the_way_forward() {
        let mut h = History::default();
        h.visit(note("Kīlauea"));
        h.back(Some(note("Lava")));
        assert!(h.peek_forward().is_some());
        h.visit(note("Kīlauea"));
        assert_eq!(h.forward(Some(note("Vesuvius"))), None);
    }

    #[test]
    fn back_skips_where_you_already_are() {
        let mut h = History::default();
        h.visit(note("Kīlauea"));
        h.visit(note("Lava"));
        assert_eq!(h.back(Some(note("Lava"))), Some(note("Kīlauea")));
    }

    #[test]
    fn follows_a_moved_folder_and_forgets_a_deleted_one() {
        let mut h = History::default();
        h.visit(note("People/Johnston"));
        h.visit(note("Lava"));
        h.moved(Path::new("People"), Path::new("Volcanologists"));
        assert_eq!(h.back(None), Some(note("Lava")));
        assert_eq!(h.back(None), Some(note("Volcanologists/Johnston")));

        let mut h = History::default();
        h.visit(note("Volcanologists/Johnston"));
        h.visit(note("Lava"));
        h.forget(Path::new("Volcanologists"));
        assert_eq!(h.back(None), Some(note("Lava")));
        assert_eq!(h.back(None), None);
    }

    #[test]
    fn forgets_a_deleted_note_both_ways() {
        let mut h = History::default();
        h.visit(note("Kīlauea"));
        h.visit(note("Lava"));
        h.visit(note("Shield Volcano"));
        h.back(Some(note("Lava")));
        h.forget(Path::new("Lava.md"));
        assert_eq!(h.back(Some(note("Shield Volcano"))), Some(note("Kīlauea")));
        assert_eq!(h.forward(None), Some(note("Shield Volcano")));
        assert_eq!(h.forward(None), None);
    }

    #[test]
    fn keeps_the_last_hundred() {
        let mut h = History::default();
        for i in 0..150 {
            h.visit(note(&i.to_string()));
        }
        let mut n = 0;
        while h.back(None).is_some() {
            n += 1;
        }
        assert_eq!(n, LIMIT);
    }

    #[test]
    fn hint_names_both_ways() {
        let mut h = History::default();
        let ctrl_o = Key::parse("ctrl+o");
        assert_eq!(Nav::of(&h).hint(ctrl_o.as_ref(), None), "");
        h.visit(note("Kīlauea"));
        h.visit(note("Shield Volcano"));
        h.back(Some(note("Lava")));
        let hint = Nav::of(&h).hint(ctrl_o.as_ref(), None);
        assert_eq!(hint, "Ctrl-o ⟵ Kīlauea · ⟶ Lava");
    }
}
