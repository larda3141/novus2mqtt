# Reverse-engineering run #01 — Information menu (READ-ONLY)

**Goal:** capture the bus traffic produced by simply *browsing* every submenu of the
`Information` main menu on the TFT touch panel. No values are changed.

**Unit state assumed:** running (fans spinning). **Panel type:** TFT touch panel.
**Prior expectation:** no bus traffic on TFT ops — so the interesting outcome is
whether anything *does* appear. Any difference from the baseline window is signal.

## Prep (do this BEFORE you start the app)

1. **Avoid the hourly time-sync window.** Our AUTO_SYNC_TIME fires at HH:49 and
   produces ~20 s of contaminated traffic (the `0x88` storm). Plan the run for
   HH:00 → HH:40, or set `AUTO_SYNC_TIME=false` in `.env` for this session.
2. Make sure `BUS_DUMP_FILE=bus_dump_run01.log` is set in `.env`.
3. Be physically at the TFT panel before starting.
4. Have the app terminal visible — you'll want to see the marker confirmations.

## Key reminder

- `m` — planned marker. **Press BEFORE you start each action.** The marker
  announces *"the next thing I do is …"*, so the bus traffic *after* Mn is the
  effect of action n. This way you don't have to guess how long reactive
  traffic will last.
- `b` — unplanned bookmark for anything unexpected (fan noise change, display
  flicker, VMC doing something on its own).
- Counters reset on app start. Wait ~10 s after each action's effects settle
  before pressing the next `m`.

## Actions

Workflow for every row: **(1) press `m` → (2) perform the action → (3) wait ~10 s**.

| # | Navigation & action (after pressing `m`) |
|---|------------------------------------------|
| – | Start the app with the dump active. **Do not touch the panel.** Wait 60 s — this is the pure baseline, no marker yet. |
| **M1** | Do nothing — this is the "idle" window boundary. Wait ~30 s. |
| **M2** | Wake the display: touch the screen once. Stay on the **start/home** screen. |
| **M3** | Navigate: **Home → Information** (open the main Information menu). |
| **M4** | Navigate: **Home → Information → Current message**. Read the screen. |
| **M5** | Navigate: **Home → Information → Last messages**. |
| **M6** | Navigate: **Home → Information → Fan speeds** (just viewing, no change). |
| **M7** | Navigate: **Home → Information → Current status of the device**. |
| **M8** | Navigate: **Home → Information → Software versions**. |
| **M9** | Navigate: **Home → Information → Connected devices**. |
| **M10** | Navigate: **Home → Information → Filter age**. |
| **M11** | Navigate: **Home → Information → Operating hours**. |
| **M12** | Exit all the way back to the **start/home** screen. Let it idle. |
| – | Wait 30 s after M12, then stop the app cleanly (`q`). |

Total: **12 markers**, ~5 minutes. Use `b` anywhere if something unexpected happens.

## What I'll look for in the dump afterwards

The window `[Mn, Mn+1)` contains the effect of action n. For each window:
- Any new `0x85` register addresses that didn't appear in the baseline window.
- Any change in the periodic broadcast rhythm.
- Any non-`0x84`/`0x85` frames (especially `0x86`/`0x88`).
- Whether the panel triggers a read request of some sort when you enter a submenu.
