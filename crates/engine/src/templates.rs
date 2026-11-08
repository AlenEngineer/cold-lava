//! Note templates, following the workspace's Obsidian "Templates" settings
//! (`.obsidian/templates.json`: folder, date and time formats) and Templater's folder.

use crate::daily::format_datetime;
use anyhow::{Context, Result};
use chrono::{Local, NaiveDateTime};
use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;

pub struct Settings {
    /// Workspace-relative folders holding templates, without slashes at either end. Empty when the
    /// workspace has no templates set up.
    pub folders: Vec<String>,
    /// moment.js formats for `{{date}}` and `{{time}}`.
    pub date_format: String,
    pub time_format: String,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            folders: Vec::new(),
            date_format: "YYYY-MM-DD".into(),
            time_format: "HH:mm".into(),
        }
    }
}

impl Settings {
    pub fn load(root: &Path) -> Settings {
        let json = |rel: &str| -> serde_json::Value {
            std::fs::read_to_string(root.join(rel))
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default()
        };
        let core = json(".obsidian/templates.json");
        let templater = json(".obsidian/plugins/templater-obsidian/data.json");
        let get = |v: &serde_json::Value, k: &str| -> Option<String> {
            v.get(k)
                .and_then(|v| v.as_str())
                .map(|s| s.trim().trim_matches('/').to_string())
                .filter(|s| !s.is_empty())
        };
        let mut folders = Vec::new();
        for folder in [get(&core, "folder"), get(&templater, "templates_folder")]
            .into_iter()
            .flatten()
        {
            if !folders.contains(&folder) {
                folders.push(folder);
            }
        }
        let default = Settings::default();
        Settings {
            folders,
            date_format: get(&core, "dateFormat").unwrap_or(default.date_format),
            time_format: get(&core, "timeFormat").unwrap_or(default.time_format),
        }
    }

    /// Is the note at workspace-relative `rel` (with or without `.md`) a template?
    pub fn is_template(&self, rel: &str) -> bool {
        self.folders.iter().any(|f| {
            rel.strip_prefix(f.as_str())
                .is_some_and(|r| r.starts_with('/'))
        })
    }

    /// A template's name as the picker shows it: its path inside its templates folder.
    pub fn name<'a>(&self, rel: &'a str) -> &'a str {
        self.folders
            .iter()
            .find_map(|f| rel.strip_prefix(f.as_str())?.strip_prefix('/'))
            .unwrap_or(rel)
    }

    /// The text of a new note called `title`, made at `now` from the template `text`.
    pub fn fill(&self, text: &str, title: &str, now: NaiveDateTime) -> String {
        fill(text, title, now, &self.date_format, &self.time_format)
    }
}

/// The text of a new note called `title`, from the template at workspace-relative `template`
/// (without `.md`), filled now.
pub fn new_note(root: &Path, template: &str, title: &str) -> Result<String> {
    let rel = format!("{template}.md");
    let text = std::fs::read_to_string(root.join(&rel))
        .with_context(|| format!("reading the template {rel}"))?;
    Ok(Settings::load(root).fill(&text, title, Local::now().naive_local()))
}

/// Obsidian core Templates variables: `{{title}}`, `{{date}}`, `{{time}}`, `{{date:FORMAT}}` and
/// `{{time:FORMAT}}`, FORMAT in moment.js. Anything else is kept, and so is all of a Templater
/// command (`<% %>`): it is code for Templater to run.
pub fn fill(
    text: &str,
    title: &str,
    now: NaiveDateTime,
    date_format: &str,
    time_format: &str,
) -> String {
    static VARIABLE: OnceLock<Regex> = OnceLock::new();
    let re = VARIABLE.get_or_init(|| {
        Regex::new(r"<%[\s\S]*?%>|\{\{\s*(date|time|title)\s*(?::([^}]*))?\}\}").unwrap()
    });
    re.replace_all(text, |c: &regex::Captures| {
        let Some(variable) = c.get(1) else {
            return c[0].to_string();
        };
        let format = c.get(2).map(|m| m.as_str().trim());
        match variable.as_str() {
            "title" => title.to_string(),
            "date" => format_datetime(now.date(), now.time(), format.unwrap_or(date_format)),
            _ => format_datetime(now.date(), now.time(), format.unwrap_or(time_format)),
        }
    })
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at(h: u32, m: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 11, 7)
            .unwrap()
            .and_hms_opt(h, m, 30)
            .unwrap()
    }

    #[test]
    fn fills_every_form_of_the_core_variables() {
        let t = "# {{title}}\n{{date}} {{time}} · {{date:dddd, MMMM Do}} · {{ time:HH:mm:ss }} · \
                 {{date:[W]WW}} · {{time:h A}}";
        assert_eq!(
            fill(t, "Etna", at(14, 5), "YYYY-MM-DD", "HH:mm"),
            "# Etna\n2026-11-07 14:05 · Saturday, November 7th · 14:05:30 · W45 · 2 PM"
        );
    }

    #[test]
    fn the_workspaces_formats_change_only_the_bare_variables() {
        let s = Settings {
            folders: vec![],
            date_format: "DD.MM.YYYY".into(),
            time_format: "h:mm A".into(),
        };
        assert_eq!(
            s.fill("{{date}} {{time}} {{date:YYYY}}", "x", at(9, 5)),
            "07.11.2026 9:05 AM 2026"
        );
    }

    #[test]
    fn keeps_unknown_variables_and_templater() {
        let t =
            "{{author}} {{Title}} {{ date:YYYY }} <% tp.file.title %> <%* tR += \"{{title}}\" %>";
        assert_eq!(
            fill(t, "Etna", at(9, 5), "YYYY-MM-DD", "HH:mm"),
            "{{author}} {{Title}} 2026 <% tp.file.title %> <%* tR += \"{{title}}\" %>"
        );
    }

    #[test]
    fn reads_the_core_and_templater_folders() {
        let root = std::env::temp_dir().join(format!("cold-lava-templates-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".obsidian/plugins/templater-obsidian")).unwrap();
        assert!(Settings::load(&root).folders.is_empty(), "nothing set up");
        std::fs::write(
            root.join(".obsidian/templates.json"),
            r#"{"folder": "/Meta/Templates/", "dateFormat": "DD.MM.YYYY"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join(".obsidian/plugins/templater-obsidian/data.json"),
            r#"{"templates_folder": "Templater"}"#,
        )
        .unwrap();
        let s = Settings::load(&root);
        assert_eq!(s.folders, ["Meta/Templates", "Templater"]);
        assert_eq!(
            (s.date_format.as_str(), s.time_format.as_str()),
            ("DD.MM.YYYY", "HH:mm")
        );
        assert!(s.is_template("Meta/Templates/Volcano.md"));
        assert!(s.is_template("Templater/Sub/Meeting"));
        assert!(!s.is_template("Meta/Templates Old/Volcano.md"));
        assert!(!s.is_template("Templater.md"));
        assert_eq!(s.name("Templater/Sub/Meeting"), "Sub/Meeting");
        let _ = std::fs::remove_dir_all(&root);
    }
}
