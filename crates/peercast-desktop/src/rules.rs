use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use unicode_normalization::UnicodeNormalization;

pub const RULE_FIELDS: &[&str] = &[
    "name",
    "details",
    "comment",
    "contactUrl",
    "yellowPage",
    "streamUrl",
    "contentType",
    "listeners",
    "bitrate",
];
pub const MAX_RULES: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelRule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub pattern: String,
    pub fields: Vec<String>,
    pub use_regex: bool,
    pub normalize_width: bool,
    pub match_case: bool,
    pub favorite: bool,
    pub hidden: bool,
    pub mute: bool,
    pub text_color: Option<String>,
    pub background_color: Option<String>,
    pub sound_path: Option<String>,
}

impl Default for ChannelRule {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: "新しいルール".into(),
            enabled: true,
            pattern: String::new(),
            fields: vec!["name".into()],
            use_regex: false,
            normalize_width: true,
            match_case: false,
            favorite: true,
            hidden: false,
            mute: false,
            text_color: None,
            background_color: None,
            sound_path: None,
        }
    }
}

pub fn normalize_rules(mut rules: Vec<ChannelRule>) -> Result<Vec<ChannelRule>, String> {
    if rules.len() > MAX_RULES {
        return Err("ルールは128件まで登録できます".into());
    }
    let mut ids = HashSet::new();
    for rule in &mut rules {
        rule.name = rule.name.trim().to_owned();
        if rule.id.is_empty()
            || rule.id.len() > 80
            || !rule
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            || !ids.insert(rule.id.clone())
        {
            return Err("ルールの識別子が無効または重複しています".into());
        }
        if rule.name.is_empty()
            || rule.name.chars().count() > 80
            || rule.name.chars().any(char::is_control)
        {
            return Err("ルール名は1〜80文字で入力してください".into());
        }
        if rule.pattern.chars().count() > 2048 || rule.pattern.contains('\0') {
            return Err("検索文字は2048文字以内で入力してください".into());
        }
        let mut fields = HashSet::new();
        if rule
            .fields
            .iter()
            .any(|field| !RULE_FIELDS.contains(&field.as_str()) || !fields.insert(field.clone()))
        {
            return Err("検索対象が無効または重複しています".into());
        }
        if rule.enabled && (rule.pattern.is_empty() || rule.fields.is_empty()) {
            return Err(format!(
                "「{}」の検索文字と検索対象を指定してください",
                rule.name
            ));
        }
        for value in [&mut rule.text_color, &mut rule.background_color]
            .into_iter()
            .flatten()
        {
            if value.len() != 7
                || !value.starts_with('#')
                || !value.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
            {
                return Err("色は#RRGGBB形式で指定してください".into());
            }
            *value = value.to_ascii_uppercase();
        }
        if let Some(path) = rule.sound_path.take() {
            let path = path.trim().to_owned();
            if !path.is_empty() {
                validate_sound_path(&path)?;
                rule.sound_path = Some(path);
            }
        }
        if rule.enabled {
            compile(rule)?;
        }
    }
    Ok(rules)
}

pub fn upsert_quick_channel_rule(
    rules: &mut Vec<ChannelRule>,
    channel_name: &str,
    kind: &str,
) -> Result<(), String> {
    let normalized_name = channel_name.trim().nfkc().collect::<String>();
    if normalized_name.is_empty() || normalized_name.chars().any(char::is_control) {
        return Err("チャンネル名が正しくありません".into());
    }
    let pattern = format!("^{}$", regex::escape(&normalized_name));
    if pattern.chars().count() > 2048 {
        return Err("チャンネル名が長すぎます".into());
    }
    let (favorite, hidden) = match kind {
        "favorite" => (true, false),
        "ng" => (false, true),
        _ => return Err("チャンネルの登録方法が正しくありません".into()),
    };
    let existing = rules.iter_mut().find(|rule| {
        rule.id.starts_with("quick-")
            && rule.use_regex
            && rule.normalize_width
            && rule.fields == ["name"]
            && rule.pattern == pattern
    });
    let rule = match existing {
        Some(rule) => rule,
        None => {
            if rules.len() >= MAX_RULES {
                return Err("ルールは128件まで登録できます".into());
            }
            rules.push(ChannelRule {
                id: format!("quick-{}", uuid::Uuid::new_v4().simple()),
                ..ChannelRule::default()
            });
            rules.last_mut().expect("a quick rule was just added")
        }
    };
    rule.name = normalized_name.chars().take(80).collect();
    rule.enabled = true;
    rule.pattern = pattern;
    rule.fields = vec!["name".into()];
    rule.use_regex = true;
    rule.normalize_width = true;
    rule.match_case = false;
    rule.favorite = favorite;
    rule.hidden = hidden;
    rule.mute = false;
    rule.text_color = None;
    rule.background_color = None;
    rule.sound_path = None;
    normalize_rules(rules.clone()).map(|normalized| *rules = normalized)
}

pub fn validate_sound_path(path: &str) -> Result<(), String> {
    if path.len() > 32767
        || path.chars().any(char::is_control)
        || !std::path::Path::new(path)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
    {
        return Err("通知音にはWAVファイルを指定してください".into());
    }
    Ok(())
}

fn compile(rule: &ChannelRule) -> Result<Regex, String> {
    let pattern = if rule.normalize_width {
        rule.pattern.nfkc().collect::<String>()
    } else {
        rule.pattern.clone()
    };
    let pattern = if rule.use_regex {
        pattern
    } else {
        regex::escape(&pattern)
    };
    RegexBuilder::new(&pattern)
        .case_insensitive(!rule.match_case)
        .size_limit(512 * 1024)
        .dfa_size_limit(512 * 1024)
        .nest_limit(64)
        .build()
        .map_err(|error| format!("「{}」の検索式を確認してください: {error}", rule.name))
}

