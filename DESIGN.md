# Chauffeur interface

Chauffeur is a compact desktop workspace for running and supervising agents. Preserve the
existing dark surfaces, system typography, Lucide icons and restrained accent colour.

## Hierarchy and density

- The current task and the next action take precedence over infrastructure details.
- Keep the first prompt prominent when creating a workspace. Optional setup belongs in a
  secondary disclosure; a missing prerequisite explains what is needed.
- A pending question needs room for its recent context. Fold the task list by default in a
  short chat pane or while input is pending, while keeping manual expansion available.
- In Settings, keep search outside the scrolling section list. Its height is 32 px, with
  12 px horizontal padding. Searching must not change its dimensions or show unrelated content.
- Use the shared components in `app/src/components/ui.tsx` and theme tokens in `app/sdk/theme.css`.
  Keep spacing on the existing 4 px scale except where a compact control needs an optical adjustment.

## Reading and interaction

- Normal reading text, including secondary text and placeholders, needs 4.5:1 contrast on its
  actual surface. Decorative marks and disabled controls are distinct from secondary reading text.
- Keep a visible keyboard focus. A field's visible label must name its control; hints describe it.
- A modal contains focus, makes its background inert and restores its trigger when closed.
  Nested dialogs and portalled pickers belong to the active interaction.
- Standard icon buttons are at least 24 × 24 px. Smaller inline controls need adequate spacing
  or an equivalent full-sized control; 44 px is a touch enhancement, not the desktop baseline.
- The native desktop minimum is 900 × 560. Test this size and 1280 × 800, including short split
  panes. The separate mobile client has its own layout; do not stack the desktop shell onto a phone.

## States and recovery

- Empty states say what is missing and offer the next action when one is useful.
- A failure names what failed and how to recover. Put technical detail in a disclosure when it
  would overwhelm the main message. Reconnecting preserves drafts and pending work.
- Archived work is explicitly archived, not described as loading or not ready. Preserve immediate
  archival and explicit restoration; explain the snapshot retention in the archive's context.
- Search results name both the option and its section, and open the relevant setting.

## Verification

Render the changed states on an isolated daemon. Check keyboard order, focus restoration,
loading/empty/error states and narrow desktop layouts. Use real computed dimensions and contrast
ratios, not screenshots alone. Never use the user's real workspaces for interaction tests.
