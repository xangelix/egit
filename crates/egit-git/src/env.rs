//! The environment every git process runs in.
//!
//! egit inherits whatever environment it was started from, which may have been a git hook, a
//! shell with `GIT_DIR` exported, or a terminal whose locale translates git's messages. Each
//! git process gets that environment minus everything that would make it act on another
//! repository or wait for a person: no pager, no editor, no terminal prompt, and English
//! messages (egit recognises a few of them, such as a held lock). The locale's character set
//! stays, so hooks still see the user's encoding.

#[cfg(test)]
use std::ffi::OsStr;
use std::ffi::OsString;
use std::process::Command;

/// Variables that point git at a particular repository, index, or object store, exactly as
/// `git rev-parse --local-env-vars` lists them. egit always names the repository itself.
const REPOSITORY_VARIABLES: [&str; 15] = [
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
];

/// The numbered pairs `GIT_CONFIG_COUNT` refers to.
const CONFIG_PAIR_PREFIXES: [&str; 2] = ["GIT_CONFIG_KEY_", "GIT_CONFIG_VALUE_"];

/// Variables egit sets for every git process.
const FIXED: [(&str, &str); 7] = [
    // Messages in English; the character set (`LC_CTYPE`, `LANG`) is left alone.
    ("LC_MESSAGES", "C"),
    // Git never asks on a terminal: it uses the askpass helper, or fails.
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_PAGER", "cat"),
    ("PAGER", "cat"),
    // `:` is git's "no editor": messages and todo lists are kept as egit wrote them.
    ("GIT_EDITOR", ":"),
    ("GIT_SEQUENCE_EDITOR", ":"),
    ("GIT_MERGE_AUTOEDIT", "no"),
];

/// The changes made to the inherited environment for every git process.
#[derive(Clone, Debug, Default)]
pub(crate) struct Environment {
    remove: Vec<OsString>,
    set: Vec<(OsString, OsString)>,
}

