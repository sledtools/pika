# Compose Accessibility Checks

Use this guide to catch common accessibility regressions in Material 3 Compose screens.

## Semantics and Labels

- Prefer Material components first for strong built-in semantics
- Add meaningful labels for icon-only actions and images
- Ensure custom click targets expose proper semantics roles and states
- Keep headings and logical grouping understandable for screen readers

## Touch and Readability

- Ensure controls remain comfortably tappable on all layouts
- Avoid fixed heights that break under larger font scales
- Verify truncation strategy for larger text and narrower windows

## Color and Contrast

- Keep semantic foreground/background pairs (`on*` with matching container/surface)
- Re-check contrast after custom theme overrides
- Avoid relying on color alone for selected/error/success meaning

## Adaptive-Specific Checks

- Confirm traversal order remains logical across compact/medium/expanded layouts
- Validate nav shell changes (bar/rail/drawer) do not hide critical actions
- Check foldable layouts for clipped or inaccessible controls near hinge areas

## Source Links

- https://developer.android.com/develop/ui/compose/accessibility
- https://developer.android.com/develop/ui/compose/accessibility/semantics
- https://developer.android.com/develop/ui/compose/designsystems/material3
