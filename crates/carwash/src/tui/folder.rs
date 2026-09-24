//! Scanning another folder without leaving the UI: a path prompt with directory completion.

use super::app::{App, Effect, Level, Mode};
use super::store::Store;
use crate::config::expand_home;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent};
use std::path::{Path, PathBuf};
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

/// Completions offered at most, so a huge directory cannot flood the prompt.
const MAX_CANDIDATES: usize = 200;

#[derive(Debug, Default)]
pub struct FolderPrompt {
    /// Directories completing `for_input`, written the way the user typed the parent
    /// (`~/wo` offers `~/work/`).
    candidates: Vec<String>,
    for_input: String,
}

impl FolderPrompt {
    /// Candidates for `input`; none while its own are still being listed.
    pub fn for_input(&self, input: &str) -> &[String] {
        if self.for_input == input {
            &self.candidates
        } else {
            &[]
        }
    }

    /// What Tab turns `input` into: the only candidate, or the longest prefix all candidates
    /// share when it adds something.
    fn complete(&self, input: &str) -> Option<String> {
        if self.for_input != input || self.candidates.is_empty() {
            return None;
        }
        let common = common_prefix(&self.candidates);
        (common.chars().count() > input.chars().count()).then_some(common)
    }
}

fn common_prefix(items: &[String]) -> String {
    let Some(first) = items.first() else {
        return String::new();
    };
    let mut prefix: Vec<char> = first.chars().collect();
    for item in &items[1..] {
        let shared = prefix
            .iter()
            .zip(item.chars())
            .take_while(|(a, b)| a.eq_ignore_ascii_case(b))
            .count();
        prefix.truncate(shared);
    }
    prefix.into_iter().collect()
}

/// `raw` as a path: `~` is the home directory, relative paths start at `base`.
pub fn resolve(raw: &str, base: &Path, home: Option<&Path>) -> PathBuf {
    let path = expand_home(raw.trim(), home);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

/// Subdirectories completing `input`, case-insensitively; hidden ones only when the typed
/// name starts with a dot.
pub fn complete_dirs(input: &str, base: &Path, home: Option<&Path>) -> Vec<String> {
    let (parent, prefix) = match input.rfind('/') {
        Some(i) => input.split_at(i + 1),
        None => ("", input),
    };
    let dir = if parent.is_empty() {
        base.to_path_buf()
    } else {
        resolve(parent, base, home)
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let wanted = prefix.to_lowercase();
    let mut out: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let visible = !name.starts_with('.') || prefix.starts_with('.');
            (visible && name.to_lowercase().starts_with(&wanted) && e.path().is_dir())
                .then(|| format!("{parent}{name}/"))
        })
        .collect();
    out.sort_by_key(|c| c.to_lowercase());
    out.truncate(MAX_CANDIDATES);
    out
}

impl App {
    /// Opens the prompt on the current folder.
    pub fn open_folder_prompt(&mut self) -> Vec<Effect> {
        let current = format!("{}/", super::view::home_relative(&self.store.root));
        self.input = Input::new(current.clone());
        self.folder = FolderPrompt::default();
        self.mode = Mode::Folder;
        vec![Effect::CompletePath(current)]
    }

