import UIKit

/// UIKit `UIKey` → Mac key name + modifier flag mapping.
enum KeyMapper {
    static func modifiers(from flags: UIKeyModifierFlags) -> [KeyMod] {
        var out: [KeyMod] = []
        if flags.contains(.command)   { out.append(.cmd) }
        if flags.contains(.shift)     { out.append(.shift) }
        if flags.contains(.alternate) { out.append(.opt) }
        if flags.contains(.control)   { out.append(.ctrl) }
        return out
    }

    static func name(for usage: UIKeyboardHIDUsage) -> String? {
        switch usage {
        case .keyboardEscape:                      return "esc"
        case .keyboardTab:                         return "tab"
        case .keyboardReturnOrEnter, .keypadEnter: return "return"
        case .keyboardDeleteOrBackspace:           return "delete"
        case .keyboardUpArrow:                     return "up"
        case .keyboardDownArrow:                   return "down"
        case .keyboardLeftArrow:                   return "left"
        case .keyboardRightArrow:                  return "right"
        case .keyboardSpacebar:                    return "space"
        default:                                   return nil
        }
    }

    static func isSpecial(_ usage: UIKeyboardHIDUsage) -> Bool {
        name(for: usage) != nil
    }
}
