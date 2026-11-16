//! Registry of coding-agent integrations.
//!
//! The nod-answer channel itself is agent-agnostic: any agent that can run a
//! shell command before it shows its question can post that question to the
//! `zcode-ask` socket and read the nod choice back. What differs per agent is
//! only how the hook gets installed and removed, so each integration is
//! described here as a small struct the settings UI can iterate over. ZCode
//! ships first; Codex and friends slot in as new entries without touching the
//! UI or the answering engine.

use crate::system::{codex_ask, zcode_ask};
use std::sync::LazyLock;

/// Whether an agent's hook is wired into that agent's configuration.
#[derive(Debug, Clone, PartialEq)]
pub enum HookInstallStatus {
    NotInstalled,
    /// Installed; carries the command path the agent will invoke.
    Installed(String),
    /// The agent's config exists but could not be read as JSON.
    ConfigUnreadable(String),
}

/// One supported agent integration.
pub struct AgentHook {
    /// Stable identifier, e.g. `"zcode"`.
    pub id: &'static str,
    /// Human-facing name for the settings rows.
    pub display_name: &'static str,
    /// One line about what this integration does.
    pub description: &'static str,
    /// `false` for agents whose integration is not implemented yet; the UI
    /// shows them as 即将支持 placeholders.
    pub available: bool,
    install: fn() -> Result<(), String>,
    uninstall: fn() -> Result<(), String>,
    status: fn() -> HookInstallStatus,
}

impl AgentHook {
    pub fn install(&self) -> Result<(), String> {
        (self.install)()
    }

    pub fn uninstall(&self) -> Result<(), String> {
        (self.uninstall)()
    }

    pub fn status(&self) -> HookInstallStatus {
        (self.status)()
    }
}

/// Every agent integration RustPods knows about, in display order.
pub fn registered_agents() -> &'static [AgentHook] {
    static AGENTS: LazyLock<Vec<AgentHook>> = LazyLock::new(|| vec![zcode_hook(), codex_hook()]);
    &AGENTS
}

fn zcode_hook() -> AgentHook {
    AgentHook {
        id: "zcode",
        display_name: "ZCode",
        description: "点头回答 ZCode 的 AskUserQuestion 选择题",
        available: true,
        install: zcode_ask::install_hook,
        uninstall: zcode_ask::uninstall_hook,
        status: zcode_ask::hook_status,
    }
}

fn codex_hook() -> AgentHook {
    AgentHook {
        id: "codex",
        display_name: "Codex",
        description: "点头回答 Codex / ChatGPT 的选择题",
        available: true,
        install: codex_ask::install_hook,
        uninstall: codex_ask::uninstall_hook,
        status: codex_ask::hook_status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_contains_zcode_and_codex_in_order() {
        let agents = registered_agents();
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].id, "zcode");
        assert!(agents[0].available);
        assert_eq!(agents[1].id, "codex");
        assert!(agents[1].available);
    }

    #[test]
    fn zcode_status_reflects_this_machines_config() {
        // The real config may or may not carry the hook; the call must not
        // panic either way.
        match registered_agents()[0].status() {
            HookInstallStatus::Installed(cmd) => assert!(cmd.contains("rustpods")),
            HookInstallStatus::NotInstalled | HookInstallStatus::ConfigUnreadable(_) => {}
        }
    }
}