pub fn rule_matches(rule: &ChannelRule, row: &Value, endpoint: &str) -> Result<bool, String> {
    if !rule.enabled || rule.pattern.is_empty() || rule.fields.is_empty() {
        return Ok(false);
    }
    let expression = compile(rule)?;
    Ok(matches_compiled(rule, &expression, row, endpoint))
}

fn matches_compiled(rule: &ChannelRule, expression: &Regex, row: &Value, endpoint: &str) -> bool {
    rule.fields.iter().any(|field| {
        let value = match field.as_str() {
            "details" => format!(
                "{} {} {}",
                row["genre"].as_str().unwrap_or_default(),
                row["description"].as_str().unwrap_or_default(),
                row["trackTitle"].as_str().unwrap_or_default()
            ),
            "streamUrl" => format!(
                "{}/stream/{}",
                endpoint.trim_end_matches('/'),
                row["channelId"].as_str().unwrap_or_default()
            ),
            field => match row.get(field) {
                Some(Value::String(value)) => value.clone(),
                Some(Value::Number(value)) => value.to_string(),
                _ => String::new(),
            },
        };
        if rule.normalize_width {
            expression.is_match(&value.nfkc().collect::<String>())
        } else {
            expression.is_match(&value)
        }
    })
}

pub fn decorate_channels(
    mut rows: Vec<Value>,
    rules: &[ChannelRule],
    endpoint: &str,
) -> Result<Vec<Value>, String> {
    let normalized = normalize_rules(rules.to_vec())?;
    let compiled = normalized
        .iter()
        .filter(|rule| rule.enabled)
        .map(|rule| compile(rule).map(|expression| (rule, expression)))
        .collect::<Result<Vec<_>, _>>()?;
    for row in &mut rows {
        let selected = compiled
            .iter()
            .rev()
            .find(|(rule, expression)| matches_compiled(rule, expression, row, endpoint))
            .map(|(rule, _)| *rule);
        if let Some(object) = row.as_object_mut() {
            object.insert(
                "isFavorite".into(),
                json!(selected.is_some_and(|rule| rule.favorite)),
            );
            object.insert(
                "isHidden".into(),
                json!(selected.is_some_and(|rule| rule.hidden)),
            );
            object.insert("matchedRuleId".into(), json!(selected.map(|rule| &rule.id)));
            object.insert(
                "matchedRuleName".into(),
                json!(selected.map(|rule| &rule.name)),
            );
            object.insert(
                "ruleTextColor".into(),
                json!(selected.and_then(|rule| rule.text_color.as_ref())),
            );
            object.insert(
                "ruleBackgroundColor".into(),
                json!(selected.and_then(|rule| rule.background_color.as_ref())),
            );
            object.insert(
                "ruleMute".into(),
                json!(selected.is_none_or(|rule| rule.mute)),
            );
            object.insert(
                "ruleSoundPath".into(),
                json!(selected.and_then(|rule| rule.sound_path.as_ref())),
            );
        }
    }
    Ok(rows)
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RulePreviewChannel {
    pub channel_id: String,
    pub name: String,
    pub rule_id: String,
    pub rule_name: String,
    pub is_favorite: bool,
    pub is_hidden: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RulePreview {
    pub total: usize,
    pub matched: usize,
    pub favorites: usize,
    pub hidden: usize,
    pub channels: Vec<RulePreviewChannel>,
}

pub fn preview_rules(
    rows: &[Value],
    rules: &[ChannelRule],
    endpoint: &str,
) -> Result<RulePreview, String> {
    let rows = decorate_channels(rows.to_vec(), rules, endpoint)?;
    let matched: Vec<_> = rows
        .iter()
        .filter(|row| row["matchedRuleId"].is_string())
        .collect();
    Ok(RulePreview {
        total: rows.len(),
        matched: matched.len(),
        favorites: matched
            .iter()
            .filter(|row| row["isFavorite"] == true)
            .count(),
        hidden: matched.iter().filter(|row| row["isHidden"] == true).count(),
        channels: matched
            .into_iter()
            .take(100)
            .map(|row| RulePreviewChannel {
                channel_id: row["channelId"].as_str().unwrap_or_default().into(),
                name: row["name"].as_str().unwrap_or_default().into(),
                rule_id: row["matchedRuleId"].as_str().unwrap_or_default().into(),
                rule_name: row["matchedRuleName"].as_str().unwrap_or_default().into(),
                is_favorite: row["isFavorite"] == true,
                is_hidden: row["isHidden"] == true,
            })
            .collect(),
    })
}

#[cfg(test)]
mod quick_rule_tests {
    use super::*;

    #[test]
    fn quick_channel_rule_is_exact_idempotent_and_switchable() {
        let mut rules = Vec::new();
        upsert_quick_channel_rule(&mut rules, "Ａ＋Ｂ [test]", "favorite").unwrap();
        assert_eq!(rules.len(), 1);
        assert!(rules[0].favorite);
        assert!(!rules[0].hidden);
        assert!(rule_matches(&rules[0], &json!({ "name": "A+B [test]" }), "").unwrap());
        assert!(!rule_matches(&rules[0], &json!({ "name": "A+B [test] extra" }), "").unwrap());

        upsert_quick_channel_rule(&mut rules, "Ａ＋Ｂ [test]", "ng").unwrap();
        assert_eq!(rules.len(), 1);
        assert!(!rules[0].favorite);
        assert!(rules[0].hidden);
    }
}
