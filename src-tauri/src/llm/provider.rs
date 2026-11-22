//! Which company answers (REQUESTS.md 2): OpenRouter, as in Electron, or a
//! native client for Anthropic, OpenAI or Google Gemini with the user's own
//! key. The provider is read from the key's prefix and can be overridden in
//! Settings. The three local command-line tools (Claude Code, Codex, Gemini
//! CLI) answer on the person's own subscription with no key (llm::cli);
//! they are used only once turned on in Settings.

use super::catalogue::{Curated, Kind, Tier};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Provider {
    OpenRouter,
    Anthropic,
    OpenAi,
    Gemini,
    /// Claude Code (`claude`), on the person's Claude subscription.
    ClaudeCli,
    /// OpenAI Codex (`codex`), on the person's ChatGPT subscription.
    CodexCli,
    /// Gemini CLI (`gemini`), on the person's Google account.
    GeminiCli,
}

impl Provider {
    /// Settings order. OpenRouter first: its free models are the default path.
    pub const ALL: [Provider; 4] = [
        Provider::OpenRouter,
        Provider::Anthropic,
        Provider::OpenAi,
        Provider::Gemini,
    ];

    /// The stable id stored in settings and sent to the renderer.
    pub fn id(self) -> &'static str {
        match self {
            Provider::OpenRouter => "openrouter",
            Provider::Anthropic => "anthropic",
            Provider::OpenAi => "openai",
            Provider::Gemini => "gemini",
            Provider::ClaudeCli => "claude-cli",
            Provider::CodexCli => "codex-cli",
            Provider::GeminiCli => "gemini-cli",
        }
    }

    /// The local CLIs, in Settings order. Not in [`Provider::ALL`]: they
    /// join the try order only when turned on.
    pub const CLI: [Provider; 3] = [Provider::ClaudeCli, Provider::CodexCli, Provider::GeminiCli];

    pub fn from_id(id: &str) -> Option<Provider> {
        let id = id.trim();
        Provider::ALL
            .into_iter()
            .chain(Provider::CLI)
            .find(|p| p.id() == id)
    }

    /// A local command-line tool rather than an HTTP API.
    pub fn is_cli(self) -> bool {
        Provider::CLI.contains(&self)
    }

    /// The name used in sentences the user reads.
    pub fn name(self) -> &'static str {
        match self {
            Provider::OpenRouter => "OpenRouter",
            Provider::Anthropic => "Anthropic",
            Provider::OpenAi => "OpenAI",
            Provider::Gemini => "Google Gemini",
            Provider::ClaudeCli => "Claude Code",
            Provider::CodexCli => "Codex",
            Provider::GeminiCli => "Gemini CLI",
        }
    }

    /// The provider a key belongs to, from its prefix. `sk-ant-` and `sk-or-`
    /// are checked before the bare `sk-` OpenAI uses (`sk-proj-`, `sk-svcacct-`,
    /// legacy `sk-`); Google API keys start with `AIza`.
    pub fn detect(key: &str) -> Option<Provider> {
        let k = key.trim();
        if k.starts_with("sk-ant-") {
            Some(Provider::Anthropic)
        } else if k.starts_with("sk-or-") {
            Some(Provider::OpenRouter)
        } else if k.starts_with("sk-") {
            Some(Provider::OpenAi)
        } else if k.starts_with("AIza") {
            Some(Provider::Gemini)
        } else {
            None
        }
    }

    /// The curated models for this provider, in picker order. OpenRouter's
    /// table is the Electron one and lives in the catalogue module.
    pub fn models(self, kind: Kind) -> Vec<&'static Curated> {
        match self {
            Provider::OpenRouter => super::catalogue::pool_for(kind),
            Provider::Anthropic => ANTHROPIC.iter().collect(),
            Provider::OpenAi => OPENAI.iter().collect(),
            Provider::Gemini => GEMINI.iter().collect(),
            Provider::ClaudeCli => CLAUDE_CLI.iter().collect(),
            Provider::CodexCli => CODEX_CLI.iter().collect(),
            Provider::GeminiCli => GEMINI_CLI.iter().collect(),
        }
    }

    /// The model each job uses when the user has not picked one.
    pub fn default_model(self, kind: Kind) -> &'static str {
        match (self, kind) {
            (Provider::OpenRouter, k) => super::catalogue::default_for(k),
            (Provider::Anthropic, Kind::Vision) => "claude-sonnet-5-5",
            (Provider::Anthropic, _) => "claude-opus-5-5",
            (Provider::OpenAi, Kind::Vision) => "gpt-6.1-terra",
            (Provider::OpenAi, _) => "gpt-6.1-sol",
            (Provider::Gemini, Kind::Vision) => "gemini-3.8-flash",
            (Provider::Gemini, Kind::Agent) => "gemini-3.8-flash",
            (Provider::Gemini, Kind::Text) => "gemini-4-argon",
            (Provider::ClaudeCli, Kind::Vision) => "sonnet",
            (Provider::ClaudeCli, _) => "opus",
            // The model the tool itself is set to: no `--model` goes out.
            (Provider::CodexCli, _) => super::cli::DEFAULT_MODEL,
            (Provider::GeminiCli, _) => super::cli::DEFAULT_MODEL,
        }
    }

    /// Native providers bill every call; only OpenRouter has free models,
    /// failover and a public catalogue. The CLIs count as native: one model
    /// list of their own, no free-model failover.
    pub fn is_native(self) -> bool {
        self != Provider::OpenRouter
    }
}

