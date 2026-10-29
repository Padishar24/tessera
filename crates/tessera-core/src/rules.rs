//! Window rules: which windows are ignored, float, or end up in
//! a specific zone?

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

/// Rule as written in `config.toml` under `[[rules]]`.
///
/// `exe` and `class` must match completely, `title` only needs to be
/// contained. All patterns are case-insensitive regular
/// expressions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleConfig {
    pub exe: Option<String>,
    pub class: Option<String>,
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ignore: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub float: bool,
    /// Zone, 1-based.
    pub zone: Option<u8>,
}

/// What a matching rule means for a window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RuleEffect {
    pub ignore: bool,
    pub float: bool,
    pub zone: Option<u8>,
}

/// Properties of a window that rules are checked against.
#[derive(Debug, Clone, Copy)]
pub struct WindowProps<'a> {
    /// File name of the executable, e.g. `notepad.exe`.
    pub exe: &'a str,
    pub class: &'a str,
    pub title: &'a str,
}

/// Compiled window patterns, shared by rules and scratchpads: `exe` and
/// `class` must match completely, `title` only needs to be contained.
#[derive(Debug, Clone)]
pub(crate) struct Matcher {
    exe: Option<Regex>,
    class: Option<Regex>,
    title: Option<Regex>,
}

impl Matcher {
    /// Compiles the given patterns; errors start with `context`, e.g.
    /// `rule 3`.
    pub(crate) fn compile(
        context: &str,
        exe: Option<&str>,
        class: Option<&str>,
        title: Option<&str>,
        errors: &mut Vec<String>,
    ) -> Self {
        let mut compile = |field: &str, pattern: Option<&str>, anchored: bool| {
            let pattern = pattern?;
            let source = if anchored {
                format!("^(?:{pattern})$")
            } else {
                pattern.to_owned()
            };
            match RegexBuilder::new(&source).case_insensitive(true).build() {
                Ok(re) => Some(re),
                Err(e) => {
                    errors.push(format!("{context}: invalid pattern for {field}: {e}"));
                    None
                }
            }
        };
        Self {
            exe: compile("exe", exe, true),
            class: compile("class", class, true),
            title: compile("title", title, false),
        }
    }

    pub(crate) fn matches(&self, props: WindowProps<'_>) -> bool {
        let matches =
            |re: &Option<Regex>, text: &str| re.as_ref().is_none_or(|re| re.is_match(text));
        matches(&self.exe, props.exe)
            && matches(&self.class, props.class)
            && matches(&self.title, props.title)
    }
}

#[derive(Debug, Clone)]
struct Rule {
    matcher: Matcher,
    effect: RuleEffect,
}

#[derive(Debug, Clone, Default)]
pub struct Rules(Vec<Rule>);

impl Rules {
    /// Compiles the rules; errors contain the rule number (1-based).
    pub fn compile(configs: &[RuleConfig]) -> Result<Self, Vec<String>> {
        let mut rules = Vec::with_capacity(configs.len());
        let mut errors = Vec::new();

        for (i, cfg) in configs.iter().enumerate() {
            let n = i + 1;
            if cfg.exe.is_none() && cfg.class.is_none() && cfg.title.is_none() {
                errors.push(format!("rule {n}: specify at least exe, class or title"));
                continue;
            }
            if !cfg.ignore && !cfg.float && cfg.zone.is_none() {
                errors.push(format!("rule {n}: specify ignore, float or zone"));
                continue;
            }
            if cfg.zone == Some(0) {
                errors.push(format!("rule {n}: zones start at 1"));
                continue;
            }

            let matcher = Matcher::compile(
                &format!("rule {n}"),
                cfg.exe.as_deref(),
                cfg.class.as_deref(),
                cfg.title.as_deref(),
                &mut errors,
            );
            rules.push(Rule {
                matcher,
                effect: RuleEffect {
                    ignore: cfg.ignore,
                    float: cfg.float,
                    zone: cfg.zone,
                },
            });
        }

        if errors.is_empty() {
            Ok(Rules(rules))
        } else {
            Err(errors)
        }
    }

    /// Effect of the first matching rule, otherwise `None`.
    pub fn find(&self, props: WindowProps<'_>) -> Option<RuleEffect> {
        self.0
            .iter()
            .find(|r| r.matcher.matches(props))
            .map(|r| r.effect)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(exe: Option<&str>, class: Option<&str>, title: Option<&str>) -> RuleConfig {
        RuleConfig {
            exe: exe.map(Into::into),
            class: class.map(Into::into),
            title: title.map(Into::into),
            float: true,
            ..Default::default()
        }
    }

    fn props<'a>(exe: &'a str, class: &'a str, title: &'a str) -> WindowProps<'a> {
        WindowProps { exe, class, title }
    }

    #[test]
    fn exe_and_class_match_fully_title_partially() {
        let rules = Rules::compile(&[
            rule(Some("code.exe"), None, None),
            rule(None, Some("#32770"), None),
            rule(None, None, Some("Picture-in-Picture")),
        ])
        .unwrap();

        assert!(
            rules
                .find(props("Code.exe", "Chrome_WidgetWin_1", "x"))
                .is_some()
        );
        assert!(
            rules
                .find(props("vscode.exe", "Chrome_WidgetWin_1", "x"))
                .is_none()
        );
        assert!(rules.find(props("a.exe", "#32770", "Save As")).is_some());
        assert!(rules.find(props("a.exe", "#327701", "")).is_none());
        assert!(
            rules
                .find(props(
                    "firefox.exe",
                    "MozillaDialogClass",
                    "Picture-in-Picture – Video"
                ))
                .is_some()
        );
    }

    #[test]
    fn all_given_fields_must_match_and_first_rule_wins() {
        let mut ignore = rule(Some("explorer.exe"), Some("OperationStatusWindow"), None);
        ignore.float = false;
        ignore.ignore = true;
        let rules = Rules::compile(&[ignore, rule(Some("explorer.exe"), None, None)]).unwrap();

        let effect = rules
            .find(props("explorer.exe", "OperationStatusWindow", ""))
            .unwrap();
        assert!(effect.ignore && !effect.float);
        let effect = rules
            .find(props("explorer.exe", "CabinetWClass", ""))
            .unwrap();
        assert!(effect.float && !effect.ignore);
    }

    #[test]
    fn reports_invalid_rules() {
        let mut no_effect = rule(Some("a.exe"), None, None);
        no_effect.float = false;
        let errors = Rules::compile(&[
            rule(None, None, None),
            no_effect,
            rule(Some("("), None, None),
            RuleConfig {
                exe: Some("a.exe".into()),
                zone: Some(0),
                ..Default::default()
            },
        ])
        .unwrap_err();
        assert_eq!(errors.len(), 4);
        assert!(errors[2].starts_with("rule 3"));
    }
}