    pub fn on_folder_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Browse;
                Vec::new()
            }
            KeyCode::Enter => {
                self.mode = Mode::Browse;
                let value = self.input.value().trim().to_owned();
                if value.is_empty() {
                    return Vec::new();
                }
                vec![Effect::ChangeRoot(value)]
            }
            KeyCode::Tab => match self.folder.complete(self.input.value()) {
                Some(completed) => {
                    self.input = Input::new(completed.clone());
                    vec![Effect::CompletePath(completed)]
                }
                None => {
                    self.dirty = false;
                    Vec::new()
                }
            },
            _ => {
                if self.input.handle_event(&Event::Key(key)).is_some() {
                    vec![Effect::CompletePath(self.input.value().to_owned())]
                } else {
                    Vec::new()
                }
            }
        }
    }

    /// Completions arrive after the fact; keep them only if the input has not moved on.
    pub fn on_path_completions(&mut self, input: String, candidates: Vec<String>) {
        if matches!(self.mode, Mode::Folder) && input == self.input.value() {
            self.folder = FolderPrompt {
                candidates,
                for_input: input,
            };
        } else {
            self.dirty = false;
        }
    }

    /// A new root replaces everything tied to the previous scan; running jobs carry on.
    pub fn on_root_changed(&mut self, result: Result<PathBuf, String>) -> Vec<Effect> {
        let root = match result {
            Ok(root) => root,
            Err(error) => {
                self.toast(error, Level::Warn);
                return Vec::new();
            }
        };
        self.toast(
            format!("Scanning {}", super::view::home_relative(&root)),
            Level::Info,
        );
        self.store = Store::new(root);
        self.marked.clear();
        self.expanded.clear();
        self.set_query(String::new());
        self.tasks.forget_projects();
        self.updates.forget_projects();
        vec![Effect::Scan, Effect::RefreshDisk]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::Msg;
    use crate::tui::app::tests::app;
    use ratatui::crossterm::event::KeyModifiers;
    use std::fs;

    fn press(app: &mut App, code: KeyCode) -> Vec<Effect> {
        app.update(Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)))
    }

    #[test]
    fn completes_like_a_shell() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        for sub in ["work", "Workshop", "web", ".hidden", "work/carwash"] {
            fs::create_dir_all(base.join(sub)).unwrap();
        }
        fs::write(base.join("wobble.txt"), b"").unwrap();

        assert_eq!(complete_dirs("wo", base, None), ["work/", "Workshop/"]);
        assert_eq!(complete_dirs("work/c", base, None), ["work/carwash/"]);
        assert_eq!(complete_dirs(".h", base, None), [".hidden/"]);
        assert!(!complete_dirs("", base, None).contains(&".hidden/".to_owned()));
        let home = complete_dirs("~/we", Path::new("/elsewhere"), Some(base));
        assert_eq!(home, ["~/web/"]);

        let prompt = FolderPrompt {
            candidates: complete_dirs("wo", base, None),
            for_input: "wo".into(),
        };
        assert_eq!(prompt.complete("wo").as_deref(), Some("work"));
        assert_eq!(prompt.complete("stale"), None);
    }

    #[test]
    fn resolves_home_and_relative_paths() {
        let home = Path::new("/home/me");
        let base = Path::new("/home/me/work");
        assert_eq!(
            resolve("~/x", base, Some(home)),
            PathBuf::from("/home/me/x")
        );
        assert_eq!(
            resolve("sub", base, Some(home)),
            PathBuf::from("/home/me/work/sub")
        );
        assert_eq!(resolve("/abs", base, Some(home)), PathBuf::from("/abs"));
    }

    #[test]
    fn the_prompt_changes_the_root_and_resets_scan_state() {
        let mut app = app();
        press(&mut app, KeyCode::Char('a'));
        assert!(!app.marked.is_empty());

        let effects = press(&mut app, KeyCode::Char('p'));
        assert!(matches!(app.mode, Mode::Folder));
        assert!(matches!(effects.as_slice(), [Effect::CompletePath(_)]));
        app.input = Input::new("/tmp/elsewhere".into());
        let effects = press(&mut app, KeyCode::Enter);
        assert!(matches!(effects.as_slice(), [Effect::ChangeRoot(p)] if p == "/tmp/elsewhere"));

        let effects = app.update(Msg::RootChanged(Ok(PathBuf::from("/tmp/elsewhere"))));
        assert!(matches!(effects.first(), Some(Effect::Scan)));
        assert_eq!(app.store.root, PathBuf::from("/tmp/elsewhere"));
        assert!(app.marked.is_empty());
        assert!(app.store.entries.is_empty());

        app.update(Msg::RootChanged(Err("cannot open /nope".into())));
        assert_eq!(app.toast.as_ref().unwrap().text, "cannot open /nope");
    }

    #[test]
    fn tab_uses_completions_only_for_the_current_input() {
        let mut app = app();
        press(&mut app, KeyCode::Char('p'));
        app.input = Input::new("~/wo".into());
        app.update(Msg::PathCompletions {
            input: "~/old".into(),
            candidates: vec!["~/older/".into()],
        });
        assert!(
            press(&mut app, KeyCode::Tab).is_empty(),
            "stale completions ignored"
        );
        app.update(Msg::PathCompletions {
            input: "~/wo".into(),
            candidates: vec!["~/work/".into()],
        });
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.input.value(), "~/work/");
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Browse));
    }
}
