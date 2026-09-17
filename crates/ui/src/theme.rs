use std::{collections::BTreeMap, sync::Arc, sync::LazyLock};

use gpui::{App, Global, Hsla, Pixels, Rgba, SharedString, Window, WindowAppearance, px};
use gpui_base::{
    ColorTokens, RadiusTokens, ResizableTheme, ScrollbarMode, ScrollbarStyles, ScrollbarTheme,
    SemanticThemeTokens, ThemeAppearance, TypographyTokens,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) mod editor;
mod vscode;

pub use crate::highlight::HighlightTheme;

pub(crate) const TCODE_THEME: &str = include_str!("../../../themes/tcode.json");

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
    pub radius: Pixels,
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
            tcode_client::ConnectionState::Connected => self.success,
            tcode_client::ConnectionState::Syncing
            | tcode_client::ConnectionState::Reconnecting { .. } => self.warning,
            tcode_client::ConnectionState::Offline { .. } => self.danger,
        }
    }

    /// The composer's fast-mode bolt when fast mode is on. Deliberately not a
    /// theme token: one electric amber that reads on both light and dark
    /// surfaces, so the state is recognisable regardless of the active theme.
    pub(crate) fn fast_mode_accent(&self) -> Hsla {
        gpui::rgb(0xF5A524).into()
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

#[derive(Clone, Default, Serialize, Deserialize)]
struct ThemePreferences {
    #[serde(default)]
    families: Vec<ThemeFamily>,
    light: Option<String>,
    dark: Option<String>,
}

#[derive(Clone)]
struct Themes {
    available: Vec<Theme>,
    preferences: ThemePreferences,
    opaque_canvas: bool,
    revision: u64,
}
impl Global for Themes {}

fn embedded_themes() -> &'static Vec<Theme> {
    static THEMES: LazyLock<Vec<Theme>> = LazyLock::new(|| {
        let family: ThemeFamily = serde_json::from_str(TCODE_THEME).expect("valid embedded theme");
        family
            .themes
            .iter()
            .map(resolve_theme)
            .collect::<Result<_, _>>()
            .expect("valid embedded colors")
    });
    &THEMES
}

pub(crate) fn parse_color(value: &str) -> Result<Hsla, String> {
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
        .map_err(|e| format!("invalid color {value:?}: {e}"))
}

