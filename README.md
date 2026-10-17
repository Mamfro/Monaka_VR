# Monaka VR

Monaka is a framework for (more) easily implementing VR mods into games. It reduces the amount of plumbing needed, and offers reusable structures (for head tracking, driving hands/arms/fingers, etc) that can be used by many games.

> **Monaka VR is in pre-alpha.** Expect bugs and missing pieces. Options and how games are run may
> still work differently from one version to the next. Game packs keep working across Monaka VR
> versions that speak the same pack contract; a pack made for an older contract shows as
> **Needs update** in the game list, with what to do.
>
> Contribution is encouraged, though please hold off a little bit longer until the API has further stabilised. 
> The license was chosen specifically to encourage open source collaboration instead of VR mods behind paywalls.

## Games

Monaka VR runs games through **game packs**: each game's VR support is made separately, as a pack
you add to Monaka VR (see [Game packs](#game-packs)). The window lists the games of the packs you
added, each with its status, what to set in the game first, any known bugs, and its options.
Currently only the **Steam** versions of games are supported.

## What you need

- A Windows PC that runs the game well. VR rendering is heavier than playing on a monitor.
- A headset that works with **SteamVR**. Monaka VR has *currently ONLY* been tested with the Steam Frame. Other SteamVR
  headsets should work but haven't been tried.
- **SteamVR** installed through Steam.

## Installing

1. Download `monaka_vr-<version>.zip` from the [Releases page](https://github.com/Mamfro/monaka_vr/releases)
   and unzip it anywhere, for example `C:\Games\Monaka VR`.
2. Run `monaka.exe`. The first time, Windows may say **"Windows protected your PC"**: Monaka VR
   isn't code-signed yet. Click **More info**, then **Run anyway**. Some antivirus programs may
   flag it too, because it loads its code into the game.

To update, unzip the new version over the old one. Your options and game packs are kept.

### Building from source (optional)

1. **Install Rust.** Download `rustup-init.exe` from [rustup.rs](https://rustup.rs) and run it. If
   it asks to install the Visual Studio C++ build tools, say yes and let it finish, then press
   Enter to accept the defaults.
2. **Get Monaka VR and the Monaka SDK.** Monaka VR builds on the SDK's crates, from a folder
   beside its own. On the [Monaka VR page](https://github.com/Mamfro/monaka_vr) and the
   [Monaka SDK page](https://github.com/Mamfro/monaka_sdk), click **Code** → **Download ZIP** and
   unzip both into one folder, named `monaka_vr` and `monaka_sdk`: for example `C:\vr\monaka_vr`
   and `C:\vr\monaka_sdk`. If you use git, clone both there instead.
3. **Open a terminal in the `monaka_vr` folder.** In File Explorer, open the folder, right-click an empty spot
   and pick **Open in Terminal**.
4. **Build it:**

   ```powershell
   cargo build --release
   ```

   The first build takes a few minutes. When it ends with `Finished`, the programs are in
   `target\release`.

To update later, download the new version over the old one and run `cargo build --release` again.

## Playing

1. Run `monaka.exe`.
2. Add the game pack for your game, once (see below).
3. Pick your game. The window shows what to set in the game's own options first, and the options
   Monaka VR has for that game. Options are saved as you change them.
4. Start **SteamVR** and turn on your headset (the window has a **Start SteamVR** button). Monaka
   VR's home opens in the headset: your games, their settings and the log, on panels around you.
   Hold **Start** to show or hide it; point a controller and pull the trigger to click.
5. Press **Play**, in the window or on the headset's Games panel. Under it, the window says what
   it will do: some games are started for you, others must be started first (**Start game** does
   that through Steam). **Alt+tab into the game** (that is important). Play.

### Game packs

A game pack is a game's VR support: a `.zip` (or a single `.dll`) from the people who made it, with
its own instructions. Press **Add game pack...** and pick it: the window says which games it adds
and where it says it comes from, and adds it only if you say OK. Game packs run their own code
inside the game, so only add packs from people you trust. To update a pack, add the new version;
**Remove pack** takes one out again. Your options for its games are kept either way.

**While playing:**

- **Recentre** with your headset's own recentre button (on the Steam Frame, hold the SteamVR
  button). Forward is wherever your headset faces.
- **Menus, maps and loading screens** appear on a flat screen in front of you. That's expected.
- **To stop VR**, press **Stop** in the window or **Stop game** in the headset, or quit the game.
  The game keeps running flat if you stop VR, and the headset goes back to Monaka VR's home.
  **Leave VR** closes the headset view; **Open in headset** brings it back.
- If VR stops on its own, it restarts after a few seconds. If it keeps stopping right after it
  starts, Monaka VR gives up and says so.

## Controllers

In games that use the VR controllers, they act as one Xbox controller. Thumbsticks, triggers,
bumpers and face buttons sit where an Xbox pad has them, and the menu button is Start. The game
shows its own gamepad button prompts. A real gamepad still works alongside them. To change a
button, open SteamVR's **Controller Bindings** for `monaka_viewer` while VR is running. In games
that don't use them yet, play with a gamepad, or mouse and keyboard; a wireless gamepad is easiest,
since you can't see a keyboard.

Quest controllers have no d-pad and no View button. Rest your left thumb on the left controller's
thumbrest to get them: while it rests there, the right thumbstick's four directions are the d-pad
and the menu button is View (Back). In SteamVR's bindings this is the "D-pad layer" action; bound to
another button instead, either thumbstick pushed is the d-pad while that button is held.

Valve Index controllers have no d-pad either: rest your left thumb on the left trackpad (without
pressing it) and the right thumbstick is the d-pad.

To try these layouts without the controllers, set "Controller test" on the Controls tab to Quest or
Index: your controllers then act like those (the buttons they lack are hidden), and your left grip
stands in for the Quest thumbrest or the Index trackpad touch.

## When something goes wrong

| What you see | What to do |
|---|---|
| `Start SteamVR and turn on your headset first.` | Start SteamVR, wait until it sees your headset, then press **Play** again. |
| `Build first: ...` | Run `cargo build --release` in the Monaka VR folder (see setup). |
| `Start the game first and load into gameplay.` | That game needs to be running before you press **Play**: start it (**Start game**) and load in. |
| `Attaching failed` or `The mod refused to start in this game` | The game probably updated and Monaka VR doesn't recognise the new version yet. It refuses instead of risking a crash. Check for a Monaka VR update. |
| `Another VR session is already running` | VR is still running from another terminal or the Monaka VR window. Stop that one first. |
| The headset shows a flat screen | You're in a menu or a loading screen. That's normal. It switches to 3D in gameplay. |
| The game paused | Some games pause when the window loses focus. Click into the game window once. |

Every run leaves a log: press **Open logs** in the window, or look in `%LOCALAPPDATA%\monaka\runs`
(newest folder, file ending in `.log`). If you report a problem, press **Save report**: it makes one
zip of the latest run's logs and your settings (with your Windows user name taken out) and opens
its folder. Attach that zip.

## Good to know

- Monaka VR is for **single-player**. Online play and co-op are not supported.
- Games made for a flat screen can cause motion sickness in VR more easily than games made for VR.
  Take breaks, and stop at the first sign of discomfort.
- Nothing in your game folder is changed. Monaka VR runs next to the game and switches itself off when you
  stop VR. Restarting the game clears it out completely.

## Licence

Monaka VR is free software under the [GNU General Public License v3.0](LICENSE). You can use, share and
change it. If you share a changed version, you have to share its source code under the same
licence.