// Checked 2026-11-07 against the provider catalogues (and OpenRouter's mirror
// of them). All of these read images and call tools.
#[rustfmt::skip]
const ANTHROPIC: [Curated; 3] = [
    Curated {
        id: "claude-opus-5-5",
        tier: Tier::Paid,
        note: "Anthropic's flagship. The most careful reader and the strongest agent here.",
    },
    Curated {
        id: "claude-sonnet-5-5",
        tier: Tier::Paid,
        note: "Fast and sharp at reading a screen. The default for screen answers.",
    },
    Curated {
        id: "claude-haiku-5-5",
        tier: Tier::Paid,
        note: "The quickest and cheapest Claude.",
    },
];

#[rustfmt::skip]
const OPENAI: [Curated; 3] = [
    Curated {
        id: "gpt-6.1-sol",
        tier: Tier::Paid,
        note: "OpenAI's strongest model below the flagship. Good at multi-step work.",
    },
    Curated {
        id: "gpt-6.1-terra",
        tier: Tier::Paid,
        note: "The everyday GPT-6.1. Quick to answer about a screen.",
    },
    Curated {
        id: "gpt-6-luna",
        tier: Tier::Paid,
        note: "The fast, low-cost GPT-6.",
    },
];

#[rustfmt::skip]
const GEMINI: [Curated; 3] = [
    Curated {
        id: "gemini-4-argon",
        tier: Tier::Paid,
        note: "Google's flagship reasoning model. Slower to start, best at hard problems.",
    },
    Curated {
        id: "gemini-3.8-flash",
        tier: Tier::Paid,
        note: "The strongest Flash. Quick, and reads screens well.",
    },
    Curated {
        id: "gemini-3.5-flash-lite",
        tier: Tier::Paid,
        note: "The lightest and cheapest Gemini.",
    },
];

// The CLIs take their own aliases; `default` sends no `--model`.
#[rustfmt::skip]
const CLAUDE_CLI: [Curated; 3] = [
    Curated {
        id: "opus",
        tier: Tier::Paid,
        note: "The newest Opus on your Claude plan. The most careful, and the slowest.",
    },
    Curated {
        id: "sonnet",
        tier: Tier::Paid,
        note: "The newest Sonnet on your Claude plan. Quick at reading a screen.",
    },
    Curated {
        id: "haiku",
        tier: Tier::Paid,
        note: "The newest Haiku on your Claude plan. The fastest.",
    },
];

#[rustfmt::skip]
const CODEX_CLI: [Curated; 1] = [
    Curated {
        id: super::cli::DEFAULT_MODEL,
        tier: Tier::Paid,
        note: "The model Codex is set to use on your ChatGPT plan.",
    },
];

#[rustfmt::skip]
const GEMINI_CLI: [Curated; 3] = [
    Curated {
        id: super::cli::DEFAULT_MODEL,
        tier: Tier::Paid,
        note: "The model Gemini CLI picks for your Google account.",
    },
    Curated {
        id: "gemini-4-argon",
        tier: Tier::Paid,
        note: "Google's flagship reasoning model.",
    },
    Curated {
        id: "gemini-3.8-flash",
        tier: Tier::Paid,
        note: "The strongest Flash. Quick, and reads screens well.",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_recognised_by_prefix() {
        let cases = [
            ("sk-ant-api03-abc", Some(Provider::Anthropic)),
            ("sk-or-v1-abc", Some(Provider::OpenRouter)),
            ("sk-proj-abc", Some(Provider::OpenAi)),
            ("sk-abc", Some(Provider::OpenAi)),
            ("  AIzaSyAbc ", Some(Provider::Gemini)),
            ("hello", None),
            ("", None),
        ];
        for (key, want) in cases {
            assert_eq!(Provider::detect(key), want, "{key}");
        }
    }

    #[test]
    fn anthropic_defaults_are_the_latest_models() {
        let a = Provider::Anthropic;
        let ids: Vec<&str> = a.models(Kind::Vision).iter().map(|c| c.id).collect();
        let want = ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"];
        assert_eq!(ids, want);
        assert_eq!(a.default_model(Kind::Text), "claude-opus-5-5");
        assert_eq!(a.default_model(Kind::Agent), "claude-opus-5-5");
        assert_eq!(a.default_model(Kind::Vision), "claude-sonnet-5-5");
    }

    #[test]
    fn every_default_is_in_its_own_list() {
        for p in Provider::ALL {
            for k in [Kind::Vision, Kind::Text, Kind::Agent] {
                let d = p.default_model(k);
                let pool = if p == Provider::OpenRouter {
                    super::super::catalogue::pool_for(Kind::Agent)
                } else {
                    p.models(k)
                };
                assert!(pool.iter().any(|c| c.id == d), "{p:?} {k:?} {d}");
            }
            assert_eq!(Provider::from_id(p.id()), Some(p));
        }
        for p in Provider::CLI {
            assert!(p.is_cli() && p.is_native());
            assert_eq!(Provider::from_id(p.id()), Some(p));
            for k in [Kind::Vision, Kind::Text, Kind::Agent] {
                let d = p.default_model(k);
                assert!(p.models(k).iter().any(|c| c.id == d), "{p:?} {k:?} {d}");
            }
        }
        assert!(Provider::ALL.iter().all(|p| !p.is_cli()));
    }
}