fn resolve_theme(config: &ThemeConfig) -> Result<Theme, String> {
    let dark = config.appearance.is_dark();
    // Defaults come from the same appearance in the bundled Zed theme family.
    let defaults: ThemeFamily = serde_json::from_str(TCODE_THEME).expect("valid embedded theme");
    let default = &defaults.themes[usize::from(dark)].style;
    let color = |key: &str, fallback: Option<Hsla>| -> Result<Hsla, String> {
        let value = config.style.get(key).filter(|v| !v.is_null());
        if value.is_none()
            && let Some(fallback) = fallback
        {
            return Ok(fallback);
        }
        let value = value
            .or_else(|| default.get(key))
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{key} must be a color"))?;
        parse_color(value)
    };
    let background = color("background", None)?;
    let foreground = color("text", None)?;
    let content_surface = color("editor.background", Some(background))?;
    let editor_foreground = color("editor.foreground", Some(foreground))?;
    let primary = color("text.accent", None)?;
    let rgb = Rgba::from(primary);
    let linear = |channel: f32| {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * linear(rgb.r) + 0.7152 * linear(rgb.g) + 0.0722 * linear(rgb.b);
    let primary_foreground = if luminance > 0.179 {
        gpui::black()
    } else {
        gpui::white()
    };
    let secondary = color("element.background", Some(foreground.opacity(0.06)))?;
    let muted = color("ghost_element.background", Some(secondary))?;
    let muted_foreground = color("text.muted", Some(foreground.opacity(0.65)))?;
    let accent = secondary;
    let border = color("border", Some(foreground.opacity(0.12)))?;
    let input = color("border.variant", Some(border))?;
    let ring = color("border.focused", Some(primary))?;
    let popover = color("elevated_surface.background", Some(content_surface))?.opacity(1.);
    let danger_foreground = color("error", None)?;
    let danger = danger_foreground;
    let selection = config
        .style
        .get("players")
        .and_then(Value::as_array)
        .and_then(|players| players.first())
        .and_then(|p| p.get("selection"))
        .and_then(Value::as_str)
        .map(parse_color)
        .transpose()?
        .unwrap_or(primary.opacity(0.3));
    let radius = px(10.);
    let font_family: SharedString = "DM Sans".into();
    let mono_font_family: SharedString = if cfg!(any(
        target_os = "ios",
        target_os = "android",
        target_family = "wasm"
    )) {
        "Lilex".into()
    } else {
        "SF Mono".into()
    };
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
        radius: RadiusTokens {
            sm: px(5.),
            md: radius,
            lg: px(14.),
            xl: px(18.),
            ..Default::default()
        },
        typography: TypographyTokens {
            sans: font_family.clone(),
            mono: mono_font_family.clone(),
            ..Default::default()
        },
        ..Default::default()
    };
    let syntax = if dark {
        HighlightTheme::default_dark()
    } else {
        HighlightTheme::default_light()
    };
    let highlight_theme =
        Arc::new(syntax.with_syntax(config.style.get("syntax").unwrap_or(&Value::Null))?);
    let names = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ];
    let mut ansi = [foreground; 16];
    for (i, entry) in ansi.iter_mut().enumerate() {
        let bright = if i >= 8 { "bright_" } else { "" };
        *entry = color(&format!("terminal.ansi.{bright}{}", names[i % 8]), None)?;
    }
    let terminal = crate::terminal_drawer::TerminalPalette {
        background: color("terminal.background", Some(content_surface))?,
        foreground: color("terminal.foreground", Some(foreground))?,
        selection,
        cursor: config
            .style
            .get("players")
            .and_then(Value::as_array)
            .and_then(|players| players.first())
            .and_then(|p| p.get("cursor"))
            .and_then(Value::as_str)
            .map(parse_color)
            .transpose()?
            .unwrap_or(foreground),
        ansi,
    };
    Ok(Theme {
        revision: 0,
        accent,
        background,
        content_surface,
        editor_foreground,
        terminal,
        border,
        danger,
        danger_active: danger,
        danger_foreground,
        font_family,
        foreground,
        highlight_theme,
        info: color("info", None)?,
        info_foreground: color("info", None)?,
        input,
        link: primary,
        list_active: color("ghost_element.selected", Some(selection))?,
        list_hover: color("ghost_element.hover", Some(secondary))?,
        mode: config.appearance,
        mono_font_family,
        muted,
        muted_foreground,
        popover,
        primary,
        primary_foreground,
        radius,
        ring,
        scrollbar: color("scrollbar.track.background", Some(background.opacity(0.)))?,
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
        success: color("success", None)?,
        success_foreground: color("success", None)?,
        tab_active: content_surface,
        theme_name: config.name.clone().into(),
        tokens,
        warning: color("warning", None)?,
        warning_foreground: color("warning", None)?,
    })
}

impl Themes {
    fn rebuild(&mut self) {
        self.available = embedded_themes().clone();
        for config in self
            .preferences
            .families
            .iter()
            .flat_map(|family| &family.themes)
        {
            match resolve_theme(config) {
                Ok(theme) => self.available.push(theme),
                Err(error) => log::warn!("Ignoring saved theme {:?}: {error}", config.name),
            }
        }
    }

    fn selected(&self, mode: ThemeMode) -> &Theme {
        let name = if mode.is_dark() {
            &self.preferences.dark
        } else {
            &self.preferences.light
        };
        self.available
            .iter()
            .find(|t| t.mode == mode && Some(t.theme_name.as_ref()) == name.as_deref())
            .unwrap_or(&self.available[usize::from(mode.is_dark())])
    }
}

pub(crate) fn choices(mode: ThemeMode, cx: &App) -> Vec<SharedString> {
    cx.global::<Themes>()
        .available
        .iter()
        .filter(|t| t.mode == mode)
        .map(|t| t.theme_name.clone())
        .collect()
}

pub(crate) fn selected_name(mode: ThemeMode, cx: &App) -> SharedString {
    cx.global::<Themes>().selected(mode).theme_name.clone()
}

pub(crate) fn select(mode: ThemeMode, name: String, cx: &mut App) {
    let themes = cx.global_mut::<Themes>();
    if mode.is_dark() {
        themes.preferences.dark = Some(name);
    } else {
        themes.preferences.light = Some(name);
    }
}

