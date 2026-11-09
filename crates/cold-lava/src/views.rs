//! The views extensions opened through the view gateway (`ext::view`), as the menu keeps them.
//! Each is drawn by the list or table view and keeps its selection, folds, sort and filter
//! while it is out of sight, so Back returns to it as it was. Updates wait for the frame and
//! only the last one is drawn; an extension has at most `PER_EXTENSION` views open.

use ratatui::crossterm::event::KeyEvent;
use ratatui::text::Line;

use crate::ext::ExtId;
use crate::ext::view::{Content, Event, ViewId};
use crate::list::{self, Filter, ListView};
use crate::table::{self, TableView};

/// Opening one more closes the extension's oldest.
pub const PER_EXTENSION: usize = 8;

#[derive(Default)]
pub struct Views {
    /// Oldest first.
    open: Vec<Open>,
}

pub struct Open {
    pub owner: ExtId,
    pub id: ViewId,
    drawn: Drawn,
    /// The last update since the frame was drawn, and the row it selects.
    pending: Option<(Content, Option<String>)>,
}

enum Drawn {
    List(ListView),
    Table(TableView),
}

/// What the menu should do after a key in a view.
#[derive(Debug, PartialEq)]
pub enum Action {
    Stay,
    Close,
    Say(String),
    Send(Event),
}

impl Drawn {
    fn new(content: Content) -> Drawn {
        match content {
            Content::List(l) => Drawn::List(ListView::new(l)),
            Content::Table(t) => Drawn::Table(TableView::new(t)),
        }
    }

    fn update(&mut self, content: Content, select: Option<String>) {
        match (&mut *self, content) {
            (Drawn::List(v), Content::List(l)) => v.update(l),
            (Drawn::Table(v), Content::Table(t)) => v.update(t),
            (_, other) => *self = Drawn::new(other),
        }
        self.select(select);
    }

    fn select(&mut self, select: Option<String>) {
        match (self, select) {
            (Drawn::List(v), Some(id)) => v.select(&id),
            (Drawn::Table(v), Some(id)) => v.select(&id),
            (_, None) => {}
        }
    }
}

impl Views {
    /// Show `content` as `id`. Returns the owner's oldest view when it was closed to make room.
    pub fn open(
        &mut self,
        owner: ExtId,
        id: ViewId,
        content: Content,
        select: Option<String>,
    ) -> Option<ViewId> {
        let mine = self.open.iter().filter(|v| v.owner == owner).count();
        let closed = (mine >= PER_EXTENSION)
            .then(|| self.open.iter().position(|v| v.owner == owner))
            .flatten()
            .map(|i| self.open.remove(i).id);
        let mut drawn = Drawn::new(content);
        drawn.select(select);
        self.open.push(Open {
            owner,
            id,
            drawn,
            pending: None,
        });
        closed
    }

    /// The owner's view `id` shows `content` from the next frame. Whether it has such a view.
    pub fn update(
        &mut self,
        owner: ExtId,
        id: ViewId,
        content: Content,
        select: Option<String>,
    ) -> bool {
        let Some(v) = self.get_mut(owner, id) else {
            return false;
        };
        let select = select.or_else(|| v.pending.take().and_then(|(_, s)| s));
        v.pending = Some((content, select));
        true
    }

    /// Whether the owner had view `id`.
    pub fn close(&mut self, owner: ExtId, id: ViewId) -> bool {
        let before = self.open.len();
        self.open.retain(|v| (v.owner, v.id) != (owner, id));
        self.open.len() != before
    }

    /// Every view of extensions that `keep` doesn't keep is gone (it stopped).
    pub fn retain_owners(&mut self, keep: impl Fn(ExtId) -> bool) {
        self.open.retain(|v| keep(v.owner));
    }

    pub fn has(&self, owner: ExtId, id: ViewId) -> bool {
        self.open.iter().any(|v| (v.owner, v.id) == (owner, id))
    }

