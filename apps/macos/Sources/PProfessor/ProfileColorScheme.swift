import SwiftUI

struct ProfileColorScheme {
    func colorForRatio(_ ratio: Double) -> Color {
        let clampedRatio = min(max(ratio, 0), 1)
        let hue = 0.6 // Blue hue
        let saturation = 0.05 + 0.95 * clampedRatio
        let brightness = 0.3 + 0.7 * clampedRatio
        return Color(hue: hue, saturation: saturation, brightness: brightness)
    }
}