pub(crate) fn reset_selection(cx: &mut App) {
    let registry = cx.global_mut::<Themes>();
    registry.preferences.light = None;
    registry.preferences.dark = None;
}

pub(crate) fn preferences(cx: &App) -> Value {
    serde_json::to_value(&cx.global::<Themes>().preferences)
        .expect("serializable theme preferences")
}

pub(crate) fn restore_preferences(value: Option<Value>, cx: &mut App) {
    let themes = cx.global_mut::<Themes>();
    themes.preferences = value
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    themes.rebuild();
}

/// Validate the whole import before changing the library or current selection.
/// VS Code conversion is approximate; Zed files use this same resolution path.
pub(crate) fn import(source: &str, cx: &mut App) -> Result<(bool, ThemeMode), String> {
    let value: Value = serde_json_lenient::from_str(source).map_err(|e| e.to_string())?;
    let converted = value.get("themes").is_none();
    let family = if converted {
        vscode::convert(value)?
    } else {
        serde_json::from_value::<ThemeFamily>(value).map_err(|e| e.to_string())?
    };
    validate_family(&family)?;
    let mode = family
        .themes
        .iter()
        .find(|t| t.appearance == cx.theme().mode)
        .unwrap_or(&family.themes[0])
        .appearance;
    let registry = cx.global_mut::<Themes>();
    for config in &family.themes {
        for saved in &mut registry.preferences.families {
            saved.themes.retain(|t| t.name != config.name);
        }
        if config.appearance.is_dark() {
            registry.preferences.dark = Some(config.name.clone());
        } else {
            registry.preferences.light = Some(config.name.clone());
        }
    }
    registry
        .preferences
        .families
        .retain(|family| !family.themes.is_empty());
    registry.preferences.families.push(family);
    registry.rebuild();
    Ok((converted, mode))
}

fn validate_family(family: &ThemeFamily) -> Result<(), String> {
    if family.themes.is_empty() {
        return Err("theme family is empty".into());
    }
    let mut names = std::collections::HashSet::new();
    for config in &family.themes {
        if config.name.trim().is_empty() {
            return Err("theme name is empty".into());
        }
        if !names.insert(&config.name) {
            return Err(format!("duplicate theme {:?}", config.name));
        }
        if embedded_themes()
            .iter()
            .any(|t| t.theme_name.as_ref() == config.name)
        {
            return Err(format!(
                "rename {:?} before importing a customized built-in theme",
                config.name
            ));
        }
        resolve_theme(config)?;
    }
    Ok(())
}

pub(crate) fn export_selected(cx: &App) -> String {
    let registry = cx.global::<Themes>();
    let name = cx.theme().theme_name.as_ref();
    let family = if let Some(family) = registry
        .preferences
        .families
        .iter()
        .find(|family| family.themes.iter().any(|t| t.name == name))
    {
        family.clone()
    } else {
        let bundled: ThemeFamily = serde_json::from_str(TCODE_THEME).expect("valid embedded theme");
        let mut config = bundled
            .themes
            .into_iter()
            .find(|t| t.name == name)
            .expect("active theme has a definition");
        config.name = format!("{name} Custom");
        ThemeFamily {
            name: config.name.clone(),
            author: bundled.author,
            themes: vec![config],
        }
    };
    serde_json::to_string_pretty(&family).expect("serializable theme")
}

pub(crate) fn remove_selected(cx: &mut App) {
    let name = cx.theme().theme_name.to_string();
    let registry = cx.global_mut::<Themes>();
    for family in &mut registry.preferences.families {
        family.themes.retain(|t| t.name != name);
    }
    registry
        .preferences
        .families
        .retain(|family| !family.themes.is_empty());
    if registry.preferences.light.as_deref() == Some(&name) {
        registry.preferences.light = None;
    }
    if registry.preferences.dark.as_deref() == Some(&name) {
        registry.preferences.dark = None;
    }
    registry.rebuild();
}

pub(crate) fn is_custom(cx: &App) -> bool {
    cx.global::<Themes>()
        .preferences
        .families
        .iter()
        .flat_map(|family| &family.themes)
        .any(|t| t.name == cx.theme().theme_name.as_ref())
}

/// Initialize gpui-base behavior and tcode's application-owned theme.
pub fn init(cx: &mut App) {
    init_with_options(false, cx);
}

