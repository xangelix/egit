//! What egit can be asked to do from the keyboard: the commands the palette offers (⌘K), the
//! shortcuts that run some at once, and the list of every shortcut the Preferences tab shows.
//!
//! A command about a repository goes to the repository in front (`RepoCommand`), whose history
//! view carries it out next frame with everything its own buttons have; the others are the
//! application's (`Action`s).

use std::borrow::Cow;

use bstr::BString;
use egui::{Key, KeyboardShortcut, Modifiers};

use crate::theme::ThemeChoice;

/// Something a repository's history view does, asked from the keyboard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RepoCommand {
    Fetch,
    Pull,
    Push,
    Undo,
    Redo,
    Search,
    FilterSidebar,
    ToggleSidebar,
    /// Show or hide the output panel (what tasks and hooks printed).
    ToggleOutput,
    /// Show or hide the agents' panel.
    ToggleAgents,
    Reflog,
    WorkTree,
    PullRequests,
    Stash,
    ApplyPatches,
    PastePatch,
    CreateBranch,
    /// Switch to this local branch (its short name).
    Switch(BString),
    /// Show this commit (a remote's branch's, a tag's) in the history.
    GoTo(egit_git::ObjectId),
    /// The history's statistics.
    Stats,
    /// The repository's overview, and its settings.
    Overview,
    Settings,
    /// Git LFS's page, and the submodules'.
    GitLfs,
    Submodules,
    /// The repository's folder, in the file manager.
    OpenFolder,
    AddRemote,
    CreateTag,
    Refresh,
}

/// A command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    /// The command palette.
    Palette,
    /// Help: every topic.
    Help,
    Preferences,
    OpenRepository,
    CloneRepository,
    Home,
    Profile,
    CloseTab,
    /// The `n`th tab (from 1).
    Tab(usize),
    Theme(ThemeChoice),
    /// Open a repository opened before (here, or on another computer).
    OpenRecent(crate::settings::Location),
    /// Something about the repository in front.
    Repo(RepoCommand),
}

impl Command {
    /// The commands the palette always offers, in its order (the repository's first).
    pub(crate) fn listed() -> Vec<Self> {
        let repo = [
            RepoCommand::Search,
            RepoCommand::Fetch,
            RepoCommand::Pull,
            RepoCommand::Push,
            RepoCommand::Undo,
            RepoCommand::Redo,
            RepoCommand::WorkTree,
            RepoCommand::Reflog,
            RepoCommand::PullRequests,
            RepoCommand::Stash,
            RepoCommand::ApplyPatches,
            RepoCommand::PastePatch,
            RepoCommand::CreateBranch,
            RepoCommand::CreateTag,
            RepoCommand::AddRemote,
            RepoCommand::Stats,
            RepoCommand::Overview,
            RepoCommand::Settings,
            RepoCommand::GitLfs,
            RepoCommand::Submodules,
            RepoCommand::OpenFolder,
            RepoCommand::ToggleSidebar,
            RepoCommand::FilterSidebar,
            RepoCommand::ToggleOutput,
            RepoCommand::ToggleAgents,
            RepoCommand::Refresh,
        ];
        repo.into_iter()
            .map(Self::Repo)
            .chain([
                Self::OpenRepository,
                Self::CloneRepository,
                Self::Home,
                Self::Profile,
                Self::Preferences,
                Self::Help,
                Self::Theme(ThemeChoice::System),
                Self::Theme(ThemeChoice::Light),
                Self::Theme(ThemeChoice::Dark),
                Self::CloseTab,
            ])
            .collect()
    }

