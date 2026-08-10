use std::collections::HashSet;

use serde_json::Value;

use crate::{
    error::AppError,
    models::{AppSettings, ParamDefinition, ParamKind, PublicSettingsUpdate},
    state::AppState,
};

pub async fn save_public_settings(
    state: &AppState,
    mut update: PublicSettingsUpdate,
) -> Result<AppSettings, AppError> {
    normalize_update(&mut update);
    validate_update(&update)?;

    let mut settings = state.get_settings().await;
    validate_references(&settings, &update)?;

    settings.engines = update.engines;
    settings.projects = update.projects;
    settings.feishu_bots = update.feishu_bots;
    settings.param_definitions = update.param_definitions;
    normalize_group_params(&mut settings)?;
    state.save_settings(settings).await?;
    Ok(state.get_settings().await)
}

fn normalize_update(update: &mut PublicSettingsUpdate) {
    for engine in &mut update.engines {
        engine.name = engine.name.trim().to_owned();
        engine.path = engine.path.trim().to_owned();
    }
    for project in &mut update.projects {
        project.id = project.id.trim().to_owned();
        if project.id.is_empty() {
            project.id = format!("project_{}", uuid::Uuid::new_v4().simple());
        }
        project.name = project.name.trim().to_owned();
        project.workspace_dir_key = project.workspace_dir_key.trim().to_owned();
        project.git_url = project.git_url.trim().to_owned();
        project.engine_name = project.engine_name.trim().to_owned();
    }
    for bot in &mut update.feishu_bots {
        bot.id = bot.id.trim().to_owned();
        if bot.id.is_empty() {
            bot.id = format!("bot_{}", uuid::Uuid::new_v4().simple());
        }
        bot.name = bot.name.trim().to_owned();
        bot.api_key = bot.api_key.trim().to_owned();
    }
    for (order, definition) in update.param_definitions.iter_mut().enumerate() {
        definition.key = definition.key.trim().to_owned();
        definition.label = definition.label.trim().to_owned();
        definition.description = definition.description.trim().to_owned();
        definition.options = definition
            .options
            .iter()
            .map(|option| option.trim().to_owned())
            .filter(|option| !option.is_empty())
            .collect();
        definition.order = order as u32;
        if definition.kind != ParamKind::Select {
            definition.options.clear();
        }
        if definition.kind == ParamKind::Number {
            normalize_integral_number(&mut definition.default_value);
        }
    }
}

fn validate_update(update: &PublicSettingsUpdate) -> Result<(), AppError> {
    let mut engine_names = HashSet::new();
    for engine in &update.engines {
        if engine.name.is_empty() || engine.path.is_empty() {
            return Err(AppError::validation("引擎名称和路径不能为空"));
        }
        if !engine_names.insert(engine.name.to_lowercase()) {
            return Err(AppError::conflict("引擎名称不能重复"));
        }
    }

    let mut project_ids = HashSet::new();
    let mut project_names = HashSet::new();
    for project in &update.projects {
        if project.name.is_empty() || project.git_url.is_empty() || project.engine_name.is_empty() {
            return Err(AppError::validation("项目名称、Git 地址和关联引擎不能为空"));
        }
        if !project_ids.insert(project.id.clone()) {
            return Err(AppError::conflict("项目 ID 不能重复"));
        }
        if !project_names.insert(project.name.to_lowercase()) {
            return Err(AppError::conflict("项目名称不能重复"));
        }
        if !engine_names.contains(&project.engine_name.to_lowercase()) {
            return Err(AppError::validation(format!(
                "项目 {} 引用了不存在的引擎 {}",
                project.name, project.engine_name
            )));
        }
    }

    let mut bot_ids = HashSet::new();
    let mut bot_names = HashSet::new();
    for bot in &update.feishu_bots {
        if bot.name.is_empty() || bot.api_key.is_empty() {
            return Err(AppError::validation("飞书机器人名称和 webhook 不能为空"));
        }
        if !bot
            .api_key
            .starts_with("https://open.feishu.cn/open-apis/bot/v2/hook/")
        {
            return Err(AppError::validation("飞书 webhook 地址格式无效"));
        }
        if !bot_ids.insert(bot.id.clone()) || !bot_names.insert(bot.name.to_lowercase()) {
            return Err(AppError::conflict("飞书机器人名称和 ID 不能重复"));
        }
    }

    let mut parameter_keys = HashSet::new();
    for definition in &update.param_definitions {
        validate_definition(definition)?;
        if !parameter_keys.insert(definition.key.clone()) {
            return Err(AppError::conflict("任务参数 key 不能重复"));
        }
    }
    Ok(())
}