pub fn init_with_options(opaque_canvas: bool, cx: &mut App) {
    gpui_base::init(cx);
    crate::widgets::menu::init(cx);
    cx.set_global(Themes {
        available: embedded_themes().clone(),
        preferences: ThemePreferences::default(),
        opaque_canvas,
        revision: 0,
    });
    change_mode(ThemeMode::Light, None, cx);
}

pub fn change_mode(mode: ThemeMode, window: Option<&mut Window>, cx: &mut App) {
    let registry = cx.global_mut::<Themes>();
    registry.revision += 1;
    let mut theme = registry.selected(mode).clone();
    theme.revision = registry.revision;
    if registry.opaque_canvas {
        theme.background = theme.background.opacity(1.);
        theme.tokens.colors.background = theme.background;
    }
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
                    .thumb(|style| style.bg(theme.scrollbar_thumb).radius(theme.radius))
                    .thumb_hover(|style| style.bg(theme.scrollbar_thumb_hover).radius(theme.radius))
                    .thumb_active(|style| {
                        style.bg(theme.scrollbar_thumb_hover).radius(theme.radius)
                    }),
            ),
        resizable: ResizableTheme {
            handle: Some(theme.border),
            active_handle: Some(theme.ring),
        },
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

    #[test]
    fn machine_connection_dots_preserve_severity() {
        use tcode_client::{ConnectionFailure, ConnectionState};

        for theme in embedded_themes() {
            for (state, expected) in [
                (ConnectionState::Connected, theme.success),
                (ConnectionState::Syncing, theme.warning),
                (
                    ConnectionState::Reconnecting {
                        attempt: 1,
                        reason: Some(ConnectionFailure::Unreachable),
                    },
                    theme.warning,
                ),
                (
                    ConnectionState::Offline {
                        reason: ConnectionFailure::AuthenticationRejected,
                    },
                    theme.danger,
                ),
            ] {
                assert_eq!(theme.connection_color(&state), expected, "{state:?}");
            }
            assert_ne!(theme.success, theme.warning);
            assert_ne!(theme.warning, theme.danger);
            assert_ne!(theme.success, theme.danger);
        }
    }

    #[test]
    fn parses_embedded_tcode_themes() {
        let themes = embedded_themes();

        assert_eq!(themes[0].theme_name.as_ref(), "tcode Light");
        assert_eq!(themes[1].theme_name.as_ref(), "tcode Dark");
        assert_eq!(themes[0].primary, Rgba::try_from("#1447E6").unwrap().into());
        assert_eq!(
            themes[1].background,
            Rgba::try_from("#15171CC7").unwrap().into()
        );
        assert_eq!(themes[0].radius, px(10.));
        assert_eq!(themes[1].radius, px(10.));
    }
}

#[cfg(test)]
pub(crate) fn test_terminal_palette() -> crate::terminal_drawer::TerminalPalette {
    embedded_themes()[0].terminal
}

#[cfg(test)]
mod import_tests {
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
    fn imported_theme_survives_selection_restore_and_failed_replacement(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            init_with_options(true, cx);
            assert_eq!(import(ZED, cx).unwrap(), (false, ThemeMode::Dark));
            change_mode(ThemeMode::Dark, None, cx);
            assert_eq!(cx.theme().content_surface, parse_color("#302038").unwrap());
            assert_eq!(cx.theme().primary_foreground, gpui::black());
            assert_eq!(cx.theme().terminal.ansi[1], parse_color("#ff0088").unwrap());
            assert_eq!(cx.theme().selection, parse_color("#44556688").unwrap());
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
            assert!(cx.theme().revision > revision);
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
            assert_eq!(import(source, cx).unwrap(), (true, ThemeMode::Light));
            change_mode(ThemeMode::Light, None, cx);
            assert_eq!(cx.theme().background, parse_color("#fafafa").unwrap());
            assert_eq!(cx.theme().terminal.ansi[4], parse_color("#234567").unwrap());
            let comment = cx.theme().highlight_theme.style("comment").unwrap();
            assert_eq!(comment.color, Some(parse_color("#456789").unwrap()));
            assert_eq!(comment.font_weight, Some(gpui::FontWeight::BOLD));
            assert_eq!(comment.font_style, Some(gpui::FontStyle::Italic));
            assert_eq!(cx.theme().highlight_theme.style("string.escape").unwrap().color,
                Some(parse_color("#987654").unwrap()));
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
