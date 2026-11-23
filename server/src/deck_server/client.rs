//! `deck-server` has no TUI client. This stands in for `src/client` so the
//! shared server modules compile; it keeps only what they reach outside the
//! client's own code.

pub fn run_client() -> std::io::Result<()> {
    Err(std::io::Error::other("deck-server has no TUI client"))
}
