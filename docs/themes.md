# Themes

Tcode uses the [Zed theme-family format](https://zed.dev/schema/themes/v0.2.0.json).
The built-in light and dark variants live in [themes/tcode.json](../themes/tcode.json).
You can create a file with [Zed's Theme Builder](https://zed.dev/theme-builder),
use an existing Zed theme, or import a standalone VS Code color theme.

In **Settings → General → Appearance**:

- **Theme** chooses System default, Light, or Dark.
- **Light theme** and **Dark theme** choose the variant used in each mode.
  System default follows changes to the window's system appearance.
- **Import theme** opens a JSON editor. Paste the file, or use **Open theme file**
  on clients with native file picking, then **Save and apply**. A file containing
  only the other appearance switches to that appearance so the result is visible.
- **Edit current** opens the selected custom theme's family. For a built-in theme,
  it creates a named custom copy. **Copy JSON** exports the editor contents.
- **Remove current** removes the selected custom variant and falls back to the
  built-in variant of that appearance. Other variants remain installed.

Imported definitions and selections are saved on this device, independently of
which machine hosts the workspace. Editing the original external file does not
change the installed copy; import it again to update it. Names identify variants,
so importing an existing name replaces it. Built-in names are reserved.
Import validation completes before any saved theme is replaced. Restore settings
resets the selections and appearance mode but keeps imported themes installed.

## Minimal Zed theme

A family can contain one or more light or dark variants. Missing and null color
values use Tcode's fallbacks. Comments and trailing commas are accepted.

```json
{
  "$schema": "https://zed.dev/schema/themes/v0.2.0.json",
  "name": "Plum",
  "author": "Your name",
  "themes": [
    {
      "name": "Plum Dark",
      "appearance": "dark",
      "style": {
        "background": "#201028",
        "editor.background": "#302038",
        "text": "#EEE8F2",
        "text.accent": "#C090F0",
        "panel.background": "#201028",
        "elevated_surface.background": "#382840",
        "terminal.ansi.red": "#F08090",
        "syntax": {
          "comment": { "color": "#A090A8", "font_style": "italic" },
          "string": { "color": "#A0C090" }
        }
      }
    }
  ]
}
```

Tcode maps the Zed fields to its own controls:

| Zed field | Tcode use |
| --- | --- |
| `background` | Window canvas |
| `editor.background`, `editor.foreground` | Reading area and code text |
| `text`, `text.muted`, `text.accent` | UI text, secondary text, primary actions and links |
| `panel.background`, `icon` | Sidebar background and foreground |
| `elevated_surface.background` | Composer, menus and dialogs |
| `element.background`, `ghost_element.*` | Control backgrounds, hover and selection |
| `border`, `border.variant`, `border.focused` | Borders, inputs and focus ring |
| `error`, `warning`, `info`, `success` | Status colors and diff removal/addition colors |
| `scrollbar.track.background`, `scrollbar.thumb.background`, `scrollbar.thumb.hover_background` | Scrollbars |
| `terminal.background`, `terminal.foreground`, `terminal.ansi.*` | Terminal and inline command output |
| First `players` entry's `selection` and `cursor` | Text selection and terminal cursor |
| `syntax` | Named code styles, including color, background, font style and weight |

The [theme loader](../crates/ui/src/theme.rs) owns the mappings and fallbacks.
The [syntax resolver](../crates/ui/src/highlight.rs) maps language scopes to
named styles. Tcode uses Syntect grammars, so which tokens receive a style can
differ from Zed. Unused style keys are preserved in exported custom families.
Zed's preview reflects Zed's layout, not Tcode's.

Fonts, geometry and native window materials remain Tcode-owned. The Zed
`background.appearance` field is retained but does not change the platform window
material. Opaque windows flatten the selected canvas color, and popovers remain
opaque for readability.

## VS Code import

The importer converts UI colors, terminal ANSI colors and common TextMate token
colors into a Zed family. It uses Tcode's existing syntax categories, without
requiring Zed's importer or its workspace dependencies.

The conversion is approximate. Language-specific scope distinctions, complex
scope selectors and semantic highlighting are not reproduced. Syntax italic and
bold styles are supported; VS Code underline and strikethrough are not converted.
Themes without a `type` need `editor.background` so the importer can infer the
appearance. A missing name becomes **Imported VS Code Theme**; rename it to keep
multiple such imports.

Files using `include` or an external `.tmTheme` reference are rejected with an
explanation. In VS Code, run **Developer: Generate Color Theme from Current
Settings** and import the resulting standalone JSON. Extension packages and
Marketplace installation are outside the importer; use the theme JSON itself.
