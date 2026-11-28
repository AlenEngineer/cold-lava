//! What a frontend hands its extensions each frame, and in which order (`Deliveries`): the
//! core's item news to whoever subscribed, `file/changed` to the owners of files that changed,
//! `file/read` and `file/edit` to the owners the core asks, the answers to `items/edit`s that
//! came later, then a poll of every extension. Each step takes what the core has for it when
//! the step begins, so what an extension asks for in one step reaches the steps after it.
//!
//! A frontend calls each extension it is given (`Next::Call`) and applies what that extension
//! asked for before it asks for the next one. A frontend that applies effects only after the
//! whole frame calls `Registry::frame`. With no core (no workspace open, or its folder gone),
//! only the polls run, so extensions are still answered and their crashes still noticed.

use crate::extension::Extension;
use crate::host::{ExtId, Host};
use crate::protocol::{Effect, FileNews};
use crate::registry::Registry;
use cold_lava_engine::core::{Core, Event};
use cold_lava_engine::gateway::{self, Edited, News, Ticket};
use cold_lava_engine::owner::cache::FileRead;
use cold_lava_engine::owner::edits::FileEdit;
use cold_lava_engine::read;
use std::collections::VecDeque;

/// One step of a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step<'a> {
    /// `items/changed`, `items/reset` and `links/changed` to the subscription's owner.
    News,
    /// `file/changed` to the owner of each file these events say changed on disk.
    Files(&'a [Event]),
    /// `file/read`, then `file/edit`, to the owner the core asks.
    Owners,
    /// The answer to an `items/edit` that was answered `Later`, to whoever made it.
    Edited,
    /// Every extension answers what it asked since the last frame.
    Polls,
}

/// A frame's steps in order, `events` being the core's since the last frame (`Core::poll`).
/// `Registry::frame` runs it. The menu doesn't: it polls every frame, and delivers the news
/// and then `after_sync` after each sync with the core, which happens only when the core had
/// something to say (and again inside a frame, when an extension's edit is read back).
pub fn frame(events: &[Event]) -> [Step<'_>; 5] {
    [
        Step::News,
        Step::Files(events),
        Step::Owners,
        Step::Edited,
        Step::Polls,
    ]
}

/// What the menu delivers after a sync with the core has taken `events` and handed out the
/// news: a frame's steps between the news and the polls.
pub fn after_sync(events: &[Event]) -> [Step<'_>; 3] {
    [Step::Files(events), Step::Owners, Step::Edited]
}

/// What one call to an extension hands it.
#[derive(Debug)]
pub enum Delivery {
    Items(News),
    File(FileNews),
    Read(FileRead),
    Edit(FileEdit),
    Edited(Ticket, Box<Result<Edited, gateway::Error>>),
    Poll,
}

impl Delivery {
    pub fn send(self, extension: &mut dyn Extension, host: &mut Host<'_>) {
        match self {
            Delivery::Items(news) => extension.items(news, host),
            Delivery::File(news) => extension.file(news, host),
            Delivery::Read(read) => extension.read_file(read, host),
            Delivery::Edit(edit) => extension.edit_file(edit, host),
            Delivery::Edited(ticket, answer) => extension.edited(ticket, *answer, host),
            Delivery::Poll => extension.poll(host),
        }
    }
}

/// What the frontend does next.
#[derive(Debug)]
pub enum Next {
    /// Call the extension with this, then apply what it asked for.
    Call(ExtId, Delivery),
    /// Item news for one of the frontend's own clients (those `ours` names to `next`).
    News { client: String, news: News },
    /// The answer to an `items/edit` that no extension made: one of the frontend's screens.
    Edited {
        by: String,
        ticket: Ticket,
        answer: Box<Result<Edited, gateway::Error>>,
    },
}

/// The steps of a frame not yet taken, and what the one under way still has to hand out.
pub struct Deliveries<'a> {
    steps: VecDeque<Step<'a>>,
    waiting: VecDeque<Waiting>,
}

/// Taken from the core, not yet given to anyone: who it is for is looked up as it goes out.
enum Waiting {
    News(String, News),
    File(String),
    Read(String, FileRead),
    Edit(String, FileEdit),
    Edited(String, Ticket, Box<Result<Edited, gateway::Error>>),
    Poll(ExtId),
}

