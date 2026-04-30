//! macagent-core
//!
//! 后续里程碑会持续把核心模块加到这里。当前 M1：PairAuth + ctrl 消息。

pub mod ctrl_msg;
pub mod pair_auth;
pub mod rtc_peer;
pub mod signaling;
pub mod socket_proto;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version_is_non_empty() {
        assert!(!version().is_empty());
    }
}
