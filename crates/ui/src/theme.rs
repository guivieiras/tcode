use std::{
    collections::{BTreeMap, HashSet},
    sync::{
        Arc, LazyLock,
        atomic::{AtomicU64, Ordering},
    },
};

use gpui::{App, Global, Hsla, Rgba, SharedString, Window, WindowAppearance, px};
use gpui_base::{
    ColorTokens, PlotTheme, RadiusTokens, ResizableTheme, ScrollbarMode, ScrollbarStyles,
    ScrollbarTheme, SemanticThemeTokens, ThemeAppearance, TypographyTokens,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) mod editor;
mod vscode;

pub use crate::highlight::HighlightTheme;

pub(crate) const TCODE_THEME: &str = include_str!("../../../themes/tcode.json");

/// The Zed theme format carries no geometry or fonts; these stay Tcode's own
/// for every theme, built-in or imported.
const RADIUS: f32 = 10.;
const RADIUS_LG: f32 = 14.;
const FONT_FAMILY: &str = "DM Sans";
const MONO_FONT_FAMILY: &str = if cfg!(any(
    target_os = "ios",
    target_os = "android",
    target_family = "wasm"
)) {
    "Lilex"
} else {
    "SF Mono"
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    Light,
    Dark,
}

impl ThemeMode {
    pub fn is_dark(self) -> bool {
        matches!(self, Self::Dark)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

impl From<WindowAppearance> for ThemeMode {
    fn from(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::Dark,
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::Light,
        }
    }
}

/// The application-owned visual values used directly by tcode render code.
#[derive(Debug, Clone)]
pub struct Theme {
    /// Identifies one resolved theme, for caches of theme-derived output.
    pub(crate) revision: u64,
    pub accent: Hsla,
    pub background: Hsla,
    pub content_surface: Hsla,
    pub editor_foreground: Hsla,
    pub(crate) terminal: crate::terminal_drawer::TerminalPalette,
    pub border: Hsla,
    pub danger: Hsla,
    pub danger_active: Hsla,
    pub danger_foreground: Hsla,
    pub font_family: SharedString,
    pub foreground: Hsla,
    pub highlight_theme: Arc<HighlightTheme>,
    pub info: Hsla,
    pub info_foreground: Hsla,
    pub input: Hsla,
    pub link: Hsla,
    pub list_active: Hsla,
    pub list_hover: Hsla,
    pub mode: ThemeMode,
    pub mono_font_family: SharedString,
    pub muted: Hsla,
    pub muted_foreground: Hsla,
    pub popover: Hsla,
    pub primary: Hsla,
    pub primary_foreground: Hsla,
    pub ring: Hsla,
    pub scrollbar: Hsla,
    pub scrollbar_thumb: Hsla,
    pub scrollbar_thumb_hover: Hsla,
    pub secondary: Hsla,
    pub secondary_active: Hsla,
    pub selection: Hsla,
    pub sidebar: Hsla,
    pub sidebar_accent: Hsla,
    pub sidebar_foreground: Hsla,
    pub success: Hsla,
    pub success_foreground: Hsla,
    pub tab_active: Hsla,
    pub theme_name: SharedString,
    pub tokens: SemanticThemeTokens,
    pub warning: Hsla,
    pub warning_foreground: Hsla,
}

impl Theme {
    /// Connection severity shared by the sidebar and attached machine row.
    pub(crate) fn connection_color(&self, state: &tcode_client::ConnectionState) -> Hsla {
        match state {
            tcode_client::ConnectionState::Connected { .. }
            | tcode_client::ConnectionState::Syncing { .. } => self.success,
            tcode_client::ConnectionState::Reconnecting { .. } => self.warning,
            tcode_client::ConnectionState::Offline { .. } => self.danger,
        }
    }

    /// The composer's fast-mode bolt when fast mode is on. Deliberately not a
    /// theme token: one electric amber that reads on both light and dark
    /// surfaces, so the state is recognisable regardless of the active theme.
    pub(crate) fn fast_mode_accent(&self) -> Hsla {
        gpui::rgb(0xF5A524).into()
    }