    pub fn get_mut(&mut self, owner: ExtId, id: ViewId) -> Option<&mut Open> {
        self.open
            .iter_mut()
            .find(|v| (v.owner, v.id) == (owner, id))
    }

    /// Before drawing: each view takes its last update.
    pub fn settle(&mut self) {
        for v in &mut self.open {
            if let Some((content, select)) = v.pending.take() {
                v.drawn.update(content, select);
            }
        }
    }
}

impl Open {
    pub fn title(&self) -> &str {
        match &self.drawn {
            Drawn::List(v) => v.title(),
            Drawn::Table(v) => v.title(),
        }
    }

    /// Whether the view takes `k` as text: a filter or a cell being typed, or a character in
    /// a view that filters as you type.
    pub fn takes(&self, k: &KeyEvent) -> bool {
        match &self.drawn {
            Drawn::List(v) => v.typing() || v.filters().takes(k),
            Drawn::Table(v) => v.typing() || v.filters().takes(k),
        }
    }

    pub fn key(&mut self, k: KeyEvent) -> Action {
        match &mut self.drawn {
            Drawn::List(v) => match v.key(k) {
                list::Action::Stay => Action::Stay,
                list::Action::Close => Action::Close,
                list::Action::Send(e) => Action::Send(e.into()),
            },
            Drawn::Table(v) => match v.key(k) {
                table::Action::Stay => Action::Stay,
                table::Action::Close => Action::Close,
                table::Action::Say(s) => Action::Say(s),
                table::Action::Send(e) => Action::Send(e.into()),
            },
        }
    }

    /// The keys, for the bottom of its frame.
    pub fn hint(&self) -> String {
        match &self.drawn {
            Drawn::List(v) if v.filters() == Filter::Typing => {
                " ↑↓ move · ←→ fold · type to filter · ⏎ open · Esc close ".into()
            }
            Drawn::List(v) => {
                let keys: String = (v.keys().iter())
                    .map(|k| format!(" · {} {}", k.key, k.title))
                    .collect();
                format!(" ↑↓ move · x tick · ⏎ open · h/l fold · / filter{keys} · Esc close ")
            }
            Drawn::Table(v) if v.filters() == Filter::Typing => {
                " ←→↑↓ move · type to filter · ⏎ open · Esc close ".into()
            }
            Drawn::Table(_) => {
                " ←→↑↓ move · s/S sort · / filter · e edit · ⏎ open · Esc close ".into()
            }
        }
    }

