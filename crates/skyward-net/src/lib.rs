pub mod dhcp;
pub mod dns;
pub mod download;
pub mod github;
pub mod http;
#[cfg(target_os = "android")]
pub mod iface;
pub mod reach;
pub mod supplicant;
pub mod wlan;

/// Why a server gave no answer to a question asked over the joined network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unanswered {
    /// The network joined wants a login in a browser first.
    Login,
    /// The network joined does not reach the internet: why.
    Offline(String),
    /// The server could not be asked, or its answer not read: why.
    Failed(String),
}

/// Work stopped because it was asked to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stopped;

impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("stopped as asked")
    }
}

impl std::error::Error for Stopped {}
