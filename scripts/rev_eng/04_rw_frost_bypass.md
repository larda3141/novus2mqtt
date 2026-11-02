# Reverse-engineering run #04 — Frost protection + bypass RW

**Goal:** find the registers controlling frost protection mode and bypass
enable. These are "mode enum" type settings (not continuous values) — useful to
confirm how enum-style writes look on the bus (1-byte value? packed?).

**Risk:** ⭐⭐ low. We cycle modes and restore. No impact on fans. Bypass toggle
only changes whether the VMC is *allowed* to open the bypass flap, not its
current physical state — at a per-cycle level this is harmless.
**Duration:** ~7 min.

## Prep

- `BUS_DUMP_FILE=bus_dump_run04.log`. Avoid HH:49.
- **Note current values BEFORE starting:**
  - Frost protection mode (Eco / Safe / Moisture heat exchanger)
  - Bypass: active or inactive
- Your NOVUS 300 variant determines which frost modes are available:
  - Non-F: Eco, Safe
  - F (enthalpy): Eco, Safe, Moisture heat exchanger

## Key reminder

- `m` — planned marker. **Press BEFORE you start each action.** Mn announces
  *"the next thing I do is …"*; bus traffic after Mn is the effect of action n.
- `b` — unplanned bookmark for anything unexpected.
- Counters reset on app start. Wait ~20 s after each change for the bus to
  settle before the next `m`.

## Actions

Workflow for every row: **(1) press `m` → (2) perform the action → (3) wait ~20 s**.

| # | Navigation & action (after pressing `m`) |
|---|------------------------------------------|
| – | Start app. **Don't touch panel.** Wait 60 s baseline. |
| **M1** | Do nothing — idle boundary. Wait ~30 s. |
| — | **Frost protection cycle** | |
| **M2** | Navigate: **Home → Settings → Frost protection**. |
| **M3** | Switch to a DIFFERENT mode than the current one (e.g. Eco → Safe). **Confirm with Enter.** |
| **M4** | If your unit is F-variant with 3 modes: switch to the 3rd mode. **Confirm.** (Otherwise do nothing — just wait 20 s.) |
| **M5** | Switch back to the ORIGINAL frost-protection mode. **Confirm.** |
| — | **Bypass toggle** | |
| **M6** | Navigate: **Home → Settings → By-pass**. |
| **M7** | Toggle the bypass activation (active → inactive or vice versa). **Confirm.** |
| **M8** | Toggle it BACK to the original state. **Confirm.** |
| **M9** | Exit to the **start/home** screen. |
| – | Wait 30 s after M9, then stop the app (`q`). |

**9 markers.**

## What I'll look for

- Compare the `0x85` frames in `[M3, M4)`, `[M4, M5)`, `[M5, M6)` — same
  register, different value bytes should tell us the enum encoding (likely
  1 byte: 0/1/2).
- Does bypass toggle (`[M7, M8)`, `[M8, M9)`) write to the same register as
  frost mode, or a different one?
- Does the bypass toggle trigger *immediate* bus activity beyond the write
  itself (e.g. the bypass actuator module waking up)?
