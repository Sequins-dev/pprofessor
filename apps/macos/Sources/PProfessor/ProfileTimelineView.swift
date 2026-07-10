import PProfessorKit
import SwiftUI

struct ProfileTimelineView: View {
    let timeline: ProfileTimeline
    let range: ClosedRange<Int64>
    let onRangeChange: (ClosedRange<Int64>, Bool) -> Void
    let onReset: () -> Void

    @State private var dragMode: DragMode?
    @State private var dragStartRange: ClosedRange<Int64> = 0...1
    @State private var latestDragRange: ClosedRange<Int64>?

    private enum DragMode {
        case lowerHandle
        case upperHandle
        case selection
    }

    var body: some View {
        VStack(spacing: 6) {
            HStack {
                Text("Timeline")
                    .font(.caption.weight(.semibold))
                Text(rangeDescription)
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
                Spacer()
                Button("Reset", action: onReset)
                    .font(.caption)
                    .buttonStyle(.plain)
                    .disabled(range.lowerBound == 0 && range.upperBound == timeline.durationNanos)
            }

            GeometryReader { geometry in
                timelineCanvas(size: geometry.size)
                    .contentShape(Rectangle())
                    .gesture(rangeGesture(width: geometry.size.width))
                    .onTapGesture(count: 2, perform: onReset)
            }
            .frame(height: 58)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(Color(NSColor.controlBackgroundColor))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Profile activity timeline")
        .accessibilityValue(rangeDescription)
    }

    private func timelineCanvas(size: CGSize) -> some View {
        let lowerX = xPosition(for: range.lowerBound, width: size.width)
        let upperX = xPosition(for: range.upperBound, width: size.width)
        return ZStack {
            Canvas { context, canvasSize in
                guard !timeline.buckets.isEmpty else { return }
                let step = canvasSize.width / CGFloat(max(1, timeline.buckets.count - 1))
                var line = Path()
                for (index, value) in timeline.buckets.enumerated() {
                    let point = CGPoint(
                        x: CGFloat(index) * step,
                        y: canvasSize.height - max(1, canvasSize.height * CGFloat(value))
                    )
                    if index == 0 { line.move(to: point) } else { line.addLine(to: point) }
                }
                var area = line
                area.addLine(to: CGPoint(x: canvasSize.width, y: canvasSize.height))
                area.addLine(to: CGPoint(x: 0, y: canvasSize.height))
                area.closeSubpath()
                context.fill(area, with: .color(Color.gray.opacity(0.18)))
                context.stroke(
                    line,
                    with: .color(Color.primary.opacity(0.72)),
                    style: StrokeStyle(lineWidth: 1.5, lineJoin: .round)
                )
            }

            HStack(spacing: 0) {
                Color.black.opacity(0.42).frame(width: max(0, lowerX))
                Color.clear.frame(width: max(0, upperX - lowerX))
                Color.black.opacity(0.42)
            }

            Rectangle()
                .fill(Color.primary.opacity(0.06))
                .frame(width: max(1, upperX - lowerX), height: size.height)
                .position(x: (lowerX + upperX) / 2, y: size.height / 2)

            Rectangle()
                .stroke(Color.secondary.opacity(0.9), lineWidth: 1)
                .frame(width: max(1, upperX - lowerX), height: size.height)
                .position(x: (lowerX + upperX) / 2, y: size.height / 2)

            handle(at: min(max(6, lowerX), max(6, size.width - 6)), height: size.height)
            handle(at: min(max(6, upperX), max(6, size.width - 6)), height: size.height)
        }
        .clipShape(RoundedRectangle(cornerRadius: 4))
        .overlay(RoundedRectangle(cornerRadius: 4).stroke(.separator, lineWidth: 1))
    }

    private func handle(at x: CGFloat, height: CGFloat) -> some View {
        RoundedRectangle(cornerRadius: 3)
            .fill(Color(NSColor.windowBackgroundColor).opacity(0.95))
            .frame(width: 12, height: height)
            .overlay {
                HStack(spacing: 2) {
                    Capsule().fill(Color.secondary).frame(width: 1, height: 16)
                    Capsule().fill(Color.secondary).frame(width: 1, height: 16)
                }
            }
            .overlay(RoundedRectangle(cornerRadius: 3).stroke(Color.secondary, lineWidth: 1))
            .position(x: x, y: height / 2)
    }

    private func rangeGesture(width: CGFloat) -> some Gesture {
        DragGesture(minimumDistance: 1, coordinateSpace: .local)
            .onChanged { value in
                guard width > 0 else { return }
                if dragMode == nil {
                    dragStartRange = range
                    latestDragRange = range
                    let lowerX = xPosition(for: range.lowerBound, width: width)
                    let upperX = xPosition(for: range.upperBound, width: width)
                    if abs(value.startLocation.x - lowerX) <= 10 {
                        dragMode = .lowerHandle
                    } else if abs(value.startLocation.x - upperX) <= 10 {
                        dragMode = .upperHandle
                    } else if value.startLocation.x > lowerX && value.startLocation.x < upperX {
                        dragMode = .selection
                    } else {
                        dragMode = abs(value.startLocation.x - lowerX) < abs(value.startLocation.x - upperX)
                            ? .lowerHandle : .upperHandle
                    }
                }
                let updated = updatedRange(for: value, width: width)
                latestDragRange = updated
                onRangeChange(updated, true)
            }
            .onEnded { value in
                guard width > 0 else { return }
                let updated = latestDragRange ?? updatedRange(for: value, width: width)
                onRangeChange(updated, false)
                dragMode = nil
                latestDragRange = nil
            }
    }

    private func updatedRange(for value: DragGesture.Value, width: CGFloat) -> ClosedRange<Int64> {
        let duration = max(1, timeline.durationNanos)
        let minimumSpan = max(1, duration / 1_000)
        switch dragMode {
        case .lowerHandle:
            let lower = min(time(at: value.location.x, width: width), dragStartRange.upperBound - minimumSpan)
            return max(0, lower)...dragStartRange.upperBound
        case .upperHandle:
            let upper = max(time(at: value.location.x, width: width), dragStartRange.lowerBound + minimumSpan)
            return dragStartRange.lowerBound...min(duration, upper)
        case .selection:
            let span = dragStartRange.upperBound - dragStartRange.lowerBound
            let delta = Int64(Double(value.translation.width / width) * Double(duration))
            let lower = min(max(0, dragStartRange.lowerBound + delta), duration - span)
            return lower...(lower + span)
        case nil:
            return range
        }
    }

    private func xPosition(for time: Int64, width: CGFloat) -> CGFloat {
        width * CGFloat(Double(time) / Double(max(1, timeline.durationNanos)))
    }

    private func time(at x: CGFloat, width: CGFloat) -> Int64 {
        let fraction = min(1, max(0, x / max(1, width)))
        return Int64(Double(timeline.durationNanos) * Double(fraction))
    }

    private var rangeDescription: String {
        "\(format(range.lowerBound)) – \(format(range.upperBound)) (\(format(range.upperBound - range.lowerBound)))"
    }

    private func format(_ nanoseconds: Int64) -> String {
        String(format: "%.3fs", Double(nanoseconds) / 1_000_000_000)
    }
}