impl Environment {
    /// Plans the environment from the variables egit itself was started with.
    pub(crate) fn plan(inherited: impl IntoIterator<Item = (OsString, OsString)>) -> Self {
        let mut remove: Vec<OsString> = REPOSITORY_VARIABLES.iter().map(OsString::from).collect();
        // `LANGUAGE` outranks `LC_MESSAGES` for translations; `GPG_TTY` would send pinentry to
        // the terminal egit was started from, where nobody is looking.
        remove.extend(["LANGUAGE", "GPG_TTY", "LC_ALL"].map(OsString::from));
        let mut set: Vec<(OsString, OsString)> = FIXED
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect();

        let mut lc_all = None;
        let mut has_lc_ctype = false;
        for (key, value) in inherited {
            let Some(name) = key.to_str() else { continue };
            if CONFIG_PAIR_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix))
            {
                remove.push(key);
            } else if name == "LC_ALL" {
                lc_all = Some(value);
            } else if name == "LC_CTYPE" {
                has_lc_ctype = true;
            }
        }
        // `LC_ALL` overrides every category, `LC_MESSAGES` included, so it goes; whatever
        // character set it chose is kept as `LC_CTYPE`.
        if let Some(value) = lc_all.filter(|value| !value.is_empty())
            && !has_lc_ctype
        {
            set.push((OsString::from("LC_CTYPE"), value));
        }

        Self { remove, set }
    }

    /// Adds a variable, set after (and so over) egit's own.
    pub(crate) fn push(&mut self, key: impl Into<OsString>, value: impl Into<OsString>) {
        self.set.push((key.into(), value.into()));
    }

    /// Removes a variable, whatever was planned for it.
    pub(crate) fn unset(&mut self, key: impl Into<OsString>) {
        let key = key.into();
        self.set.retain(|(name, _)| *name != key);
        self.remove.push(key);
    }

    /// Applies the plan to a command about to be spawned.
    pub(crate) fn apply(&self, command: &mut Command) {
        for key in &self.remove {
            command.env_remove(key);
        }
        for (key, value) in &self.set {
            command.env(key, value);
        }
    }

    /// The value the plan gives `key`, if it sets one (the last one set wins).
    #[cfg(test)]
    fn value(&self, key: &str) -> Option<&OsStr> {
        self.set
            .iter()
            .rev()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_os_str())
    }

    /// Whether the plan removes `key`.
    #[cfg(test)]
    fn removes(&self, key: &str) -> bool {
        self.remove.iter().any(|name| name == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(vars: &[(&str, &str)]) -> Environment {
        Environment::plan(
            vars.iter()
                .map(|(key, value)| (OsString::from(key), OsString::from(value))),
        )
    }

    #[test]
    fn repository_variables_never_reach_git() {
        let environment = plan(&[
            ("GIT_DIR", "/elsewhere/.git"),
            ("GIT_WORK_TREE", "/elsewhere"),
        ]);
        for name in REPOSITORY_VARIABLES {
            assert!(environment.removes(name), "{name} survives");
        }
    }

    #[test]
    fn numbered_config_pairs_are_removed_with_their_count() {
        let environment = plan(&[
            ("GIT_CONFIG_COUNT", "1"),
            ("GIT_CONFIG_KEY_0", "core.hooksPath"),
            ("GIT_CONFIG_VALUE_0", "/tmp/hooks"),
        ]);
        assert!(environment.removes("GIT_CONFIG_COUNT"));
        assert!(environment.removes("GIT_CONFIG_KEY_0"));
        assert!(environment.removes("GIT_CONFIG_VALUE_0"));
    }

    #[test]
    fn nothing_waits_for_a_person() {
        let environment = plan(&[("GIT_TERMINAL_PROMPT", "1"), ("GPG_TTY", "/dev/pts/3")]);
        assert_eq!(
            environment.value("GIT_TERMINAL_PROMPT"),
            Some(OsStr::new("0"))
        );
        assert_eq!(environment.value("GIT_EDITOR"), Some(OsStr::new(":")));
        assert_eq!(
            environment.value("GIT_SEQUENCE_EDITOR"),
            Some(OsStr::new(":"))
        );
        assert_eq!(
            environment.value("GIT_MERGE_AUTOEDIT"),
            Some(OsStr::new("no"))
        );
        assert_eq!(environment.value("GIT_PAGER"), Some(OsStr::new("cat")));
        assert!(environment.removes("GPG_TTY"));
    }

    #[test]
    fn messages_are_english_and_the_character_set_stays() {
        let environment = plan(&[("LC_ALL", "de_DE.UTF-8"), ("LANGUAGE", "de")]);
        assert!(environment.removes("LC_ALL"));
        assert!(environment.removes("LANGUAGE"));
        assert_eq!(environment.value("LC_MESSAGES"), Some(OsStr::new("C")));
        assert_eq!(
            environment.value("LC_CTYPE"),
            Some(OsStr::new("de_DE.UTF-8"))
        );
    }

    #[test]
    fn an_explicit_character_set_wins_over_lc_all() {
        let environment = plan(&[("LC_ALL", "de_DE.UTF-8"), ("LC_CTYPE", "ja_JP.UTF-8")]);
        assert_eq!(environment.value("LC_CTYPE"), None);
    }

    #[test]
    fn pushed_variables_come_last() {
        let mut environment = plan(&[]);
        environment.push("GIT_PAGER", "less");
        environment.push("HOME", "/sandbox");
        assert_eq!(environment.value("GIT_PAGER"), Some(OsStr::new("less")));
        assert_eq!(environment.value("HOME"), Some(OsStr::new("/sandbox")));
    }

    #[test]
    fn unset_variables_go_whatever_was_planned() {
        let mut environment = plan(&[("GIT_AUTHOR_NAME", "Someone")]);
        environment.push("GIT_PAGER", "less");
        environment.unset("GIT_AUTHOR_NAME");
        environment.unset("GIT_PAGER");
        assert!(environment.removes("GIT_AUTHOR_NAME"));
        assert!(environment.removes("GIT_PAGER"));
        assert_eq!(environment.value("GIT_PAGER"), None);
        assert_eq!(environment.value("GIT_EDITOR"), Some(OsStr::new(":")));
    }
}
