import Foundation

enum ErrorMessage {
    static func humanize(_ code: String) -> String {
        switch code {
        case "permission_denied":   return "Mac 未授予 Accessibility 权限"
        case "window_gone":         return "目标窗口已关闭"
        case "launch_timeout":      return "启动超时（5 秒未发现新窗口）"
        case "launch_failed":       return "启动失败"
        case "bundle_not_allowed":  return "App 不在白名单"
        case "supervision_limit":   return "监管数已达上限（8）"
        case "fit_denied":          return "窗口尺寸调整被拒绝"
        case "encoder_failed":      return "硬件 H.264 编码器初始化失败"
        case "no_focus":            return "目标窗口无法获得焦点"
        case "throttled":           return "操作过于频繁"
        case "network_error":       return "网络错误"
        default:                    return ""
        }
    }

    /// "humanize or fall back to <code> + (message ?? '')"
    static func describe(code: String, message: String? = nil) -> String {
        let h = humanize(code)
        if !h.isEmpty { return message.map { "\(h)（\($0)）" } ?? h }
        return message.map { "未知错误：\(code)（\($0)）" } ?? "未知错误：\(code)"
    }
}
