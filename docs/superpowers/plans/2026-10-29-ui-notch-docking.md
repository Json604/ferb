# FERB pill ↔ hardware notch docking

## Goal
Owner request, 2026-10-29: dropping the pill directly onto the Mac notch swallows it
into a docked notch. Dragging immediately below the notch pulls it back into a movable pill.

## Architecture
Extend the existing native presentation in PanelCoordinator, FerbIndicatorControl,
NotchBackdrop and NotchPresentation. Retain the single-window expanded pill/chat
attachment. Persist docking mode with the existing display/edge preferences.
The hardware cutout geometry determines the drop target and pull-out strip.

## Spec
- Drop onto the physical notch to dock, with a brief connected swallow transition.
- Docked FERB opens chat from the camera notch; no logo is drawn over the camera.
- A drag from beneath the notch or its noninteractive chat header tears out the pill.
- Preserve pointer anchoring and the combined pill/chat drag.
- Reduce Motion uses an immediate/gentle transition.
- Displays without a physical notch keep the edge-pill behavior.

## Global Constraints
All code stays on ui/ux; no merge to main. No visual inspection, screenshots, or
automated UI interaction: the owner explicitly reserves appearance verification.
No daemon changes for this feature. Do not interrupt active FERB tasks.

## Review Focus
Interrupted transitions; reopening/closing while docked; persisted mode after display
changes; hardware-camera avoidance; controls and text selection not stolen by drag;
single-window movement and reduced-motion behavior.

## Task 1 — Native docking and pull-out
Files: UI coordinator/control/backdrop/presentation, placement helper/model as needed,
and focused NotchCoreTests.
1. Add failing tests for physical-notch drop targeting, docking state and pull-out geometry.
2. Implement docking persistence, swallow transition, docked expansion and drag-out strip.
3. Run focused Swift tests and full Swift suite with scripts/swiftpm-anonymous.sh.
4. Fresh read-only review and focused fixes. Keep changes on ui/ux.

## Self-Review
The owner has requested implementation directly. The existing Python regression run
covers the preceding preview work; this UI-only follow-up requires Swift validation.
Visual motion and feel remain owner acceptance, not an automated claim.