fn validate_definition(definition: &ParamDefinition) -> Result<(), AppError> {
    if definition.label.is_empty() || !valid_parameter_key(&definition.key) {
        return Err(AppError::validation(
            "任务参数显示名不能为空，key 必须是字母或下划线开头的标识符",
        ));
    }
    if definition.required && is_empty(&definition.default_value) {
        return Err(AppError::validation(format!(
            "必填参数 {} 必须提供默认值",
            definition.label
        )));
    }
    if !is_empty(&definition.default_value)
        && !value_matches_kind(&definition.default_value, &definition.kind)
    {
        return Err(AppError::validation(format!(
            "参数 {} 的默认值类型不正确",
            definition.label
        )));
    }
    if definition.kind == ParamKind::Select {
        if definition.options.is_empty() {
            return Err(AppError::validation(format!(
                "选择参数 {} 至少需要一个选项",
                definition.label
            )));
        }
        let unique = definition.options.iter().collect::<HashSet<_>>();
        if unique.len() != definition.options.len() {
            return Err(AppError::validation(format!(
                "选择参数 {} 的选项不能重复",
                definition.label
            )));
        }
        if let Some(value) = definition.default_value.as_str()
            && !definition.options.iter().any(|option| option == value)
        {
            return Err(AppError::validation(format!(
                "参数 {} 的默认值不在选项中",
                definition.label
            )));
        }
    }
    Ok(())
}

fn validate_references(
    settings: &AppSettings,
    update: &PublicSettingsUpdate,
) -> Result<(), AppError> {
    let project_ids = update
        .projects
        .iter()
        .map(|project| project.id.as_str())
        .collect::<HashSet<_>>();
    for group in &settings.task_groups {
        if !project_ids.contains(group.project_id.as_str()) {
            return Err(AppError::conflict(format!(
                "项目仍被任务组 {} 使用，不能删除",
                group.name
            )));
        }
    }
    Ok(())
}

fn normalize_group_params(settings: &mut AppSettings) -> Result<(), AppError> {
    for group in &mut settings.task_groups {
        let mut params = std::collections::BTreeMap::new();
        for definition in &settings.param_definitions {
            let mut value = group
                .params
                .get(&definition.key)
                .filter(|value| is_empty(value) || value_matches_kind(value, &definition.kind))
                .cloned()
                .unwrap_or_else(|| definition.default_value.clone());
            if definition.kind == ParamKind::Number {
                normalize_integral_number(&mut value);
            }
            if definition.required && is_empty(&value) {
                return Err(AppError::validation(format!(
                    "任务组 {} 的参数 {} 缺少有效值",
                    group.name, definition.label
                )));
            }
            params.insert(definition.key.clone(), value);
        }
        group.params = params;
    }
    Ok(())
}

pub(crate) fn normalize_integral_number(value: &mut Value) -> bool {
    let Some(number) = value.as_number() else {
        return false;
    };
    if !number.is_f64() {
        return false;
    }
    let Some(float) = number.as_f64() else {
        return false;
    };
    if !float.is_finite() || float.fract() != 0.0 {
        return false;
    }

    let integer = float.to_string();
    let normalized = integer
        .parse::<i64>()
        .map(Value::from)
        .or_else(|_| integer.parse::<u64>().map(Value::from));
    let Ok(normalized) = normalized else {
        return false;
    };
    *value = normalized;
    true
}

fn valid_parameter_key(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn value_matches_kind(value: &Value, kind: &ParamKind) -> bool {
    match kind {
        ParamKind::Text | ParamKind::Select => value.is_string(),
        ParamKind::Number => value.is_number(),
        ParamKind::Switch => value.is_boolean(),
    }
}

fn is_empty(value: &Value) -> bool {
    value.is_null() || value.as_str().is_some_and(|value| value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Engine, Project};

    #[test]
    fn rejects_duplicate_engines_and_invalid_webhook() {
        let duplicate = PublicSettingsUpdate {
            engines: vec![
                Engine {
                    name: "Cocos".to_owned(),
                    path: "a".to_owned(),
                },
                Engine {
                    name: "cocos".to_owned(),
                    path: "b".to_owned(),
                },
            ],
            ..PublicSettingsUpdate::default()
        };
        assert!(validate_update(&duplicate).is_err());
    }

    #[test]
    fn project_requires_existing_engine() {
        let update = PublicSettingsUpdate {
            projects: vec![Project {
                id: "project_1".to_owned(),
                name: "demo".to_owned(),
                git_url: "https://example.com/demo.git".to_owned(),
                engine_name: "missing".to_owned(),
                ..Project::default()
            }],
            ..PublicSettingsUpdate::default()
        };
        assert!(validate_update(&update).is_err());
    }

    #[test]
    fn parameter_key_and_type_are_validated() {
        let definition = ParamDefinition {
            key: "bad-key".to_owned(),
            label: "坏参数".to_owned(),
            default_value: Value::String("x".to_owned()),
            ..ParamDefinition::default()
        };
        assert!(validate_definition(&definition).is_err());
    }

    #[test]
    fn integral_floats_are_normalized_without_changing_decimals() {
        let mut integer = serde_json::json!(7.0);
        let mut decimal = serde_json::json!(7.5);

        assert!(normalize_integral_number(&mut integer));
        assert_eq!(integer, serde_json::json!(7));
        assert!(!normalize_integral_number(&mut decimal));
        assert_eq!(decimal, serde_json::json!(7.5));
    }
}
