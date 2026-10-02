import SwiftUI

/// Two panes stacked vertically with a draggable divider.
/// Unlike `VSplitView`, the position is a binding, so callers can keep it across view rebuilds.
/// A `nil` height splits the space in half; dragging stores an explicit height, double-click resets.
struct VerticalSplit<Top: View, Bottom: View>: View {
    @Binding var topHeight: CGFloat?
    var minTop: CGFloat = 80
    var minBottom: CGFloat = 80
    @ViewBuilder var top: Top
    @ViewBuilder var bottom: Bottom

    @State private var dragStart: CGFloat?

    private let dividerHitArea: CGFloat = 8

    var body: some View {
        GeometryReader { geo in
            let height = clamped(topHeight ?? geo.size.height / 2, in: geo.size.height)
            VStack(spacing: 0) {
                top
                    .frame(maxWidth: .infinity)
                    .frame(height: height)
                Divider()
                bottom
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            .overlay(alignment: .top) {
                // Invisible, taller grab area centered on the 1pt divider.
                Color.clear
                    .frame(height: dividerHitArea)
                    .contentShape(Rectangle())
                    .pointerStyle(.rowResize)
                    .offset(y: height - dividerHitArea / 2)
                    .gesture(
                        DragGesture(minimumDistance: 1, coordinateSpace: .global)
                            .onChanged { value in
                                let start = dragStart ?? height
                                dragStart = start
                                topHeight = clamped(start + value.translation.height, in: geo.size.height)
                            }
                            .onEnded { _ in dragStart = nil }
                    )
                    .onTapGesture(count: 2) { topHeight = nil }
            }
        }
    }

    private func clamped(_ value: CGFloat, in total: CGFloat) -> CGFloat {
        let upper = max(minTop, total - minBottom)
        return min(max(value, minTop), upper)
    }
}
