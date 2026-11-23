//! `deck-server` does not update itself (Deck ships it). Only the version
//! type and the no-op hooks the shared app/server code calls remain.

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.strip_prefix('v').unwrap_or(s);
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return None;
        }
        Some(Self {
            major: parts[0].parse().ok()?,
            minor: parts[1].parse().ok()?,
            patch: parts[2].parse().ok()?,
        })
    }

    pub fn current() -> Self {
        Self::parse(crate::build_info::BASE_VERSION).expect("invalid CARGO_PKG_VERSION")
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

pub(crate) fn update_install_command() -> &'static str {
    ""
}

pub(crate) fn update_install_instruction(_install_command: &str) -> String {
    "update Netrunner Deck to get a newer deck-server".to_string()
}

/// No background update check: Deck's own updater ships the server.
pub fn auto_update(_events: tokio::sync::mpsc::Sender<crate::events::AppEvent>) {}
