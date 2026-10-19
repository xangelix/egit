//! Agents: Claude Code working in a repository beside egit.

use super::{Group, Topic};

/// The topics.
pub(super) const TOPICS: &[Topic] = &[
    Topic {
        id: "agents",
        group: Group::Agents,
        title: "Agents",
        on: &["Agents", "Hide the agents"],
        what: "Claude Code working in this repository's work tree, beside the history and the \
               changes it makes: the same Claude Code as in a terminal or an editor (its \
               system prompt, tools, your settings, the project's CLAUDE.md, hooks, MCP servers, \
               skills, and sessions), driven through Anthropic's Claude Agent SDK. What it \
               changes shows in the work tree at once, to review, stage, and commit here.",
        git: "",
        cases: &[
            (
                "Does egit change how Claude Code behaves?",
                "No: it runs Claude Code itself, with your user, project, and local settings, as the terminal does. egit only draws what it says and asks you what it asks.",
            ),
            (
                "Several at once?",
                "Each conversation runs on its own: start another from the menu while one works. Each tab has its own, in its own work tree.",
            ),
            (
                "What does it cost?",
                "Each turn's end says what Claude Code estimates it cost. On a Pro, Max, or Team plan it counts against the plan's usage, not a bill; with an API key, the Console bills it.",
            ),
        ],
    },
    Topic {
        id: "agents-setup",
        group: Group::Agents,
        title: "Setting Claude up",
        on: &["Set up Claude", "Look again", "nodejs.org"],
        what: "The Claude Agent SDK runs under Node.js (18 or newer, with npm). Setting Claude \
               up installs the SDK, at the version egit was made with, and the Claude Code \
               binary for this computer (about 250 MB) into egit's cache, with npm and the \
               exact versions and checksums egit carries; no install scripts run.",
        git: "npm ci --ignore-scripts --legacy-peer-deps",
        cases: &[
            (
                "Node.js isn't found?",
                "Install it (nodejs.org, or your system's packages), then Look again. egit looks for node and npm on the PATH it was started with.",
            ),
            (
                "Behind a proxy or a private registry?",
                "npm uses your own configuration (~/.npmrc): its registry and proxy apply; the checksums still have to match.",
            ),
            (
                "Does it update by itself?",
                "No: each egit uses the SDK version it was made with, installed once; a newer egit installs its own and removes the older one.",
            ),
        ],
    },
    Topic {
        id: "agents-signing-in",
        group: Group::Agents,
        title: "Who Claude runs as",
        on: &["Copy the command"],
        what: "egit has no Claude login of its own: conversations run as Claude Code is signed \
               in on this computer, as in a terminal. Sign in with Claude Code itself (the \
               command shown runs its login); an Anthropic API key in egit's environment \
               (ANTHROPIC_API_KEY) is used instead, as the terminal would.",
        git: "claude auth status --json; claude auth login",
        cases: &[
            (
                "Nobody is signed in?",
                "Run the command shown in a terminal: Claude Code opens the browser to sign in, then Look again.",
            ),
            (
                "Signed in, but it says otherwise?",
                "egit asks the Claude Code it installed, which reads the same login (~/.claude) as yours: sign in again with it if yours is another version's.",
            ),
        ],
    },
    Topic {
        id: "agents-accounts",
        group: Group::Agents,
        title: "Accounts",
        on: &[
            "Add the account",
            "Forget the account",
            "Account's name",
            "Signs in with",
            "A Claude Code folder",
            "An API key",
            "API key",
            "Claude Code's own",
        ],
        what: "A conversation runs as Claude Code's own login here, or as another account chosen \
               under its prompt before it starts. An account is a Claude Code configuration \
               folder of its own (CLAUDE_CONFIG_DIR: its own login, settings, and sessions; \
               signed into with Claude Code itself), or an Anthropic API key, which git's \
               credential helper keeps (as https://api.anthropic.com, the account's name its \
               user). Each conversation runs as its own, so several accounts work at once.",
        git: "git credential approve, fill, reject (an API key's)",
        cases: &[
            (
                "Adding an API key fails?",
                "No credential helper kept it: set one up in Profile (the system's keychain is best), then add it again.",
            ),
            (
                "Signing in to a folder?",
                "Under the prompt, nobody signed in shows Claude Code's own login command for that folder: run it in a terminal, then Look again.",
            ),
            (
                "Its earlier conversations?",
                "Each folder keeps its own: the menu lists those of the account chosen. An API key's are Claude Code's own folder's.",
            ),
        ],
    },
    Topic {
        id: "agents-conversations",
        group: Group::Agents,
        title: "Conversations",
        on: &["New conversation", "Close the conversation"],
        what: "The menu at the panel's top starts a conversation, switches between those open, \
               and goes on with earlier ones Claude Code keeps for this folder (a terminal's \
               and an editor's too: they share them). A conversation starts Claude Code with \
               its first prompt; closing it stops Claude Code, and it stays among the earlier \
               ones.",
        git: "",
        cases: &[
            (
                "Where are they kept?",
                "Where Claude Code keeps them: ~/.claude/projects, by folder. egit keeps none of its own.",
            ),
            (
                "Going on with one from the terminal?",
                "Open it from Earlier: its messages show, and the next prompt goes on with it, as claude --resume would.",
            ),
        ],
    },
    Topic {
        id: "agents-prompt",
        group: Group::Agents,
        title: "Asking Claude",
        on: &["Ask Claude", "Send", "Stop"],
        what: "What you write goes to Claude Code as a prompt (Enter sends it; Shift+Enter starts \
               a new line); slash commands work as in a terminal. While it works, another \
               prompt waits its turn; Stop interrupts the turn under way, keeping what was done.",
        git: "",
        cases: &[(
            "It stopped halfway through an edit?",
            "Stop takes effect between steps: an edit made stays made. The work tree shows it, to keep or discard.",
        )],
    },
    Topic {
        id: "agents-permissions",
        group: Group::Agents,
        title: "Allowing tools",
        on: &["Allow once", "Allow for this conversation", "Refuse"],
        what: "Before running a command, editing a file, or using another tool its rules don't \
               allow already, Claude Code asks. Allow once lets this one run; Allow for this \
               conversation adds Claude Code's own rule for it (this command, these edits) for \
               the rest of the conversation; Refuse tells Claude why not, and it goes on \
               without.",
        git: "",
        cases: &[(
            "Allowed for good, everywhere?",
            "Add rules to your Claude Code settings (permissions in ~/.claude/settings.json, or the project's): egit reads them as the terminal does.",
        )],
    },
    Topic {
        id: "agents-modes",
        group: Group::Agents,
        title: "Permission modes",
        on: &[
            "Ask first",
            "Accept edits",
            "Plan",
            "Auto",
            "Bypass permissions",
        ],
        what: "How much Claude Code asks first. Ask first asks before anything not allowed by \
               a rule; Accept edits edits files without asking, asking for the rest; Plan \
               reads and plans, changing nothing, until you approve the plan; Auto lets Claude \
               Code's own classifier decide what needs asking; Bypass permissions asks nothing.",
        git: "",
        cases: &[(
            "Bypass permissions?",
            "Everything runs without asking, commands included: only where nothing it could do would matter (a throwaway container).",
        )],
    },
    Topic {
        id: "agents-models",
        group: Group::Agents,
        title: "Models",
        on: &["Default model"],
        what: "The model Claude Code uses, from those your account offers; changed from the \
               menu, it applies from the next turn.",
        git: "",
        cases: &[],
    },
    Topic {
        id: "agents-questions",
        group: Group::Agents,
        title: "Claude's questions and plans",
        on: &[
            "Answer",
            "Don't answer",
            "Approve the plan",
            "Keep planning",
        ],
        what: "Claude may ask you to choose (one choice, or several, or an answer in your own \
               words), and in Plan mode shows its plan before changing anything: approve it to \
               let it start, or say what to change to keep planning.",
        git: "",
        cases: &[],
    },
    Topic {
        id: "agents-tools",
        group: Group::Agents,
        title: "What Claude did",
        on: &[],
        what: "Each tool Claude used is a line: a file read, a search, a command run, an edit. \
               Opened, it shows what it was given (the command, the edit as the lines it \
               replaced and those it put there, the plan's steps) and what it gave back; a \
               subagent's work shows inside its line.",
        git: "",
        cases: &[],
    },
];