    /// What it does, in words (`nouns`: the forge's requests, "pull requests" or "merge
    /// requests").
    pub(crate) fn label(&self, nouns: &str) -> String {
        match self {
            Self::Palette => "Show every command".to_owned(),
            Self::Help => "Help: what everything does".to_owned(),
            Self::Preferences => "Preferences".to_owned(),
            Self::OpenRepository => "Open a repository…".to_owned(),
            Self::CloneRepository => "Clone a repository".to_owned(),
            Self::Home => "Go home".to_owned(),
            Self::Profile => "Your git profile".to_owned(),
            Self::CloseTab => "Close the tab".to_owned(),
            Self::Tab(n) => format!("Go to tab {n}"),
            Self::Theme(ThemeChoice::System) => "Theme: as the desktop's".to_owned(),
            Self::Theme(ThemeChoice::Light) => "Theme: light".to_owned(),
            Self::Theme(ThemeChoice::Dark) => "Theme: dark".to_owned(),
            Self::OpenRecent(location) => location.host.as_ref().map_or_else(
                || format!("Open {}", location.path.display()),
                |host| format!("Open {} on {host}", location.path.display()),
            ),
            Self::Repo(command) => match command {
                RepoCommand::Fetch => "Fetch".to_owned(),
                RepoCommand::Pull => "Pull".to_owned(),
                RepoCommand::Push => "Push".to_owned(),
                RepoCommand::Undo => "Undo".to_owned(),
                RepoCommand::Redo => "Redo".to_owned(),
                RepoCommand::Search => "Search the history".to_owned(),
                RepoCommand::FilterSidebar => "Filter the sidebar".to_owned(),
                RepoCommand::ToggleSidebar => "Show or hide the sidebar".to_owned(),
                RepoCommand::ToggleOutput => "Show or hide the output".to_owned(),
                RepoCommand::ToggleAgents => "Show or hide the agents".to_owned(),
                RepoCommand::Reflog => "Show the reflog".to_owned(),
                RepoCommand::WorkTree => "Show the uncommitted changes".to_owned(),
                RepoCommand::PullRequests => format!("Show the {nouns}"),
                RepoCommand::Stash => "Stash the changes…".to_owned(),
                RepoCommand::ApplyPatches => "Apply patches…".to_owned(),
                RepoCommand::PastePatch => "Paste a patch…".to_owned(),
                RepoCommand::CreateBranch => "Create a branch…".to_owned(),
                RepoCommand::Switch(branch) => format!("{SWITCH_TO}{branch}"),
                RepoCommand::GoTo(commit) => format!("Go to {}", commit.abbreviated(8)),
                RepoCommand::Stats => "Show the history's statistics".to_owned(),
                RepoCommand::Overview => "Show the repository's overview".to_owned(),
                RepoCommand::Settings => "Show the repository's settings".to_owned(),
                RepoCommand::GitLfs => "Show Git LFS".to_owned(),
                RepoCommand::Submodules => "Show the submodules".to_owned(),
                RepoCommand::OpenFolder => "Open the repository's folder".to_owned(),
                RepoCommand::AddRemote => "Add a remote…".to_owned(),
                RepoCommand::CreateTag => "Create a tag…".to_owned(),
                RepoCommand::Refresh => "Read the repository again".to_owned(),
            },
        }
    }

    /// Its keyboard shortcut, as a Mac shows it ([`keys_text`] shows it as this platform does).
    pub(crate) const fn keys(&self) -> Option<&'static str> {
        Some(match self {
            Self::Palette => "⌘K",
            Self::Help => "F1",
            Self::Preferences => "⌘,",
            Self::OpenRepository => "⌘O",
            Self::CloseTab => CLOSE_TAB,
            Self::Repo(RepoCommand::Search) => "⌘F",
            Self::Repo(RepoCommand::Undo) => "⌘Z",
            Self::Repo(RepoCommand::Redo) => "⇧⌘Z",
            Self::Repo(RepoCommand::FilterSidebar) => "⌥⌘F",
            Self::Repo(RepoCommand::ToggleSidebar) => "⌘B",
            Self::Repo(RepoCommand::ToggleOutput) => "⌘J",
            Self::Repo(RepoCommand::ToggleAgents) => "⌘L",
            Self::Repo(RepoCommand::Refresh) => "⌘R",
            _ => return None,
        })
    }

    /// Whether it is about the repository in front (and needs one).
    pub(crate) const fn needs_repo(&self) -> bool {
        matches!(self, Self::Repo(_))
    }
}

/// The shortcuts the application itself takes (the views take theirs: ⌘F, ⌘Z, …), the more
/// specific first (⌘K alone would match ⇧⌘K too).
fn global() -> [(Modifiers, Key, Command); 10] {
    [
        (Modifiers::NONE, Key::F1, Command::Help),
        // ⇧⌘P, as other editors have it.
        (
            Modifiers::COMMAND | Modifiers::SHIFT,
            Key::P,
            Command::Palette,
        ),
        (Modifiers::COMMAND, Key::K, Command::Palette),
        (Modifiers::COMMAND, Key::Comma, Command::Preferences),
        (Modifiers::COMMAND, Key::O, Command::OpenRepository),
        (TAB_MODIFIERS, Key::W, Command::CloseTab),
        (
            Modifiers::COMMAND,
            Key::B,
            Command::Repo(RepoCommand::ToggleSidebar),
        ),
        (
            Modifiers::COMMAND,
            Key::R,
            Command::Repo(RepoCommand::Refresh),
        ),
        (
            Modifiers::COMMAND,
            Key::J,
            Command::Repo(RepoCommand::ToggleOutput),
        ),
        (
            Modifiers::COMMAND,
            Key::L,
            Command::Repo(RepoCommand::ToggleAgents),
        ),
    ]
}

