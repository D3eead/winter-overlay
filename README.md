# winter-overlay
Wayland snow overlay for Hyprland: snow falls, piles up on window tops, slides off, melts, and can be cleared with a snowplow.
Snow: User Guide

Snow falls over the whole screen, piles up on the top edge of windows, slides off, melts, and the snow at the bottom of the screen can be cleared with a snowplow.

1. Requirements
Arch Linux (or a derivative, e.g. CachyOS) running Hyprland
Packages: rustup, gtk4, gtk4-layer-shell
sudo pacman -S rustup gtk4 gtk4-layer-shell
rustup default stable
rustup component add rust-analyzer
2. Create the project (if you haven't yet)
cargo new snow
cd snow

Cargo.toml:

toml
[package]
name = "snow"
version = "0.1.0"
edition = "2021"
description = "Wayland snow overlay for Hyprland: snow falls, piles up on window tops, slides off, melts, and can be cleared with a snowplow."

[dependencies]
gtk4 = "0.9"
gtk4-layer-shell = "0.4"
serde_json = "1"
rand = "0.8"

Paste the full code into src/main.rs and save (Ctrl+S).

3. Run
cd ~/snow
cargo run --release

The first build takes a couple of minutes, later runs start instantly. To stop it: Ctrl+C in the terminal, or pkill snow from anywhere.

4. Autostart with Hyprland

Build once: cargo build --release. Then add this to ~/.config/hypr/hyprland.conf:

exec-once = /home/d3eead/snow/target/release/snow

After you rebuild the code the binary updates itself, so the path stays the same.

5. How to use it
Snow falls on its own across the whole screen.
Windows: snow lands on the top edge of any window on visible workspaces. When a pile gets too tall, it slides and drops off the sides.
Shake snow off a window: move the window sharply. For floating windows, drag with Super + left mouse button (needs bindm = SUPER, mouse:272, movewindow). The pile falls off.
Floating mode toggles with Super + Shift + F (the togglefloating bind you added).
Snowplow sits at the bottom left. Click it: it drives right, clears all snow from the bottom of the screen, and returns.
Snow on windows and at the bottom melts over time. Clicks pass through the overlay, only the plow itself is clickable.
6. Settings

Everything is at the top of main.rs, each setting has a comment. The main ones:

I want	Setting
More or less snow	SPAWN (2 → 15)
Blizzard	raise FALL_MIN / FALL_MAX
Taller piles	raise MAXH, SLOPE
Faster sliding	raise SLIDE, SLIDE_PASSES, lower SLOPE
No melting at the bottom	MELT = 0.0
No melting on windows	WIN_MELT = 0.0
Smoother animation	FPS_MS = 16 and halve SPAWN
Lower CPU load	FPS_MS = 50, MAX_FLAKES = 3000

After editing: Ctrl+S, then cargo run --release again.

7. Troubleshooting
Snow falls but doesn't pile on windows: the overlay can't reach Hyprland. Run it from a terminal inside the Hyprland session, not over SSH or another environment.
cannot find value error: a constant is missing at the top of the file. Check that all const lines are there.
unclosed delimiter: the file was copied incompletely. The end of the file should be app.run(); }.
Doesn't start from autostart: use the full path to the binary and make sure you ran cargo build --release first.
Multiple monitors: window positions are counted from point 0,0, so snow may sit with an offset on the second monitor.
Other compositors (Sway, KDE, GNOME): won't work, the poll() function needs to be rewritten.
8. VSCodium extension
codium --install-extension rust-lang.rust-analyzer
