//! macagent-core
//!
//! 后续里程碑（M1+）会把 PairAuth、SessionManager、GuiCapture 等核心模块
//! 放到这里。M0 只暴露版本字符串供烟测使用。

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

    #[test]
    fn version_matches_semver_prefix() {
        assert!(version().starts_with("0."));
    }
}
