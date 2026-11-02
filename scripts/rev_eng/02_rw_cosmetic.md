# Reverse-engineering run #02 — Cosmetic RW (screen dimming + language)

**Goal:** first write test, on settings that can't harm ventilation. If the
panel writes to the bus at all for settings changes, this is where we'll see it
cleanly. If the bus stays silent, we learn that TFT UI settings are local to the
panel and the bus doesn't care — also a useful result.

**Risk:** ⭐ very low. Settings are reverted within the script. No impact on fans,
bypass, or temperatures.
**Duration:** ~5 min.

## Prep

- Same as script 01: avoid HH:49 window, `BUS_DUMP_FILE=bus_dump_run02.log`.
- **Note the current values BEFORE starting** (on a piece of paper):
  - Current language (German / English / …)
  - Screen dimming degree (e.g. 75%)
  - Screen dimming duration (e.g. 1 min)

## Key reminder

- `m` — planned marker. **Press BEFORE you start each action.** Mn announces
  *"the next thing I do is …"*; traffic after Mn is the effect of action n.
- `b` — unplanned bookmark for anything unexpected.
- Counters reset on app start. After each action, wait ~10 s for reactive
  traffic to settle before the next `m`.

## Actions

Workflow for every row: **(1) press `m` → (2) perform the action → (3) wait ~10 s**.

| # | Navigation & action (after pressing `m`) |
|---|------------------------------------------|
| – | Start app. **Don't touch panel.** Wait 60 s baseline, no marker yet. |
| **M1** | Do nothing — idle boundary. Wait ~30 s. |
| — | **Language change + revert** | |
| **M2** | Wake the panel, navigate: **Home → Settings → Choose language**. |
| **M3** | On the Choose language screen, switch to the OTHER option (DE↔EN). **Confirm with Enter.** |
| **M4** | Switch language BACK to the original. **Confirm with Enter.** |
| — | **Screen dimming change + revert** | |
| **M5** | Navigate: **Home → Settings → Screen dimming**. |
| **M6** | On the Screen dimming screen, change **Degree of dimming** by ONE step (e.g. 75 → 80%). **Confirm.** |
| **M7** | Revert **Degree of dimming** to the original value. **Confirm.** |
| **M8** | Exit back to the **start/home** screen. |
| – | Wait 30 s after M8, then stop the app (`q`). |

**8 markers.** If you spot anything unexpected (flicker, fan change), hit `b` immediately.

## What I'll look for

- Any `>` frames from us — there shouldn't be any (we didn't issue a command).
- New `0x85` register addresses appearing in `[M3, M4)` / `[M4, M5)` / `[M6, M7)` / `[M7, M8)`
  that weren't in the baseline `[start, M1]`.
- Whether the TFT writes language/dimming state onto the bus, or keeps it local.