/// What closing a tab and going to one take: ⌘ on a desktop; ⌥ in a browser, which keeps ⌘W
/// and ⌘1 … ⌘9 for its own tabs (a page is never told of them).
const TAB_MODIFIERS: Modifiers = if crate::desktop::IN_BROWSER {
    Modifiers::ALT
} else {
    Modifiers::COMMAND
};

/// Closing a tab's keys, as a Mac shows them.
const CLOSE_TAB: &str = if crate::desktop::IN_BROWSER {
    "⌥W"
} else {
    "⌘W"
};

/// Going to a tab's keys, as a Mac shows them.
const TAB_NUMBERS: &str = if crate::desktop::IN_BROWSER {
    "⌥1 … ⌥9"
} else {
    "⌘1 … ⌘9"
};

/// The tabs' keys: 1 to 9, with [`TAB_MODIFIERS`].
const TAB_KEYS: [Key; 9] = [
    Key::Num1,
    Key::Num2,
    Key::Num3,
    Key::Num4,
    Key::Num5,
    Key::Num6,
    Key::Num7,
    Key::Num8,
    Key::Num9,
];

/// The command a shortcut pressed this frame runs, its keys taken (so a view doesn't see them
/// too).
pub(crate) fn pressed(ctx: &egui::Context) -> Option<Command> {
    // In a browser, ⌥ and a letter or digit types a character on a Mac (⌥1 is ¡): the tabs'
    // keys are the text's while it is typed.
    let typing = crate::desktop::IN_BROWSER && ctx.egui_wants_keyboard_input();
    let tabs = !(typing && TAB_MODIFIERS == Modifiers::ALT);
    ctx.input_mut(|input| {
        for (modifiers, key, command) in global() {
            if (tabs || modifiers != TAB_MODIFIERS || key != Key::W)
                && input.consume_shortcut(&KeyboardShortcut::new(modifiers, key))
            {
                return Some(command);
            }
        }
        if !tabs {
            return None;
        }
        TAB_KEYS
            .iter()
            .zip(1..)
            .find(|(key, _)| input.consume_shortcut(&KeyboardShortcut::new(TAB_MODIFIERS, **key)))
            .map(|(_, n)| Command::Tab(n))
    })
}

/// Whether keys are named as a Mac names them: on a Mac, or in a browser on one.
#[cfg(target_arch = "wasm32")]
fn on_a_mac() -> bool {
    crate::web::is_mac()
}

/// Whether keys are named as a Mac names them: on a Mac.
#[cfg(not(target_arch = "wasm32"))]
const fn on_a_mac() -> bool {
    cfg!(target_os = "macos")
}