    pub fn lines(&mut self, width: usize, height: usize) -> Vec<Line<'static>> {
        match &mut self.drawn {
            Drawn::List(v) => v.lines(width, height),
            Drawn::Table(v) => v.lines(width, height),
        }
    }

    pub fn status(&self) -> Line<'static> {
        match &self.drawn {
            Drawn::List(v) => v.status(),
            Drawn::Table(v) => v.status(),
        }
    }

    #[cfg(test)]
    fn selected(&self) -> Option<String> {
        match &self.drawn {
            Drawn::List(v) => v.selected().map(|r| r.id.clone()),
            Drawn::Table(v) => v.selected().map(|r| r.id.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list::{Group, List, Piece, Row};
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};

    fn list(title: &str, rows: &[&str]) -> Content {
        Content::List(List {
            title: title.into(),
            groups: vec![Group {
                id: "g".into(),
                title: "Group".into(),
                collapsed: false,
                rows: (rows.iter())
                    .map(|id| Row {
                        id: id.to_string(),
                        text: vec![Piece::plain(*id)],
                        check: Some(list::Check::Unchecked),
                        right: Vec::new(),
                        depth: 0,
                        look: None,
                    })
                    .collect(),
            }],
            keys: Vec::new(),
            empty: None,
            filter: Default::default(),
        })
    }

    fn press(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    const A: ExtId = ExtId(0);
    const B: ExtId = ExtId(1);

    #[test]
    fn updates_wait_for_the_frame_and_the_last_one_wins() {
        let mut views = Views::default();
        let id = ViewId(1);
        views.open(A, id, list("First", &["r1", "r2"]), Some("r2".into()));
        let v = views.get_mut(A, id).unwrap();
        assert_eq!(v.selected().as_deref(), Some("r2"), "opened at a row");
        v.key(press(KeyCode::Up));
        v.key(press(KeyCode::Down));
        assert_eq!(v.selected().as_deref(), Some("r2"));

        assert!(
            !views.update(B, id, list("Not B's", &[]), None),
            "only its owner's"
        );
        assert!(views.update(A, id, list("Second", &["r1", "r2"]), Some("r1".into())));
        assert!(views.update(A, id, list("Third", &["r0", "r1", "r2"]), None));
        assert_eq!(
            views.get_mut(A, id).unwrap().title(),
            "First",
            "until the frame"
        );
        views.settle();
        let v = views.get_mut(A, id).unwrap();
        assert_eq!(v.title(), "Third");
        assert_eq!(
            v.selected().as_deref(),
            Some("r1"),
            "an earlier update's select carries over"
        );
        views.update(A, id, list("Fourth", &["r0", "r1", "r2"]), None);
        views.settle();
        let v = views.get_mut(A, id).unwrap();
        assert_eq!(v.selected().as_deref(), Some("r1"), "kept by id");
    }

    #[test]
    fn keys_become_events_by_row_id_and_esc_closes() {
        let mut views = Views::default();
        views.open(A, ViewId(1), list("L", &["r1"]), None);
        let v = views.get_mut(A, ViewId(1)).unwrap();
        v.key(press(KeyCode::Down));
        let toggled = Event::Toggled { row: "r1".into() };
        assert_eq!(v.key(press(KeyCode::Char('x'))), Action::Send(toggled));
        let activated = Event::Activated { row: "r1".into() };
        assert_eq!(v.key(press(KeyCode::Enter)), Action::Send(activated));
        assert_eq!(v.key(press(KeyCode::Esc)), Action::Close);
        assert_eq!(v.key(press(KeyCode::Char('q'))), Action::Close);
        assert!(views.close(A, ViewId(1)));
        assert!(!views.close(A, ViewId(1)));
        assert!(!views.has(A, ViewId(1)));
    }

    #[test]
    fn a_view_that_filters_as_you_type_takes_characters_but_not_back() {
        let mut views = Views::default();
        let Content::List(mut typing) = list("L", &["r1"]) else {
            unreachable!()
        };
        typing.filter = Filter::Typing;
        views.open(A, ViewId(1), Content::List(typing), None);
        views.open(A, ViewId(2), list("M", &["r1"]), None);
        let v = views.get_mut(A, ViewId(1)).unwrap();
        assert!(v.takes(&press(KeyCode::Char('q'))));
        let ctrl_o = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);
        assert!(!v.takes(&ctrl_o), "Ctrl-o still goes back");
        assert!(!v.takes(&press(KeyCode::Enter)));
        assert!(v.hint().contains("type to filter"), "{}", v.hint());
        let v = views.get_mut(A, ViewId(2)).unwrap();
        assert!(
            !v.takes(&press(KeyCode::Char('q'))),
            "q closes a view filtered with /"
        );
    }

    #[test]
    fn an_extension_has_a_bounded_number_of_views() {
        let mut views = Views::default();
        views.open(B, ViewId(100), list("B", &[]), None);
        for i in 0..PER_EXTENSION as u32 {
            assert_eq!(views.open(A, ViewId(i), list("A", &[]), None), None);
        }
        let n = PER_EXTENSION as u32;
        assert_eq!(
            views.open(A, ViewId(n), list("A", &[]), None),
            Some(ViewId(0))
        );
        assert!(views.has(B, ViewId(100)), "another's stay");
        views.retain_owners(|e| e != A);
        assert!(!views.has(A, ViewId(n)));
        assert!(views.has(B, ViewId(100)));
    }
}
