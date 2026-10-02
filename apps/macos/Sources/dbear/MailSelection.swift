import SwiftUI

/// Mail-style selection: soft gray rounded background, icon + title tinted with the accent color.
/// SwiftUI's native List selection can't be restyled, so lists using this manage selection themselves.
struct MailSelectionRow: ViewModifier {
    let isSelected: Bool
    let action: () -> Void

    /// The sidebar List hosts row content 6pt in from each side of the row cell, and
    /// clicks there never reach SwiftUI. The highlight is inset to match, so every
    /// visible part of a row is clickable.
    private static let highlightInset: CGFloat = 10 + 6

    func body(content: Content) -> some View {
        content
            .labelStyle(MailRowLabelStyle())
            .foregroundStyle(isSelected ? AnyShapeStyle(.tint) : AnyShapeStyle(.primary))
            .padding(.horizontal, 4)
            // Fill the row's full width and height (not just the text) so clicks anywhere
            // in the row select it. Zero insets let the content take the full row height.
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
            .contentShape(Rectangle())
            .onTapGesture(perform: action)
            .listRowInsets(EdgeInsets())
            .accessibilityElement(children: .combine)
            .listRowBackground(
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .fill(.primary.opacity(isSelected ? 0.1 : 0))
                    .padding(.horizontal, Self.highlightInset)
            )
            .accessibilityAddTraits(isSelected ? [.isButton, .isSelected] : .isButton)
    }
}

/// Sidebar lists color `Label` icons themselves; this lets the icon inherit the row's
/// foreground style so it turns accent-colored with the title when selected.
struct MailRowLabelStyle: LabelStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 6) {
            configuration.icon
                .frame(width: 22, alignment: .center)
            configuration.title
        }
    }
}

extension View {
    func mailSelection(_ isSelected: Bool, action: @escaping () -> Void) -> some View {
        modifier(MailSelectionRow(isSelected: isSelected, action: action))
    }

    /// ↑/↓ move the selection through `ids` (the visible rows, in order).
    func arrowKeySelection<ID: Equatable>(
        ids: [ID], selected: ID?, focus: FocusState<Bool>.Binding, select: @escaping (ID) -> Void
    ) -> some View {
        self
            .focusable()
            .focusEffectDisabled()
            .focused(focus)
            .onKeyPress(.downArrow) { move(1, ids: ids, selected: selected, select: select) }
            .onKeyPress(.upArrow) { move(-1, ids: ids, selected: selected, select: select) }
    }
}

private func move<ID: Equatable>(_ delta: Int, ids: [ID], selected: ID?, select: (ID) -> Void) -> KeyPress.Result {
    guard !ids.isEmpty else { return .ignored }
    let next: Int
    if let selected, let index = ids.firstIndex(of: selected) {
        next = min(max(index + delta, 0), ids.count - 1)
    } else {
        next = delta > 0 ? 0 : ids.count - 1
    }
    select(ids[next])
    return .handled
}
