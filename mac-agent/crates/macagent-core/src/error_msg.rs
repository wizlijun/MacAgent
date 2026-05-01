//! Error code → user-readable message (Chinese).

pub fn humanize(code: &str) -> &'static str {
    match code {
        "permission_denied"   => "Mac 未授予 Accessibility 权限",
        "window_gone"         => "目标窗口已关闭",
        "launch_timeout"      => "启动超时（5 秒未发现新窗口）",
        "launch_failed"       => "启动失败",
        "bundle_not_allowed"  => "App 不在白名单",
        "supervision_limit"   => "监管数已达上限（8）",
        "fit_denied"          => "窗口尺寸调整被拒绝",
        "encoder_failed"      => "硬件 H.264 编码器初始化失败",
        "no_focus"            => "目标窗口无法获得焦点",
        "throttled"           => "操作过于频繁",
        "network_error"       => "网络错误",
        _                     => "",
    }
}

#[cfg(test)]
mod tests {
    use super::humanize;

    #[test]
    fn known_codes() {
        assert_eq!(humanize("permission_denied"), "Mac 未授予 Accessibility 权限");
        assert_eq!(humanize("window_gone"), "目标窗口已关闭");
        assert_eq!(humanize("supervision_limit"), "监管数已达上限（8）");
    }

    #[test]
    fn unknown_code_returns_empty() {
        assert_eq!(humanize("xyz_unknown"), "");
        assert_eq!(humanize(""), "");
    }
}
