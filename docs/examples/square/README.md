# Aurora Square

Aurora's default interface and layout with sharp `0px` square corners across the window chrome, cards, controls, menus, and dialogs.

Designed for desktop environments and compositors (such as KDE Plasma / KWin with forced corner rounding and native shadows) where Aurora's built-in rounded window corners would otherwise double-round or clip awkwardly.

- Uses Aurora's default layout (no `layout.toml` overrides).
- Sets `radius = 0`, disabling outer window corner cutouts and inner widget rounding.
- Includes a **Palette** option to pick any of Aurora's built-in color palettes (NEVKO, Midnight, Aurora, Classic, OLED, Daylight) while keeping square corners.
