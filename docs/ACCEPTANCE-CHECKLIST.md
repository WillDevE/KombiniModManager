# Real-game acceptance checklist

Run by a person on a real Deadlock install (automated tests never touch a real game folder).
Back up `game\citadel\gameinfo.gi` and `game\citadel\addons` first. Tick each item; note the build.

Build: ________  Date: ________  Windows: 10 / 11  DPI: 100 / 125 / 150 / 200 %

## First run
- [ ] Deadlock is detected (or the folder picker accepts the install root and rejects other folders).
- [ ] Existing addons show the "not managed" banner; Import adopts them keeping order and enabled state.
- [ ] Another manager's files (DMM, Grimoire `addons1`) show the warning banner.
- [ ] With DMM or Grimoire data present, Import lists that manager's library; "Import from DMM/Grimoire" brings in every mod (names, GameBanana links, order, enabled state) and profile, activates the current one, and moves the old files to `<data>/imports/` so nothing loads twice.

## Install, enable, order
- [ ] Install from Discover: progress in Downloads, installs automatically, success toast.
- [ ] A mod with several files asks which file; flagged/archived files are not installable automatically.
- [ ] Drop a `.zip`, `.7z`, `.rar` and a loose `.vpk` on the window: each installs.
- [ ] Toggle a mod: switch flips at once with a spinner; `addons` shows `pakNN_dir.vpk` / parked names.
- [ ] Drag to reorder: preview follows the cursor, drop line shows, `pak01` is the top mod.
- [ ] 100+ enabled mods spill into `addons2…`; `gameinfo.gi` lists every used shard.
- [ ] Variants: switching variants changes the mounted file.
- [ ] Ctrl/Shift-click selects several mods; the action bar enables/disables/uninstalls them.
- [ ] Group the selection; the group switch enables/disables all members; rename and delete work.
- [ ] Merge two overlapping mods: one new mod, the higher one's files win, originals disabled.

## Launch
- [ ] Launch Modded: Preparing → Starting Steam → Waiting → Running; mods visible in game.
- [ ] No `user_keys_default` / keybinding fatal error at startup.
- [ ] While running, switches are disabled and changes are blocked.
- [ ] Close the game: "closed after …" toast; a crash shows the "Find broken mod" banner.
- [ ] Launch Vanilla: game starts unmodded; managed block restored after it exits.
- [ ] Steam not starting the game hits the timeout banner (does not claim "running").

## Safety and recovery
- [ ] Kill the app during a large enable: next start recovers (toast) and files match the library.
- [ ] Delete a mounted file in Explorer: "Addons changed" toast; Reapply restores it.
- [ ] Steam "Verify integrity" resets `gameinfo.gi`: next start re-applies it.
- [ ] Read-only `gameinfo.gi`: health error with a working "Clear read-only" fix.
- [ ] Uninstall takes a snapshot; restoring it brings the mod back.
- [ ] Export a snapshot to `.kmmsnap`, delete it, import the file and restore it.

## Profiles, sharing, updates
- [ ] Create, switch, delete profiles; switching changes mounted files.
- [ ] Share code round-trips into a fresh install and queues missing mods.
- [ ] Export a mod bundle and import it on a second data dir; nothing duplicated.
- [ ] An outdated GameBanana mod shows "Update"; updating keeps enabled state and position.
- [ ] "Update all" queues every update; "Skip this version" hides the update until a newer file.
- [ ] `kombini://` link (after enabling in Settings) opens the running app and queues the install.

## Mod issues
- [ ] After a game update, an older mod with a broken camera or outdated hero data shows "Fix available".
- [ ] Mod issues → Fix repairs it; a "Before repairing" snapshot exists; the game starts with the mod.
- [ ] A mod with nothing fixable shows no chip; Rescan reports "No fixable mod issues found."
- [ ] Settings → Credits names Grimoire and vpkmerge with working links.
- [ ] Settings → Game shows the installed version, its build date and the last major update.
- [ ] A GameBanana mod uploaded before the last major update shows "May be outdated"; the Outdated
      filter lists it; its details explain which update it predates and link the patch notes.
- [ ] Discover cards of mods last updated before the last major update carry the badge.
- [ ] Offline: the version still shows and the built-in update list is used.

## Heroes, versions, customizing
- [ ] Heroes opens on a grid in the same order as the game's hero select, with portraits and names.
- [ ] A hero with two skins on shows the amber stack badge; "Keep only this" leaves one on.
- [ ] Heroes lists each hero with skins; "Use" enables one skin and disables the hero's others.
- [ ] Enabling a second skin for a hero from My Mods asks first (unless turned off in Settings).
- [ ] After updating a mod, Versions lists the old one; Restore switches back without downloading.
- [ ] The changelog shows the author's update posts; newer ones are marked.
- [ ] Find broken mod tests the "May be outdated" mods first.
- [ ] Customize: changing the crosshair updates the preview; Save writes the block to autoexec.cfg and
      the crosshair shows in game; Stop overriding removes only that block.
- [ ] Load from game settings reads the crosshair set in Deadlock's own editor.
- [ ] Adding a library command puts it in the Kombini commands block; it shows "Added".
- [ ] A sound mod's details list its sounds; Play and Original both play; Stop stops.
- [ ] Discover → Add from link with a collection link lists its mods; Install queues the downloads.
- [ ] A mod's screenshots show as a strip; clicking opens the lightbox with next/previous.

## Feel
- [ ] Every button shows pressed/loading/disabled feedback; no silent failures.
- [ ] VPK inspector on a big mod pages instantly; Discover scrolls smoothly.
- [ ] Keyboard: tab through the top bar and settings; text inputs work with IME/paste.