    /// The effort slider's fill and label at the highest reasoning level.
    /// Deliberately not a theme token, for the same reason as
    /// [`Self::fast_mode_accent`].
    pub(crate) fn max_effort_accent(&self) -> Hsla {
        gpui::rgb(0x924FF7).into()
    }

    pub fn theme_name(&self) -> &SharedString {
        &self.theme_name
    }
}

impl Global for Theme {}

pub trait ActiveTheme {
    fn theme(&self) -> &Theme;
}

impl ActiveTheme for App {
    #[inline(always)]
    fn theme(&self) -> &Theme {
        self.try_global::<Theme>()
            .unwrap_or_else(|| &embedded_themes()[0])
    }
}

/// Zed's theme-family file format. Keep unconsumed style keys when exporting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ThemeFamily {
    pub name: String,
    pub author: String,
    pub themes: Vec<ThemeConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ThemeConfig {
    pub name: String,
    pub appearance: ThemeMode,
    pub style: BTreeMap<String, Value>,
}

/// What a client persists: the families it imported and the variant chosen
/// for each appearance.
#[derive(Clone, Default, Serialize, Deserialize)]
struct ThemePreferences {
    #[serde(default)]
    families: Vec<ThemeFamily>,
    light: Option<String>,
    dark: Option<String>,
}

struct Themes {
    available: Vec<Theme>,
    preferences: ThemePreferences,
    opaque_canvas: bool,
}

impl Global for Themes {}

/// The bundled family: its light variant, then its dark one.
fn bundled_family() -> &'static ThemeFamily {
    static FAMILY: LazyLock<ThemeFamily> = LazyLock::new(|| {
        let family: ThemeFamily =
            serde_json::from_str(TCODE_THEME).expect("embedded theme should parse");
        assert!(
            matches!(
                family.themes.as_slice(),
                [light, dark] if !light.appearance.is_dark() && dark.appearance.is_dark()
            ),
            "embedded theme must hold a light variant followed by a dark one"
        );
        family
    });
    &FAMILY
}

fn embedded_themes() -> &'static [Theme] {
    static THEMES: LazyLock<Vec<Theme>> = LazyLock::new(|| {
        bundled_family()
            .themes
            .iter()
            .map(Theme::try_from)
            .collect::<Result<_, _>>()
            .expect("embedded theme should resolve")
    });
    &THEMES
}

pub(crate) fn parse_color(value: &str) -> Result<Hsla, String> {
    // Zed accepts CSS shorthand; GPUI parses only the six and eight digit forms.
    let expanded;
    let value = if value.starts_with('#') && matches!(value.len(), 4 | 5) {
        expanded = format!(
            "#{}",
            value[1..]
                .chars()
                .flat_map(|ch| [ch, ch])
                .collect::<String>()
        );
        expanded.as_str()
    } else {
        value
    };
    Rgba::try_from(value)
        .map(Into::into)
        .map_err(|error| format!("invalid color {value:?}: {error}"))
}