impl<'a> Deliveries<'a> {
    /// `steps` in order.
    pub fn new(steps: &[Step<'a>]) -> Deliveries<'a> {
        Deliveries {
            steps: steps.iter().copied().collect(),
            waiting: VecDeque::new(),
        }
    }

    /// What to do next, or `None` when the steps are done. `ours` says which clients' item
    /// news the frontend takes itself. Item news nobody holds stops its subscription, a
    /// `file/edit` whose owner isn't loaded is answered that it isn't, and a `file/read` of
    /// one is dropped. With no `core`, only `Step::Polls` hands anything out.
    pub fn next(
        &mut self,
        registry: &Registry,
        mut core: Option<&mut Core>,
        ours: impl Fn(&str) -> bool,
    ) -> Option<Next> {
        loop {
            let Some(waiting) = self.waiting.pop_front() else {
                let step = self.steps.pop_front()?;
                self.take(step, registry, core.as_deref_mut());
                continue;
            };
            if let Some(next) = route(waiting, registry, core.as_deref_mut(), &ours) {
                return Some(next);
            }
        }
    }

    fn take(&mut self, step: Step<'a>, registry: &Registry, core: Option<&mut Core>) {
        let waiting = &mut self.waiting;
        if step == Step::Polls {
            waiting.extend(registry.ids().map(Waiting::Poll));
            return;
        }
        let Some(core) = core else {
            return;
        };
        match step {
            Step::News => {
                let news = core.take_item_news().into_iter();
                waiting.extend(news.map(|(client, news)| Waiting::News(client, news)));
            }
            Step::Files(events) => waiting.extend(events.iter().filter_map(|e| match e {
                Event::FileChanged(rel) => Some(Waiting::File(rel.clone())),
                _ => None,
            })),
            Step::Owners => {
                let (reads, edits) = (core.take_reads(), core.take_file_edits());
                waiting.extend(reads.into_iter().map(|(o, r)| Waiting::Read(o, r)));
                waiting.extend(edits.into_iter().map(|(o, e)| Waiting::Edit(o, e)));
            }
            Step::Edited => {
                let edited = core.take_edited().into_iter();
                waiting.extend(edited.map(|(by, t, a)| Waiting::Edited(by, t, Box::new(a))));
            }
            Step::Polls => {}
        }
    }
}

impl Registry {
    /// One frame (`frame`), for a frontend that applies what the extensions asked
    /// for once it is over: each effect with the extension that asked for it, in order.
    /// `events` are the core's since the last frame. The frontend has no clients of its own
    /// here: item news only extensions hold, and an answer to an `items/edit` none made is
    /// dropped.
    pub fn frame(&mut self, core: &mut Core, events: &[Event]) -> Vec<(ExtId, Effect)> {
        let mut steps = Deliveries::new(&frame(events));
        let mut asked = Vec::new();
        while let Some(next) = steps.next(self, Some(&mut *core), |_| false) {
            if let Next::Call(ext, delivery) = next {
                let effects = self.call(ext, Some(core), |e, host| delivery.send(e, host));
                asked.extend(effects.into_iter().map(|effect| (ext, effect)));
            }
        }
        asked
    }
}

/// Who `waiting` goes to, or `None` when it was dealt with here. What came from a core is
/// dropped when there is no core any more.
fn route(
    waiting: Waiting,
    registry: &Registry,
    core: Option<&mut Core>,
    ours: &impl Fn(&str) -> bool,
) -> Option<Next> {
    match waiting {
        Waiting::News(client, news) => {
            if ours(&client) {
                return Some(Next::News { client, news });
            }
            match registry.by_name(&client) {
                Some(ext) => Some(Next::Call(ext, Delivery::Items(news))),
                None => {
                    core?.unsubscribe_items(&client, news.subscription());
                    None
                }
            }
        }
        Waiting::File(path) => {
            let (owner, _) = registry.owner_of(&path)?;
            let rev = read::rev_at(core?.root(), &path);
            Some(Next::Call(
                owner,
                Delivery::File(FileNews::Changed { path, rev }),
            ))
        }
        Waiting::Read(owner, read) => {
            Some(Next::Call(registry.by_name(&owner)?, Delivery::Read(read)))
        }
        Waiting::Edit(owner, edit) => match registry.by_name(&owner) {
            Some(ext) => Some(Next::Call(ext, Delivery::Edit(edit))),
            None => {
                let why = Err(format!("{owner} isn't loaded"));
                core?.file_edited(&owner, &edit.path, &edit.rev, why);
                None
            }
        },
        Waiting::Edited(by, ticket, answer) => match registry.by_name(&by) {
            Some(ext) => Some(Next::Call(ext, Delivery::Edited(ticket, answer))),
            None => Some(Next::Edited { by, ticket, answer }),
        },
        Waiting::Poll(ext) => Some(Next::Call(ext, Delivery::Poll)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Manifest;
    use crate::protocol::{EditorRequest, PaneNews};
    use cold_lava_engine::gateway::Select;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// An extension that writes down what it was handed, in order.
    struct Heard {
        name: &'static str,
        heard: Rc<RefCell<Vec<String>>>,
    }

    impl Extension for Heard {
        fn initialize(&mut self) -> Manifest {
            Manifest {
                name: self.name.into(),
                title: self.name.into(),
                version: String::new(),
                description: String::new(),
                roles: Vec::new(),
                hints: Vec::new(),
                commands: Vec::new(),
                types: Vec::new(),
                shapes: Vec::new(),
                permissions: Vec::new(),
                file_types: Vec::new(),
                shows: Vec::new(),
            }
        }
        fn pane(&mut self, _: PaneNews, _: &mut Host<'_>) {}
        fn editor(&mut self, _: EditorRequest, _: &mut Host<'_>) {}
        fn items(&mut self, _: News, _: &mut Host<'_>) {
            self.heard.borrow_mut().push(format!("{} items", self.name));
        }
        fn poll(&mut self, _: &mut Host<'_>) {
            self.heard.borrow_mut().push(format!("{} poll", self.name));
        }
    }

    fn workspace(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cold-lava-deliver-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
        std::fs::write(dir.join("Plan.md"), "- [ ] Pay rent\n").unwrap();
        dir
    }

    fn tasks() -> Select {
        Select {
            kind: "task".into(),
            ..Select::default()
        }
    }

    #[test]
    fn a_frame_hands_news_to_its_subscribers_before_it_polls_them() {
        let dir = workspace("news");
        let mut core = Core::open(&dir, Default::default()).unwrap();
        let heard = Rc::new(RefCell::new(Vec::new()));
        let fakes = ["one", "two"].map(|name| {
            let heard = Rc::clone(&heard);
            Box::new(Heard { name, heard }) as Box<dyn Extension>
        });
        let mut registry = Registry::new(fakes.into());
        core.subscribe_items("two", tasks()).unwrap();
        let gone = core.subscribe_items("gone", tasks()).unwrap();
        core.subscribe_items("board", tasks()).unwrap();
        std::fs::write(dir.join("Plan.md"), "- [ ] Pay rent\n- [ ] Call\n").unwrap();
        let events = core.note_changed_now(&dir.join("Plan.md"));

        let mut steps = Deliveries::new(&frame(&events));
        let mut ours = Vec::new();
        while let Some(next) = steps.next(&registry, Some(&mut core), |c| c == "board") {
            match next {
                Next::Call(ext, d) => {
                    drop(registry.call(ext, Some(&mut core), |e, h| d.send(e, h)))
                }
                Next::News { client, .. } => ours.push(client),
                Next::Edited { by, .. } => panic!("no edit was made, but {by} heard of one"),
            }
        }
        assert_eq!(*heard.borrow(), ["two items", "one poll", "two poll"]);
        assert_eq!(
            ours,
            ["board"],
            "the frontend's own client is the frontend's"
        );
        assert!(
            !core.unsubscribe_items("gone", gone),
            "nobody held it, so it stopped"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_frame_with_no_core_still_polls_every_extension() {
        let heard = Rc::new(RefCell::new(Vec::new()));
        let fakes = ["one", "two"].map(|name| {
            let heard = Rc::clone(&heard);
            Box::new(Heard { name, heard }) as Box<dyn Extension>
        });
        let mut registry = Registry::new(fakes.into());
        let mut steps = Deliveries::new(&frame(&[]));
        while let Some(next) = steps.next(&registry, None, |_| false) {
            let Next::Call(ext, d) = next else {
                panic!("with no core, only extensions are called");
            };
            drop(registry.call(ext, None, |e, h| d.send(e, h)));
        }
        assert_eq!(*heard.borrow(), ["one poll", "two poll"]);
    }

    #[test]
    fn a_frame_is_the_news_then_what_follows_a_sync_then_the_polls() {
        let events = [Event::FileChanged("Rows.csv".into())];
        let mut menu = vec![Step::News];
        menu.extend(after_sync(&events));
        menu.push(Step::Polls);
        assert_eq!(frame(&events).to_vec(), menu);
    }
}