/// A shortcut as this platform names its keys. Written as a Mac shows it (`⌘K`, `⇧⌘Z`, `⌥⌘F`,
/// `⌥W`), it is shown so on macOS, and elsewhere, where egui's command key is Ctrl, as `Ctrl+K`,
/// `Ctrl+Shift+Z`, `Ctrl+Alt+F`, `Alt+W`.
pub(crate) fn keys_text(mac: &'static str) -> Cow<'static, str> {
    if on_a_mac() || !mac.contains(['⌘', '⌥']) {
        return Cow::Borrowed(mac);
    }
    Cow::Owned(
        mac.replace("⇧⌘", "Ctrl+Shift+")
            .replace("⌥⌘", "Ctrl+Alt+")
            .replace('⌘', "Ctrl+")
            .replace('⌥', "Alt+"),
    )
}

/// The words before a branch's name in a command switching to it.
pub(crate) const SWITCH_TO: &str = "Switch to ";

/// Every shortcut, and what it does, as the Preferences tab lists them.
const SHORTCUTS: [(&str, &str); 27] = [
    ("⌘K, ⇧⌘P", "Show every command"),
    ("F1", "Help: every topic"),
    (
        "Alt (held)",
        "What the control under the pointer does, at git's level",
    ),
    ("⌘,", "Preferences"),
    ("⌘O", "Open a repository"),
    (CLOSE_TAB, "Close the tab"),
    (TAB_NUMBERS, "Go to a tab"),
    ("⌘R", "Read the repository again"),
    ("⌘F", "Search the history (in a file or diff: find in it)"),
    ("⌘B", "Show or hide the sidebar"),
    ("⌥⌘F", "Filter the sidebar"),
    ("⌘J", "Show or hide the output"),
    ("⌘L", "Show or hide the agents"),
    ("⌘Z, ⇧⌘Z", "Undo, redo"),
    ("↑ ↓, PgUp PgDn, Home End", "Move through a list"),
    ("Escape", "Back to the history (from a file, the reflog, …)"),
    ("⌘Enter", "Commit what is staged (the working tree's form)"),
    ("⇧⌘Enter", "Commit what is staged, then push the branch"),
    (
        "P R S F D",
        "Pick, reword, squash, fix up, drop (the rebase editor)",
    ),
    (
        "Alt+↑, Alt+↓",
        "Move a commit (the rebase editor); a conflict (the merge editor)",
    ),
    ("⌘+, ⌘−, ⌘0", "Larger text, smaller, as it was"),
    ("⌘C, ⌘A", "Copy, select all (in text)"),
    ("Enter, Shift+Enter", "Next, previous (in a search or find)"),
    (
        "⇧-click, ⌘-click",
        "Pick a range of commits, or one more (the commit table)",
    ),
    ("⇧↑, ⇧↓", "Pick more commits above or below"),
    (
        "Alt+↑, Alt+↓",
        "The change before or after (a Markdown preview); a section (a job's log)",
    ),
    (
        "Tab, Enter",
        "Complete a slash command (the agents' prompt)",
    ),
];

impl Command {
    /// Every shortcut, and what it does.
    pub(crate) fn shortcuts() -> impl Iterator<Item = (&'static str, &'static str)> {
        SHORTCUTS.into_iter()
    }
}

/// How well `query` matches `text` (letters in either case): `None` when its letters aren't
/// all there in order; more for letters that start words and follow each other.
pub(crate) fn score(query: &str, text: &str) -> Option<u32> {
    score_letters(query, text.chars())
}

/// [`score`] against a text given letter by letter (a branch's name after some words, without
/// putting them together first).
pub(crate) fn score_letters(query: &str, text: impl Iterator<Item = char>) -> Option<u32> {
    let mut score = 0_u32;
    let mut letters = text.enumerate();
    let mut previous: Option<usize> = None;
    let mut before = ' ';
    for wanted in query.chars().filter(|c| !c.is_whitespace()) {
        let wanted = folded(wanted);
        loop {
            let (at, letter) = letters.next()?;
            let starts_word = !before.is_alphanumeric();
            before = letter;
            if folded(letter) == wanted {
                score = score.saturating_add(1);
                if starts_word {
                    score = score.saturating_add(3);
                }
                if previous.is_some_and(|previous| previous.saturating_add(1) == at) {
                    score = score.saturating_add(2);
                }
                previous = Some(at);
                break;
            }
        }
    }
    Some(score)
}

/// A letter in lower case (ASCII without consulting Unicode's tables).
fn folded(letter: char) -> char {
    if letter.is_ascii() {
        letter.to_ascii_lowercase()
    } else {
        letter.to_lowercase().next().unwrap_or(letter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_in_order_match_word_starts_best() {
        assert!(score("ftch", "Fetch").is_some());
        assert_eq!(score("hctef", "Fetch"), None, "out of order");
        assert_eq!(score("", "Anything"), Some(0));
        // A word's start counts more than letters inside one.
        let sidebar = score("sid", "Show or hide the sidebar").unwrap();
        let search = score("sid", "Search the history").unwrap_or(0);
        assert!(sidebar > search, "{sidebar} {search}");
        assert!(score("tp", "Theme: dark").is_none());
    }

    #[test]
    fn shortcuts_are_named_as_the_platform_names_its_keys() {
        let shown = |mac| keys_text(mac).into_owned();
        if cfg!(target_os = "macos") {
            assert_eq!(shown("⇧⌘Z"), "⇧⌘Z");
        } else {
            assert_eq!(shown("⌘K"), "Ctrl+K");
            assert_eq!(shown("⌘Z, ⇧⌘Z"), "Ctrl+Z, Ctrl+Shift+Z");
            assert_eq!(shown("⌥⌘F"), "Ctrl+Alt+F");
            assert_eq!(shown("⌘1 … ⌘9"), "Ctrl+1 … Ctrl+9");
            assert_eq!(shown("⌥1 … ⌥9"), "Alt+1 … Alt+9");
        }
        assert_eq!(shown("Alt+↑, Alt+↓"), "Alt+↑, Alt+↓");
    }

    #[test]
    fn every_listed_command_has_words_and_some_keys() {
        for command in Command::listed() {
            assert_ne!(command.label("pull requests"), "");
        }
        assert_eq!(Command::Palette.keys(), Some("⌘K"));
    }
}
