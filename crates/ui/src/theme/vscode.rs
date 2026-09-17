//! Standalone VS Code JSON/JSONC to Zed theme-family conversion.
//! Uses Tcode's existing scope categories; language-specific and semantic rules
//! cannot retain their full meaning in a category-based syntax palette.

use super::{ThemeConfig, ThemeFamily, ThemeMode, parse_color};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VsCodeTheme {
    name: Option<String>,
    #[serde(rename = "type")]
    appearance: Option<String>,
    colors: BTreeMap<String, Option<String>>,
    #[serde(default)]
    token_colors: Value,
    include: Option<String>,
}

#[derive(Deserialize)]
struct TokenRule {
    scope: Option<Value>,
    settings: TokenSettings,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenSettings {
    foreground: Option<String>,
    font_style: Option<String>,
}

pub(super) fn convert(value: Value) -> Result<ThemeFamily, String> {
    let source: VsCodeTheme = serde_json::from_value(value).map_err(|e| e.to_string())?;
    if source.include.is_some() || source.token_colors.is_string() {
        return Err(crate::tr!("settings.theme.external_reference").into_owned());
    }
    let appearance = match source.appearance.as_deref() {
        Some("light" | "hcLight") => ThemeMode::Light,
        Some("dark" | "hcDark") => ThemeMode::Dark,
        Some(other) => return Err(format!("unsupported theme type {other:?}")),
        None => {
            let background = source.colors.get("editor.background").and_then(Option::as_deref)
                .ok_or("VS Code theme needs type or editor.background to determine light/dark appearance")?;
            if parse_color(background)?.l > 0.5 {
                ThemeMode::Light
            } else {
                ThemeMode::Dark
            }
        }
    };
    let mut style = BTreeMap::new();
    for &(vscode, zed) in UI_COLORS {
        if let Some(Some(color)) = source.colors.get(vscode) {
            parse_color(color)?;
            style.insert(zed.to_owned(), json!(color));
        }
    }
    for name in [
        "Black", "Red", "Green", "Yellow", "Blue", "Magenta", "Cyan", "White",
    ] {
        for bright in [false, true] {
            let prefix = if bright { "Bright" } else { "" };
            if let Some(Some(color)) = source.colors.get(&format!("terminal.ansi{prefix}{name}")) {
                parse_color(color)?;
                let prefix = if bright { "bright_" } else { "" };
                style.insert(
                    format!("terminal.ansi.{prefix}{}", name.to_lowercase()),
                    json!(color),
                );
            }
        }
    }
    let mut player = serde_json::Map::new();
    for (src, dst) in [
        ("editor.selectionBackground", "selection"),
        ("editorCursor.foreground", "cursor"),
    ] {
        if let Some(Some(color)) = source.colors.get(src) {
            parse_color(color)?;
            player.insert(dst.into(), json!(color));
        }
    }
    if !player.is_empty() {
        style.insert("players".into(), json!([player]));
    }
    let rules: Vec<TokenRule> = if source.token_colors.is_null() {
        Vec::new()
    } else {
        serde_json::from_value(source.token_colors).map_err(|e| e.to_string())?
    };
    let mut syntax = BTreeMap::<String, Value>::new();
    let mut ranks = BTreeMap::new();
    for (index, rule) in rules.into_iter().enumerate() {
        if let Some(color) = &rule.settings.foreground {
            parse_color(color)?;
        }
        let scopes = match rule.scope {
            None => {
                if let Some(color) = rule.settings.foreground {
                    style
                        .entry("editor.foreground".into())
                        .or_insert(json!(color));
                }
                continue;
            }
            Some(Value::String(scope)) => vec![scope],
            Some(Value::Array(scopes)) => scopes
                .into_iter()
                .map(|s| {
                    s.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| "token scope must be a string".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?,
            _ => return Err("token scope must be a string or array".into()),
        };
        for scope in scopes.iter().flat_map(|s| s.split(',')).map(str::trim) {
            if scope.chars().any(char::is_whitespace) {
                continue;
            }
            let Some((_, category)) = crate::highlight::SCOPE_TO_THEME_KEY
                .iter()
                .find(|(selector, _)| crate::highlight::scope_matches(scope, selector))
            else {
                continue;
            };
            let rank = (scope.len(), index);
            if ranks
                .get(*category)
                .is_some_and(|previous| *previous > rank)
            {
                continue;
            }
            ranks.insert(category.to_string(), rank);
            let entry = syntax.entry(category.to_string()).or_insert(json!({}));
            if let Some(color) = &rule.settings.foreground {
                entry["color"] = json!(color);
            }
            if let Some(font) = &rule.settings.font_style {
                entry["font_style"] = json!(if font.split_whitespace().any(|s| s == "italic") {
                    "italic"
                } else {
                    "normal"
                });
                entry["font_weight"] = json!(if font.split_whitespace().any(|s| s == "bold") {
                    700
                } else {
                    400
                });
            }
        }
    }
    style.insert("syntax".into(), json!(syntax));
    let name = source
        .name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "Imported VS Code Theme".into());
    Ok(ThemeFamily {
        name: name.clone(),
        author: String::new(),
        themes: vec![ThemeConfig {
            name,
            appearance,
            style,
        }],
    })
}

// Map the controls Tcode renders, leaving unrelated VS Code editor chrome out.
const UI_COLORS: &[(&str, &str)] = &[
    ("editor.background", "background"),
    ("editor.background", "editor.background"),
    ("editor.foreground", "text"),
    ("foreground", "text"),
    ("editor.foreground", "editor.foreground"),
    ("descriptionForeground", "text.muted"),
    ("focusBorder", "border.focused"),
    ("button.background", "text.accent"),
    ("textLink.foreground", "text.accent"),
    ("panel.border", "border"),
    ("input.border", "border.variant"),
    ("input.background", "element.background"),
    ("dropdown.background", "elevated_surface.background"),
    ("sideBar.background", "panel.background"),
    ("sideBar.foreground", "icon"),
    ("list.hoverBackground", "ghost_element.hover"),
    ("list.activeSelectionBackground", "ghost_element.selected"),
    ("scrollbarSlider.background", "scrollbar.thumb.background"),
    (
        "scrollbarSlider.hoverBackground",
        "scrollbar.thumb.hover_background",
    ),
    ("terminal.background", "terminal.background"),
    ("terminal.foreground", "terminal.foreground"),
    ("editorError.foreground", "error"),
    ("editorWarning.foreground", "warning"),
    ("editorInfo.foreground", "info"),
    ("gitDecoration.addedResourceForeground", "success"),
];
