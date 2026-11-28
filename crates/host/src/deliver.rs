//! What a frontend hands its extensions each frame, and in which order (`Deliveries`): the
//! core's item news to whoever subscribed, `file/changed` to the owners of files that changed,
//! `file/read` and `file/edit` to the owners the core asks, the answers to `items/edit`s that
//! came later, then a poll of every extension. Each step takes what the core has for it when
//! the step begins, so what an extension asks for in one step reaches the steps after it.
//!
//! A frontend calls each extension it is given (`Next::Call`) and applies what that extension
//! asked for before it asks for the next one. A frontend that applies effects only after the
//! whole frame calls `Registry::frame`.

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
pub enum Step {
    /// `items/changed`, `items/reset` and `links/changed` to the subscription's owner.
    News,
    /// `file/changed` to the owner of each file that changed on disk.
    Files,
    /// `file/read`, then `file/edit`, to the owner the core asks.
    Owners,
    /// The answer to an `items/edit` that was answered `Later`, to whoever made it.
    Edited,
    /// Every extension answers what it asked since the last frame.
    Polls,
}

/// A frame's steps in order, after the frontend took the core's events (`Core::poll`).
pub const FRAME: [Step; 5] = [
    Step::News,
    Step::Files,
    Step::Owners,
    Step::Edited,
    Step::Polls,
];

/// What one call to an extension hands it.
#[derive(Debug)]
pub enum Delivery {
    Items(News),
    File(FileNews),
    Read(FileRead),
    Edit(FileEdit),
    Edited(Ticket, Result<Edited, gateway::Error>),
    Poll,
}

impl Delivery {
    pub fn send(self, extension: &mut dyn Extension, host: &mut Host<'_>) {
        match self {
            Delivery::Items(news) => extension.items(news, host),
            Delivery::File(news) => extension.file(news, host),
            Delivery::Read(read) => extension.read_file(read, host),
            Delivery::Edit(edit) => extension.edit_file(edit, host),
            Delivery::Edited(ticket, answer) => extension.edited(ticket, answer, host),
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
        answer: Result<Edited, gateway::Error>,
    },
}

/// The steps of a frame not yet taken, and what the one under way still has to hand out.
pub struct Deliveries {
    steps: VecDeque<Step>,
    changed: Vec<String>,
    waiting: VecDeque<Waiting>,
}

/// Taken from the core, not yet given to anyone: who it is for is looked up as it goes out.
enum Waiting {
    News(String, News),
    File(String),
    Read(String, FileRead),
    Edit(String, FileEdit),
    Edited(String, Ticket, Result<Edited, gateway::Error>),
    Poll(ExtId),
}

impl Deliveries {
    /// `steps` in order, `events` being what the core said since the last frame.
    pub fn new(steps: &[Step], events: &[Event]) -> Deliveries {
        let changed = events.iter().filter_map(|e| match e {
            Event::FileChanged(rel) => Some(rel.clone()),
            _ => None,
        });
        Deliveries {
            steps: steps.iter().copied().collect(),
            changed: changed.collect(),
            waiting: VecDeque::new(),
        }
    }

    /// What to do next, or `None` when the steps are done. `ours` says which clients' item
    /// news the frontend takes itself. Item news nobody holds stops its subscription, a
    /// `file/edit` whose owner isn't loaded is answered that it isn't, and a `file/read` of
    /// one is dropped.
    pub fn next(
        &mut self,
        registry: &Registry,
        core: &mut Core,
        ours: impl Fn(&str) -> bool,
    ) -> Option<Next> {
        loop {
            let Some(waiting) = self.waiting.pop_front() else {
                let step = self.steps.pop_front()?;
                self.take(step, registry, core);
                continue;
            };
            if let Some(next) = route(waiting, registry, core, &ours) {
                return Some(next);
            }
        }
    }

    fn take(&mut self, step: Step, registry: &Registry, core: &mut Core) {
        let waiting = &mut self.waiting;
        match step {
            Step::News => {
                let news = core.take_item_news().into_iter();
                waiting.extend(news.map(|(client, news)| Waiting::News(client, news)));
            }
            Step::Files => waiting.extend(self.changed.drain(..).map(Waiting::File)),
            Step::Owners => {
                let (reads, edits) = (core.take_reads(), core.take_file_edits());
                waiting.extend(reads.into_iter().map(|(o, r)| Waiting::Read(o, r)));
                waiting.extend(edits.into_iter().map(|(o, e)| Waiting::Edit(o, e)));
            }
            Step::Edited => {
                let edited = core.take_edited().into_iter();
                waiting.extend(edited.map(|(by, t, a)| Waiting::Edited(by, t, a)));
            }
            Step::Polls => waiting.extend(registry.ids().map(Waiting::Poll)),
        }
    }
}

impl Registry {
    /// One frame (`FRAME`), for a frontend that applies what the extensions asked
    /// for once it is over: each effect with the extension that asked for it, in order.
    /// `events` are the core's since the last frame. The frontend has no clients of its own
    /// here: item news only extensions hold, and an answer to an `items/edit` none made is
    /// dropped.
    pub fn frame(&mut self, core: &mut Core, events: &[Event]) -> Vec<(ExtId, Effect)> {
        let mut frame = Deliveries::new(&FRAME, events);
        let mut asked = Vec::new();
        while let Some(next) = frame.next(self, core, |_| false) {
            if let Next::Call(ext, delivery) = next {
                let effects = self.call(ext, Some(core), |e, host| delivery.send(e, host));
                asked.extend(effects.into_iter().map(|effect| (ext, effect)));
            }
        }
        asked
    }
}

/// Who `waiting` goes to, or `None` when it was dealt with here.
fn route(
    waiting: Waiting,
    registry: &Registry,
    core: &mut Core,
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
                    core.unsubscribe_items(&client, news.subscription());
                    None
                }
            }
        }
        Waiting::File(path) => {
            let (owner, _) = registry.owner_of(&path)?;
            let rev = read::rev_at(core.root(), &path);
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
                core.file_edited(&owner, &edit.path, &edit.rev, why);
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

        let mut frame = Deliveries::new(&FRAME, &events);
        let mut ours = Vec::new();
        while let Some(next) = frame.next(&registry, &mut core, |c| c == "board") {
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
}
