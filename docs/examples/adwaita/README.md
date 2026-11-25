# Adwaita for Aurora

A separate theme package inspired by [tkashkin's Adwaita for Steam](https://github.com/tkashkin/Adwaita-for-Steam) and GNOME's Adwaita design.

Adwaita copies the compact chrome of the AdwSteamGtk reference: a single 44 px header with history arrows, centered navigation, avatar and utility icons, a hamburger menu and circular close button, plus a slim 28 px footer. The library content stays in Aurora's default layout, including What's New, the large Continue Playing banner and Recent Games. There are no custom library nodes or replacement shelves. Adwaita Sans is bundled for consistent typography.

**Adwaita** is bundled with Aurora and installed automatically on startup — open **Settings → Appearance** and activate **Adwaita** (or install a custom `adwaita-theme.zip` via **Install Theme…**). Appearance and accent default to **System**: on GNOME, Aurora follows the desktop's light/dark, accent and high-contrast settings while it is open. Choose **Dark** or **Light** and one of nine accent colors to override either preference independently. Without desktop settings, the fallback is light with blue accents. Add a Game, product activation, updates, and settings are available through the Aurora menu.

Version 1.2 needs an Aurora build with the native library bridge and updated chrome actions. On Linux it uses installed GTK4 and libadwaita 1.6+ for real stylesheet colors, accent conversions, control metrics and GTK-rendered secondary button backgrounds. These libraries load in an isolated helper; no Python package or separate helper executable is required. Normal, hovered and pressed artwork retains its native corners. Navigation, text and input handling remain in egui; this is not a complete GTK widget interface or a Millennium CSS skin. Bundled Adwaita Sans remains in use.

If the libraries or display are unavailable, Aurora uses its portable Adwaita profile. The startup log identifies successfully loaded GTK/libadwaita versions. `layout.toml` defines only the surrounding chrome; the empty library layout deliberately falls back to Aurora's normal content. Window dragging and double-click maximization remain available; minimize and maximize are also in the hamburger menu.

The right-hand avatar is a rounded-square profile button. Clicking it opens your profile directly. Version 1.2.1 needs the renderer fix that prevents the avatar image from intercepting the enclosing button's click; after that update, its action and rounding are editable in the ZIP.

Reference: https://steambrew.app/theme/7dzdgNotKWgNmQYXc6A0

Chrome reference: [AdwSteamGtk](https://github.com/Foldex/AdwSteamGtk), following the screenshot supplied by the user.

Palette adapted from Adwaita for Steam (MIT; see UPSTREAM-LICENSE.txt). Fonts are Adwaita Sans by the Inter Project Authors, Renzhi Li, Jamie Gravendeel, and Florian Müllner (SIL OFL 1.1; see assets/fonts/OFL.txt). The included static regular and bold font instances were generated from the installed Adwaita Sans variable font at optical size 14. Private-use character mappings are omitted so Aurora can use its bundled icon font without glyph collisions.