/// Black or white, whichever reads better on `background` (WCAG relative
/// luminance). Zed themes name an accent but no color to put on top of it.
fn contrasting_foreground(background: Hsla) -> Hsla {
    let rgb = Rgba::from(background);
    let linear = |channel: f32| {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * linear(rgb.r) + 0.7152 * linear(rgb.g) + 0.0722 * linear(rgb.b);
    if luminance > 0.179 {
        gpui::black()
    } else {
        gpui::white()
    }
}

impl TryFrom<&ThemeConfig> for Theme {
    type Error = String;

    fn try_from(config: &ThemeConfig) -> Result<Self, Self::Error> {
        static NEXT_REVISION: AtomicU64 = AtomicU64::new(0);

        let mode = config.appearance;
        let bundled = &bundled_family().themes[usize::from(mode.is_dark())].style;
        // A missing or null key takes `fallback` when one is given, and the
        // bundled variant of the same appearance otherwise.
        let color = |key: &str, fallback: Option<Hsla>| -> Result<Hsla, String> {
            let value = config.style.get(key).filter(|value| !value.is_null());
            if value.is_none()
                && let Some(fallback) = fallback
            {
                return Ok(fallback);
            }
            let value = value
                .or_else(|| bundled.get(key))
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{key} must be a color in theme {:?}", config.name))?;
            parse_color(value)
        };
        let player = |key: &str| -> Result<Option<Hsla>, String> {
            config
                .style
                .get("players")
                .and_then(Value::as_array)
                .and_then(|players| players.first())
                .and_then(|player| player.get(key))
                .and_then(Value::as_str)
                .map(parse_color)
                .transpose()
        };

        let background = color("background", None)?;
        let foreground = color("text", None)?;
        let content_surface = color("editor.background", Some(background))?;
        let primary = color("text.accent", None)?;
        let primary_foreground = contrasting_foreground(primary);
        let secondary = color("element.background", Some(foreground.opacity(0.06)))?;
        let muted = color("ghost_element.background", Some(secondary))?;
        let muted_foreground = color("text.muted", Some(foreground.opacity(0.65)))?;
        let accent = secondary;
        let border = color("border", Some(foreground.opacity(0.12)))?;
        let input = color("border.variant", Some(border))?;
        let ring = color("border.focused", Some(primary))?;
        // Popovers stay readable over whatever they cover.
        let popover = color("elevated_surface.background", Some(content_surface))?.alpha(1.);
        let danger_foreground = color("error", None)?;
        let info_foreground = color("info", None)?;
        let success_foreground = color("success", None)?;
        let warning_foreground = color("warning", None)?;
        // Zed has one color per status. Tcode's own keys give indicators a
        // stronger shade; a theme without them uses its status color for both.
        let danger = color("tcode.error", Some(danger_foreground))?;
        let info = color("tcode.info", Some(info_foreground))?;
        let success = color("tcode.success", Some(success_foreground))?;
        let warning = color("tcode.warning", Some(warning_foreground))?;
        let selection = player("selection")?.unwrap_or(primary.alpha(0.3));
        let tokens = SemanticThemeTokens {
            colors: ColorTokens {
                selection,
                background,
                foreground,
                surface: popover,
                surface_foreground: foreground,
                primary,
                primary_foreground,
                secondary,
                secondary_foreground: foreground,
                muted,
                muted_foreground,
                accent,
                accent_foreground: foreground,
                destructive: danger,
                destructive_foreground: danger_foreground,
                border,
                input,
                ring,
            },
            // The scale gpui-kit derives from a base and a large radius.
            radius: RadiusTokens {
                sm: px(RADIUS / 2.),
                md: px(RADIUS),
                lg: px(RADIUS_LG),
                xl: px(RADIUS * 2.),
                ..Default::default()
            },
            typography: TypographyTokens {
                sans: FONT_FAMILY.into(),
                mono: MONO_FONT_FAMILY.into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let syntax = if mode.is_dark() {
            HighlightTheme::default_dark()
        } else {
            HighlightTheme::default_light()
        };
        let highlight_theme =
            Arc::new(syntax.with_syntax(config.style.get("syntax").unwrap_or(&Value::Null))?);
        let mut ansi = [foreground; 16];
        for (index, entry) in ansi.iter_mut().enumerate() {
            let bright = if index >= 8 { "bright_" } else { "" };
            let name = [
                "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
            ][index % 8];
            *entry = color(&format!("terminal.ansi.{bright}{name}"), None)?;
        }
        let terminal = crate::terminal_drawer::TerminalPalette {
            background: color("terminal.background", Some(content_surface))?,
            foreground: color("terminal.foreground", Some(foreground))?,
            selection,
            cursor: player("cursor")?.unwrap_or(foreground),
            ansi,
        };

        Ok(Self {
            revision: NEXT_REVISION.fetch_add(1, Ordering::Relaxed),
            accent,
            background,
            content_surface,
            editor_foreground: color("editor.foreground", Some(foreground))?,
            terminal,
            border,
            danger,
            danger_active: danger,
            danger_foreground,
            font_family: FONT_FAMILY.into(),
            foreground,
            highlight_theme,
            info,
            info_foreground,
            input,
            link: primary,
            list_active: color("ghost_element.selected", Some(selection))?,
            list_hover: color("ghost_element.hover", Some(secondary))?,
            mode,
            mono_font_family: MONO_FONT_FAMILY.into(),
            muted,
            muted_foreground,
            popover,
            primary,
            primary_foreground,
            ring,
            scrollbar: color("scrollbar.track.background", Some(background.alpha(0.)))?,
            scrollbar_thumb: color("scrollbar.thumb.background", Some(foreground.opacity(0.2)))?,
            scrollbar_thumb_hover: color(
                "scrollbar.thumb.hover_background",
                Some(foreground.opacity(0.35)),
            )?,
            secondary,
            secondary_active: secondary,
            selection,
            sidebar: color("panel.background", Some(background))?,
            sidebar_accent: secondary,
            sidebar_foreground: color("icon", Some(foreground))?,
            success,
            success_foreground,
            tab_active: background,
            theme_name: config.name.clone().into(),
            tokens,
            warning,
            warning_foreground,
        })
    }
}

impl Themes {
    fn rebuild(&mut self) {
        self.available = embedded_themes().to_vec();
        for config in self.preferences.themes() {
            match Theme::try_from(config) {
                Ok(theme) => self.available.push(theme),
                Err(error) => log::warn!("Ignoring saved theme {:?}: {error}", config.name),
            }
        }
    }

    fn selected(&self, mode: ThemeMode) -> &Theme {
        let name = self.preferences.selection(mode);
        self.available
            .iter()
            .find(|theme| theme.mode == mode && Some(theme.theme_name.as_ref()) == name)
            .unwrap_or(&self.available[usize::from(mode.is_dark())])
    }
}

impl ThemePreferences {
    fn themes(&self) -> impl Iterator<Item = &ThemeConfig> {
        self.families.iter().flat_map(|family| &family.themes)
    }

    fn selection(&self, mode: ThemeMode) -> Option<&str> {
        match mode {
            ThemeMode::Light => self.light.as_deref(),
            ThemeMode::Dark => self.dark.as_deref(),
        }
    }

    fn select(&mut self, mode: ThemeMode, name: Option<String>) {
        match mode {
            ThemeMode::Light => self.light = name,
            ThemeMode::Dark => self.dark = name,
        }
    }

    fn remove(&mut self, name: &str) {
        for family in &mut self.families {
            family.themes.retain(|theme| theme.name != name);
        }
        self.families.retain(|family| !family.themes.is_empty());
    }
}

pub(crate) fn choices(mode: ThemeMode, cx: &App) -> Vec<SharedString> {
    cx.global::<Themes>()
        .available
        .iter()
        .filter(|theme| theme.mode == mode)
        .map(|theme| theme.theme_name.clone())
        .collect()
}

pub(crate) fn selected_name(mode: ThemeMode, cx: &App) -> SharedString {
    cx.global::<Themes>().selected(mode).theme_name.clone()
}

/// Choose the variant an appearance uses. Takes effect at the next
/// [`change_mode`].
pub(crate) fn select(mode: ThemeMode, name: String, cx: &mut App) {
    cx.global_mut::<Themes>()
        .preferences
        .select(mode, Some(name));
}

/// Back to the built-in variants; imported families stay installed.
pub(crate) fn reset_selection(cx: &mut App) {
    let preferences = &mut cx.global_mut::<Themes>().preferences;
    preferences.light = None;
    preferences.dark = None;
}

pub(crate) fn preferences(cx: &App) -> Value {
    serde_json::to_value(&cx.global::<Themes>().preferences)
        .expect("theme preferences serialize to JSON")
}

pub(crate) fn restore_preferences(value: Option<Value>, cx: &mut App) {
    let themes = cx.global_mut::<Themes>();
    themes.preferences = value
        .and_then(|value| {
            serde_json::from_value(value)
                .inspect_err(|error| log::warn!("Ignoring saved themes: {error}"))
                .ok()
        })
        .unwrap_or_default();
    themes.rebuild();
}

/// What [`import`] installed.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Imported {
    /// The source was a VS Code theme, converted approximately.
    pub converted: bool,
    /// The appearance to show so the import is visible: the current one when
    /// the family has a variant for it.
    pub appearance: ThemeMode,
}

/// Install a Zed theme family or a standalone VS Code theme and select its
/// variants. The whole source is validated before the library or the
/// selection changes.
pub(crate) fn import(source: &str, cx: &mut App) -> Result<Imported, String> {
    let value: Value = serde_json_lenient::from_str(source).map_err(|error| error.to_string())?;
    let converted = value.get("themes").is_none();
    let family = if converted {
        vscode::convert(value)?
    } else {
        serde_json::from_value::<ThemeFamily>(value).map_err(|error| error.to_string())?
    };
    validate_family(&family)?;
    let current = cx.theme().mode;
    let appearance = family
        .themes
        .iter()
        .find(|theme| theme.appearance == current)
        .unwrap_or(&family.themes[0])
        .appearance;
    let themes = cx.global_mut::<Themes>();
    for config in &family.themes {
        themes.preferences.remove(&config.name);
        themes
            .preferences
            .select(config.appearance, Some(config.name.clone()));
    }
    themes.preferences.families.push(family);
    themes.rebuild();
    Ok(Imported {
        converted,
        appearance,
    })
}

fn validate_family(family: &ThemeFamily) -> Result<(), String> {
    if family.themes.is_empty() {
        return Err("theme family is empty".into());
    }
    let mut names = HashSet::new();
    for config in &family.themes {
        if config.name.trim().is_empty() {
            return Err("theme name is empty".into());
        }
        if !names.insert(&config.name) {
            return Err(format!("duplicate theme {:?}", config.name));
        }
        if bundled_family()
            .themes
            .iter()
            .any(|bundled| bundled.name == config.name)
        {
            return Err(format!(
                "rename {:?} before importing a customized built-in theme",
                config.name
            ));
        }
        Theme::try_from(config)?;
    }
    Ok(())
}

/// The family holding the active theme, as JSON. A built-in theme exports as
/// a renamed single-variant family, because built-in names are reserved.
pub(crate) fn export_selected(cx: &App) -> String {
    let name = cx.theme().theme_name.as_ref();
    let preferences = &cx.global::<Themes>().preferences;
    let family = if let Some(family) = preferences
        .families
        .iter()
        .find(|family| family.themes.iter().any(|theme| theme.name == name))
    {
        family.clone()
    } else {
        let bundled = bundled_family();
        let mut config = bundled.themes[usize::from(cx.theme().mode.is_dark())].clone();
        config.name = format!("{name} Custom");
        ThemeFamily {
            name: config.name.clone(),
            author: bundled.author.clone(),
            themes: vec![config],
        }
    };
    serde_json::to_string_pretty(&family).expect("theme families serialize to JSON")
}

/// Uninstall the active custom variant; its appearance falls back to the
/// built-in one at the next [`change_mode`].
pub(crate) fn remove_selected(cx: &mut App) {
    let name = cx.theme().theme_name.to_string();
    let themes = cx.global_mut::<Themes>();
    themes.preferences.remove(&name);
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        if themes.preferences.selection(mode) == Some(&name) {
            themes.preferences.select(mode, None);
        }
    }
    themes.rebuild();
}

pub(crate) fn is_custom(cx: &App) -> bool {
    let name = cx.theme().theme_name.as_ref();
    cx.global::<Themes>()
        .preferences
        .themes()
        .any(|theme| theme.name == name)
}

/// Initialize gpui-base behavior and tcode's application-owned theme.
pub fn init(cx: &mut App) {
    init_with_options(false, cx);
}

/// Initialize for a window whose canvas is `opaque_canvas`: with nothing
/// behind it to composite over, a translucent theme canvas is flattened to
/// its own solid color.
pub fn init_with_options(opaque_canvas: bool, cx: &mut App) {
    gpui_base::init(cx);
    crate::overlay::init(cx);
    crate::widgets::menu::init(cx);
    cx.set_global(Themes {
        available: embedded_themes().to_vec(),
        preferences: ThemePreferences::default(),
        opaque_canvas,
    });
    change_mode(ThemeMode::Light, None, cx);
}

pub fn change_mode(mode: ThemeMode, window: Option<&mut Window>, cx: &mut App) {
    let themes = cx.global::<Themes>();
    let mut theme = themes.selected(mode).clone();
    if themes.opaque_canvas {
        theme.background = theme.background.alpha(1.);
        theme.tokens.colors.background = theme.background;
        theme.terminal.background = theme.terminal.background.alpha(1.);
    }
    let radius = theme.tokens.radius;
    let base_theme = gpui_base::Theme {
        appearance: match mode {
            ThemeMode::Light => ThemeAppearance::Light,
            ThemeMode::Dark => ThemeAppearance::Dark,
        },
        tokens: theme.tokens.clone(),
        scrollbar: ScrollbarTheme::new()
            .with_mode(if cx.should_auto_hide_scrollbars() {
                ScrollbarMode::Scrolling
            } else {
                ScrollbarMode::Hover
            })
            .with_styles(
                ScrollbarStyles::default()
                    .track(|style| style.bg(theme.scrollbar))
                    .track_hover(|style| style.bg(theme.scrollbar))
                    .track_active(|style| style.bg(theme.scrollbar).border_color(theme.border))
                    .thumb(|style| style.bg(theme.scrollbar_thumb).radius(radius.full))
                    .thumb_hover(|style| style.bg(theme.scrollbar_thumb_hover).radius(radius.full))
                    .thumb_active(|style| {
                        style.bg(theme.scrollbar_thumb_hover).radius(radius.full)
                    }),
            ),
        resizable: ResizableTheme {
            handle: Some(theme.border),
            active_handle: Some(theme.ring),
        },
        plot: PlotTheme::default(),
    };
    cx.set_global(base_theme);
    cx.set_global(theme);
    #[cfg(target_os = "macos")]
    crate::macos_backdrop::sync_appearance(mode, cx);
    if let Some(window) = window {
        window.refresh();
    }
}

pub fn sync_system_appearance(window: Option<&mut Window>, cx: &mut App) {
    let appearance = window
        .as_ref()
        .map(|window| window.appearance())
        .unwrap_or_else(|| cx.window_appearance());
    change_mode(appearance.into(), window, cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZED: &str = r##"{
        // A single-appearance family with optional fields omitted or null.
        "name": "Test family", "author": "Theme author",
        "themes": [{"name":"Plum", "appearance":"dark", "style": {
            "background":"#201028", "editor.background":"#302038", "text.accent":"#ffff00",
            "text":"#eee", "border":null, "terminal.ansi.red":"#f08",
            "syntax":{"comment":{"color":"#8c9", "font_style":"italic"}},
            "players":[{"cursor":"#fff", "selection":"#4568"}],
            "panel.unused_setting":"preserved",
        }}],
    }"##;

    #[gpui::test]
    fn opaque_windows_and_fullscreen_keep_the_canvas_rgb(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            for opaque in [true, false] {
                init_with_options(opaque, cx);
                for (mode, expected) in
                    [(ThemeMode::Light, "#F2F4F7"), (ThemeMode::Dark, "#15171C")]
                {
                    change_mode(mode, None, cx);
                    let expected = parse_color(expected).unwrap();
                    assert_eq!(crate::material::opaque_canvas(cx), expected);
                    if opaque {
                        assert_eq!(cx.theme().background, expected);
                        assert_eq!(cx.theme().tokens.colors.background, expected);
                        assert_eq!(cx.theme().terminal.background, expected);
                    } else {
                        assert!(cx.theme().background.a < 1.);
                    }
                }
            }
        });
    }

    #[gpui::test]
    fn imported_theme_survives_selection_restore_and_failed_replacement(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            init_with_options(true, cx);
            assert_eq!(
                import(ZED, cx).unwrap(),
                Imported {
                    converted: false,
                    appearance: ThemeMode::Dark
                }
            );
            change_mode(ThemeMode::Dark, None, cx);
            assert_eq!(cx.theme().content_surface, parse_color("#302038").unwrap());
            assert_eq!(cx.theme().primary_foreground, gpui::black());
            assert_eq!(cx.theme().terminal.ansi[1], parse_color("#ff0088").unwrap());
            assert_eq!(cx.theme().selection, parse_color("#44556688").unwrap());
            assert_eq!(cx.theme().danger, cx.theme().danger_foreground);
            let comment = cx.theme().highlight_theme.style("comment").unwrap();
            assert_eq!(comment.color, Some(parse_color("#88cc99").unwrap()));
            assert_eq!(comment.font_style, Some(gpui::FontStyle::Italic));
            let revision = cx.theme().revision;
            let saved = preferences(cx);
            let exported: Value = serde_json::from_str(&export_selected(cx)).unwrap();
            assert_eq!(exported["author"], "Theme author");
            assert_eq!(
                exported["themes"][0]["style"]["panel.unused_setting"],
                "preserved"
            );

            assert!(import(&ZED.replace("#201028", "not a color"), cx).is_err());
            assert_eq!(preferences(cx), saved);
            assert_eq!(cx.theme().theme_name.as_ref(), "Plum");
            select(ThemeMode::Dark, "tcode Dark".into(), cx);
            change_mode(ThemeMode::Dark, None, cx);
            assert_ne!(
                cx.theme().highlight_theme.style("comment").unwrap().color,
                comment.color
            );
            assert_ne!(cx.theme().revision, revision);
            restore_preferences(Some(saved), cx);
            change_mode(ThemeMode::Dark, None, cx);
            assert_eq!(cx.theme().theme_name.as_ref(), "Plum");
            assert_eq!(
                cx.theme().highlight_theme.style("comment").unwrap(),
                comment
            );
            remove_selected(cx);
            change_mode(ThemeMode::Dark, None, cx);
            assert_eq!(cx.theme().theme_name.as_ref(), "tcode Dark");
            assert_ne!(cx.theme().danger, cx.theme().danger_foreground);
        });
    }

    #[gpui::test]
    fn vscode_import_converts_light_palette_scopes_and_styles(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            init(cx);
            let source = r##"{
                "name":"Paper", "type":"light",
                "colors":{"editor.background":"#fafafa", "terminal.ansiBlue":"#234567"},
                "tokenColors":[
                    {"scope":"comment", "settings":{"foreground":"#456789", "fontStyle":"italic bold"}},
                    {"scope":["string.quoted", "constant.character.escape"], "settings":{"foreground":"#987654"}},
                ],
                "semanticTokenColors":{"variable.readonly":"#abcdef"}
            }"##;
            assert_eq!(
                import(source, cx).unwrap(),
                Imported {
                    converted: true,
                    appearance: ThemeMode::Light
                }
            );
            change_mode(ThemeMode::Light, None, cx);
            assert_eq!(cx.theme().background, parse_color("#fafafa").unwrap());
            assert_eq!(cx.theme().terminal.ansi[4], parse_color("#234567").unwrap());
            let comment = cx.theme().highlight_theme.style("comment").unwrap();
            assert_eq!(comment.color, Some(parse_color("#456789").unwrap()));
            assert_eq!(comment.font_weight, Some(gpui::FontWeight::BOLD));
            assert_eq!(comment.font_style, Some(gpui::FontStyle::Italic));
            assert_eq!(
                cx.theme()
                    .highlight_theme
                    .style("string.escape")
                    .unwrap()
                    .color,
                Some(parse_color("#987654").unwrap())
            );
            let saved = preferences(cx);
            for source in [
                r#"{"type":"dark","colors":{},"include":"./base.json"}"#,
                r#"{"type":"dark","colors":{},"tokenColors":"./syntax.tmTheme"}"#,
                r#"{"name":"empty","author":"a","themes":[]}"#,
                r#"{"name":"wrong","author":"a","themes":[{"name":"x","appearance":"dark","style":{"syntax":42}}]}"#,
            ] {
                assert!(import(source, cx).is_err(), "{source}");
                assert_eq!(preferences(cx), saved);
            }
        });
    }
}
