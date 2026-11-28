//! The Obsidian Tasks plugin's settings (`.obsidian/plugins/obsidian-tasks-plugin/data.json`):
//! the global filter, what each status symbol means and where its toggle goes, "set done date",
//! "next recurrence on the line below", the task format. Items are read with them
//! (`items::Settings`) and `tasks.rs` ticks with them. A leaf, so `items` and `tasks` don't
//! depend on each other through them.

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

pub struct TasksSettings {
    pub global_filter: String,
    pub set_done_date: bool,
    pub set_cancelled_date: bool,
    pub recurrence_below: bool,
    pub dataview: bool,
    /// Whether the plugin's settings name a `taskFormat` (else `dataview` is its default).
    pub format_set: bool,
    /// What each status symbol means: the defaults, then the plugin's `statusSettings`.
    pub statuses: BTreeMap<char, StatusType>,
    /// The symbol each status's toggle goes to, where the plugin's `statusSettings` say.
    pub next: BTreeMap<char, char>,
}

impl TasksSettings {
    pub fn load(root: &Path) -> TasksSettings {
        let json: serde_json::Value =
            std::fs::read_to_string(root.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"))
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default();
        TasksSettings::from_json(&json)
    }

    pub fn from_json(json: &serde_json::Value) -> TasksSettings {
        let mut statuses: BTreeMap<char, StatusType> = [
            (' ', StatusType::Todo),
            ('x', StatusType::Done),
            ('X', StatusType::Done),
            ('-', StatusType::Cancelled),
            ('/', StatusType::InProgress),
        ]
        .into();
        let mut next = BTreeMap::new();
        let one = |v: Option<&serde_json::Value>| {
            let mut chars = v?.as_str()?.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(c),
                _ => None,
            }
        };
        let configured = ["coreStatuses", "customStatuses"]
            .iter()
            .filter_map(|k| json.pointer(&format!("/statusSettings/{k}"))?.as_array())
            .flatten();
        for s in configured {
            let kind = s.get("type").and_then(|v| v.as_str());
            if let (Some(c), Some(kind)) = (one(s.get("symbol")), kind)
                && let Some(t) = StatusType::from_plugin(kind)
            {
                statuses.insert(c, t);
                if let Some(n) = one(s.get("nextStatusSymbol")) {
                    next.insert(c, n);
                }
            }
        }
        TasksSettings {
            global_filter: json
                .get("globalFilter")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string(),
            set_done_date: json
                .get("setDoneDate")
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
            set_cancelled_date: json
                .get("setCancelledDate")
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
            recurrence_below: json
                .get("recurrenceOnNextLine")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            dataview: json.get("taskFormat").and_then(|v| v.as_str()) == Some("dataview"),
            format_set: json.get("taskFormat").is_some_and(|v| v.is_string()),
            statuses,
            next,
        }
    }

    /// A symbol the settings don't name is not a task status Tasks would act on.
    pub fn status_type(&self, symbol: char) -> StatusType {
        self.statuses
            .get(&symbol)
            .copied()
            .unwrap_or(StatusType::NonTask)
    }

    /// The symbol the workspace writes for a status of this type: the plugin's own (` `, `x`,
    /// `/`, `-`) while the settings still give it that type, else the first that has it. What
    /// a type reads for the `checkbox` of a piece an owner gave (`owner::given::CHECKBOX`).
    pub fn symbol_of(&self, status: StatusType) -> char {
        let own = match status {
            StatusType::Todo | StatusType::NonTask => ' ',
            StatusType::Done => 'x',
            StatusType::InProgress => '/',
            StatusType::Cancelled => '-',
        };
        if self.status_type(own) == status {
            return own;
        }
        let first = self.statuses.iter().find(|(_, t)| **t == status);
        first.map_or(own, |(symbol, _)| *symbol)
    }

    /// The symbol a toggle sets: the status's next one in the plugin's settings, else done for
    /// an open task and todo for a closed one.
    pub fn next_symbol(&self, symbol: char) -> char {
        match self.next.get(&symbol) {
            Some(&n) => n,
            None if matches!(
                self.status_type(symbol),
                StatusType::Done | StatusType::Cancelled
            ) =>
            {
                ' '
            }
            None => 'x',
        }
    }
}

/// The Tasks plugin's status types.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StatusType {
    Todo,
    InProgress,
    Done,
    Cancelled,
    NonTask,
}

impl StatusType {
    fn from_plugin(name: &str) -> Option<StatusType> {
        Some(match name {
            "TODO" => StatusType::Todo,
            "IN_PROGRESS" => StatusType::InProgress,
            "DONE" => StatusType::Done,
            "CANCELLED" => StatusType::Cancelled,
            "NON_TASK" => StatusType::NonTask,
            _ => return None,
        })
    }

    /// Its name in a task item's `status_type`.
    pub fn name(self) -> &'static str {
        match self {
            StatusType::Todo => "todo",
            StatusType::InProgress => "in_progress",
            StatusType::Done => "done",
            StatusType::Cancelled => "cancelled",
            StatusType::NonTask => "non_task",
        }
    }

    /// Every status type.
    pub const ALL: [StatusType; 5] = [
        StatusType::Todo,
        StatusType::InProgress,
        StatusType::Done,
        StatusType::Cancelled,
        StatusType::NonTask,
    ];

    /// The status type whose `name` is `name`.
    pub fn named(name: &str) -> Option<StatusType> {
        StatusType::ALL.into_iter().find(|t| t.name() == name)
    }

    /// The name `agenda --json` and `task toggle --json` have always used.
    pub fn agenda_name(self) -> &'static str {
        match self {
            StatusType::NonTask => "other",
            t => t.name(),
        }
    }
}
