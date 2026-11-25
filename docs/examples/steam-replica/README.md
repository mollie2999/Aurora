# Aurora Desktop theme

A declarative theme modeled on the modern Steam desktop client, using its two-row header, compact library sidebar, news band, game shelves, and bottom status bar.

- 80 px header with Aurora/View/Friends/Games/Help menus, utility buttons, account menu, window controls, and underlined navigation.
- 276 px library sidebar with Home and a Collections picker, compact filters, a dark inset search/filter field, 25 px game rows, and collapsible Recent/month/Never played groups sorted by last play. The Collections picker filters existing collections and offers All Games and New Collection.
- Fixed 318 × 179 news artwork, 179 × 268 recent capsules, a 376 × 268 featured tile with a playtime pane, and 224 × 336 library capsules.
- Horizontal news and recent-game shelves with working paging arrows; cards keep their proportions in narrower windows.
- 40 px footer with Add a Game, centered 420 px download area, and Friends & Chat.
- Modern 2023+, Classic 2018 Blue, and VGUI 2010 Olive palette options. Header accents, utility buttons, account surfaces, sidebar controls, shelf gradients, and separators all follow the selected palette. Changing an accent keeps neutral surfaces in their chosen era.

**Aurora Desktop** is bundled with Aurora and installed automatically on startup — open **Settings → Appearance** and activate **Aurora Desktop** (or install a custom `steam-replica-theme.zip` via **Install Theme…**). Choose a palette under **Theme options → Steam Era**. Version 2.3.0 uses the updated theme engine's library Collections action; update Aurora alongside the theme for that control to work.

The shell and library home are the closest matches. Each era uses the same modern layout with its own complete palette; the older options do not reproduce historical Steam layouts. Game detail pages and other built-in pages retain Aurora's layout. Aurora's existing card hover effects remain in use: this package does not ship a card-overlay script because the declarative library renderer does not invoke that hook. The header's highlighted people button opens Friends & Chat.

All layout colors reference semantic theme roles; the background script adds only translucent surface/chrome shading and a border-colored hairline. There are no era-specific literal layout colors to drift out of sync. This package does not claim pixel-perfect parity across every Steam screen.
