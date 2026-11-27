// Prints the CGWindowID of the first on-screen normal window of process $1, for screencapture -l.
import CoreGraphics
import Foundation

let pid = Int(CommandLine.arguments[1])!
let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
for window in windows
where window[kCGWindowOwnerPID as String] as? Int == pid && window[kCGWindowLayer as String] as? Int == 0 {
    print(window[kCGWindowNumber as String] as! Int)
    exit(0)
}
exit(1)
