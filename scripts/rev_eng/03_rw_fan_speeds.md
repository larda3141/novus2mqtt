# Reverse-engineering run #03 — Fan speed RW (FS1 and FS3, ±1%)

**Goal:** find the registers that hold FS1/FS2/FS3 percentages. Fan speeds are
almost certainly bus-wide state (the fan module needs to know them), so this is
the highest-confidence test for *actually seeing* a write on the bus.

**Risk:** ⭐⭐ low. Changes are ±1% and reverted within the script. You may
hear a brief change in fan noise. No safety/thermal impact.
**Duration:** ~6 min.

## Prep

- `BUS_DUMP_FILE=bus_dump_run03.log`.
- Avoid HH:49.
- **Note current values BEFORE starting:**
  - FS1 (e.g. 20%)
  - FS2 (e.g. 50%) — we will NOT change this one, just record
  - FS3 (e.g. 80%)
- Constraint to remember: `17% < FS1 < FS2 < FS3 < 100%`. Our ±1% changes stay
  safely inside that.

## Key reminder

- `m` — planned marker. **Press BEFORE you start each action.** Mn announces
  *"the next thing I do is …"*; bus traffic after Mn is the effect of action n.
- `b` — unplanned bookmark for anything unexpected (fan noise, etc.).
- Counters reset on app start. Wait ~20 s after each change for fans to react
  and the bus to settle before the next `m`.

## Actions

Workflow for every row: **(1) press `m` → (2) perform the action → (3) wait ~20 s**.

| # | Navigation & action (after pressing `m`) |
|---|------------------------------------------|
| – | Start app. **Don't touch panel.** Wait 60 s baseline. |
| **M1** | Do nothing — idle boundary. Wait ~30 s. |
| — | **FS1: +1 %, then revert** | |
| **M2** | Navigate: **Home → Settings → Fan speeds → FS1**. |
| **M3** | Increase FS1 by exactly 1 % (e.g. 20 → 21). **Confirm with Enter.** Listen for fan change. |
| **M4** | Decrease FS1 by 1 % back to the original (e.g. 21 → 20). **Confirm with Enter.** |
| — | **FS3: +1 %, then revert** | |
| **M5** | Navigate: **Home → Settings → Fan speeds → FS3**. |
| **M6** | Increase FS3 by exactly 1 % (e.g. 80 → 81). **Confirm with Enter.** |
| **M7** | Decrease FS3 by 1 % back to the original (e.g. 81 → 80). **Confirm with Enter.** |
| **M8** | Exit to the **start/home** screen. |
| – | Wait 30 s after M8, then stop the app (`q`). |

**8 markers.**

## What I'll look for

- `0x85` writes **from the TFT** (not `>` frames from us) appearing in
  `[M3, M4)`, `[M4, M5)`, `[M6, M7)`, `[M7, M8)`.
- The specific register address that holds FS1 vs FS3.
- Whether FS2 gets re-broadcast too (packed multi-register write).
- Whether the fan-slave module (addr `0x02` / `0x04` area) responds differently.
- Timing: how long after the Enter press does the bus reflect the change?
