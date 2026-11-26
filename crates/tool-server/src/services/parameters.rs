use std::collections::{BTreeMap, HashMap};

use serde_json::Value;

use crate::{
    error::AppError,
    models::{AppSettings, ParamDefinition, ParamKind},
};

pub const SYSTEM_KEYS: &[&str] = &[
    "project_branch",
    "project_path",
    "engine_path",
    "code_package_path",
    "remote_package_path",
    "code_repo_path",
    "asset_repo_path",
];

pub fn aliases(definition: &ParamDefinition) -> Vec<String> {
    if definition.kind == ParamKind::Version {
        vec![definition.key.clone(), format!("{}_prefix", definition.key)]
    } else if matches!(definition.kind, ParamKind::Identifier | ParamKind::Switch) {
        vec![
            definition.key.to_ascii_lowercase(),
            definition.key.to_ascii_uppercase(),
        ]
    } else {
        vec![definition.key.clone()]
    }
}

pub fn validate_aliases(definitions: &[ParamDefinition]) -> Result<(), AppError> {
    let mut owners = HashMap::new();
    for definition in definitions {
        for name in aliases(definition) {
            if SYSTEM_KEYS.contains(&name.as_str()) {
                return Err(AppError::validation(format!(
                    "参数 {} 的引用 ${{{name}}} 与系统占位符冲突",
                    definition.key
                )));
            }
            if let Some(owner) = owners.insert(name.clone(), definition.key.clone())
                && owner != definition.key
            {
                return Err(AppError::validation(format!(
                    "参数 {owner} 与 {} 的引用 ${{{name}}} 冲突",
                    definition.key
                )));
            }
        }
    }
    Ok(())
}

pub fn normalize_identifier(
    definition: &ParamDefinition,
    value: &mut Value,
) -> Result<(), AppError> {
    if definition.kind == ParamKind::Version {
        if value.is_null() && !definition.required {
            *value = Value::String(String::new());
        }
        let valid = value
            .as_str()
            .is_some_and(|text| (!definition.required && text.is_empty()) || valid_version(text));
        return if valid {
            Ok(())
        } else {
            Err(AppError::validation(format!(
                "参数 {} 必须是至少两段数字组成的版本号，例如 1.2 或 1.2.3（不允许空格、首尾或连续点）",
                definition.key
            )))
        };
    }
    if definition.kind != ParamKind::Identifier {
        return Ok(());
    }
    if value.is_null() && !definition.required {
        *value = Value::String(String::new());
    }
    let Some(text) = value.as_str() else {
        return Err(AppError::validation(format!(
            "参数 {} 必须是英文标识",
            definition.key
        )));
    };
    if (definition.required && text.is_empty())
        || !text.bytes().all(|b| b.is_ascii_alphabetic() || b == b'_')
    {
        return Err(AppError::validation(format!(
            "参数 {} 只允许英文字母和下划线{}",
            definition.key,
            if definition.required {
                "，且不能为空"
            } else {
                ""
            }
        )));
    }
    let text = text.to_ascii_lowercase();
    if !text.is_empty() && !definition.options.is_empty() && !definition.options.contains(&text) {
        return Err(AppError::validation(format!(
            "参数 {} 的值 {text} 不在选项中",
            definition.key
        )));
    }
    *value = Value::String(text);
    Ok(())
}

pub fn valid_version(value: &str) -> bool {
    value.contains('.')
        && value
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

pub fn version_prefix(value: &str) -> &str {
    value
        .rsplit_once('.')
        .map(|(prefix, _)| prefix)
        .unwrap_or("")
}

pub fn builtin_version() -> ParamDefinition {
    ParamDefinition {
        key: "version".into(),
        label: "版本号".into(),
        kind: ParamKind::Version,
        default_value: Value::String("1.0.0".into()),
        required: true,
        description: "系统内置版本值；前缀用于资源仓库分支".into(),
        ..Default::default()
    }
}

pub fn normalize_definition(definition: &mut ParamDefinition) -> Result<(), AppError> {
    if definition.kind == ParamKind::Version {
        let mut value = definition.default_value.clone();
        normalize_identifier(definition, &mut value)?;
        definition.default_value = value;
        return Ok(());
    }
    if definition.kind != ParamKind::Identifier {
        return Ok(());
    }
    let mut seen = std::collections::HashSet::new();
    for option in &mut definition.options {
        if option.is_empty() || !option.bytes().all(|b| b.is_ascii_alphabetic() || b == b'_') {
            return Err(AppError::validation(format!(
                "参数 {} 的选项 {option:?} 只允许英文字母和下划线",
                definition.key
            )));
        }
        *option = option.to_ascii_lowercase();
        if !seen.insert(option.clone()) {
            return Err(AppError::validation(format!(
                "参数 {} 的选项转小写后重复: {option}",
                definition.key
            )));
        }
    }
    let mut value = definition.default_value.clone();
    normalize_identifier(definition, &mut value)?;
    definition.default_value = value;
    Ok(())
}

/// Validate a copy before replacing any saved values, including the existing build_mode definition.
pub fn normalize_identifiers(settings: &mut AppSettings) -> Result<bool, AppError> {
    let mut definitions = settings.param_definitions.clone();
    let mut groups = settings.task_groups.clone();
    if definitions.iter().filter(|d| d.key == "version").count() > 1 {
        return Err(AppError::validation("内置 version 定义重复，请修正配置"));
    }
    let previous_version = definitions
        .iter()
        .find(|d| d.key == "version")
        .map(|d| d.default_value.clone())
        .unwrap_or_else(|| builtin_version().default_value);
    normalize_identifier(&builtin_version(), &mut previous_version.clone())
        .map_err(|e| AppError::validation(format!("内置 version 默认值: {e}")))?;
    definitions.retain(|d| d.key != "version");
    definitions.push(builtin_version());
    for definition in &mut definitions {
        if definition.key == "build_mode" && definition.kind == ParamKind::Select {
            definition.kind = ParamKind::Identifier;
        }
        normalize_definition(definition)?;
    }
    validate_aliases(&definitions)?;
    for group in &mut groups {
        group
            .params
            .entry("version".into())
            .or_insert_with(|| previous_version.clone());
        normalize_values(&definitions, &mut group.params, true)
            .map_err(|e| AppError::validation(format!("任务组 {}: {e}", group.name)))?;
        for preset in &mut group.presets {
            normalize_values(&definitions, &mut preset.params, false).map_err(|e| {
                AppError::validation(format!("任务组 {} / 预设 {}: {e}", group.name, preset.name))
            })?;
        }
        if group.schemes.as_ref().is_none_or(Vec::is_empty) {
            let names = if group.schemes.is_none() {
                vec!["pro", "test"]
            } else {
                vec!["pro"]
            };
            group.schemes = Some(
                names
                    .into_iter()
                    .map(|name| crate::models::BuildScheme {
                        id: format!("scheme_{}", uuid::Uuid::new_v4().simple()),
                        name: name.into(),
                        branch: group.branch.clone(),
                        params: group.params.clone(),
                        task_ids: vec![],
                    })
                    .collect(),
            );
        }
        for scheme in group.schemes.iter_mut().flatten() {
            normalize_values(&definitions, &mut scheme.params, true).map_err(|e| {
                AppError::validation(format!("任务组 {} / 方案 {}: {e}", group.name, scheme.name))
            })?;
            scheme.task_ids.retain(|id| {
                settings
                    .package_tasks
                    .iter()
                    .any(|task| task.id == *id && task.task_group_id == group.id)
            });
        }
    }
    let changed = definitions != settings.param_definitions || groups != settings.task_groups;
    settings.param_definitions = definitions;
    settings.task_groups = groups;
    Ok(changed)
}

pub fn normalize_values(
    definitions: &[ParamDefinition],
    params: &mut BTreeMap<String, Value>,
    complete: bool,
) -> Result<(), AppError> {
    params.retain(|key, _| definitions.iter().any(|d| d.key == *key));
    for definition in definitions {
        if complete {
            params
                .entry(definition.key.clone())
                .or_insert_with(|| definition.default_value.clone());
        }
        if let Some(value) = params.get_mut(&definition.key) {
            crate::routes::task_groups::check_param_kind(definition, value)?;
        }
    }
    Ok(())
}

pub fn render_aliases(
    definitions: &[ParamDefinition],
    params: &BTreeMap<String, Value>,
) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    for (key, value) in params {
        let text = value
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| value.to_string());
        result.insert(format!("param.{key}"), text.clone());
        if key == "version"
            || definitions
                .iter()
                .any(|d| d.key == *key && d.kind == ParamKind::Version)
        {
            result.insert(format!("{key}_prefix"), version_prefix(&text).to_owned());
        }
        if let Some(definition) = definitions.iter().find(|d| &d.key == key)
            && matches!(definition.kind, ParamKind::Identifier | ParamKind::Switch)
        {
            result.insert(key.to_ascii_lowercase(), text.to_ascii_lowercase());
            result.insert(key.to_ascii_uppercase(), text.to_ascii_uppercase());
        } else {
            result.insert(key.clone(), text);
        }
    }
    result
}

pub fn placeholder_catalog(definitions: &[ParamDefinition]) -> Vec<crate::models::PlaceholderInfo> {
    let mut rows = Vec::new();
    for definition in definitions {
        let example = definition
            .default_value
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| definition.default_value.to_string());
        let params = BTreeMap::from([(definition.key.clone(), definition.default_value.clone())]);
        for (key, value) in render_aliases(std::slice::from_ref(definition), &params) {
            let detail = if key == format!("{}_prefix", definition.key)
                && definition.kind == ParamKind::Version
            {
                "只读前缀，删除最后一个点及后面的内容"
            } else if matches!(definition.kind, ParamKind::Identifier | ParamKind::Switch) {
                "按引用大小写输出；英文标识保存为小写"
            } else {
                "原始保存值"
            };
            rows.push(crate::models::PlaceholderInfo { reference: format!("${{{key}}}"), builtin: definition.key == "version",
                description: format!("{} · {} · {detail} · 来源：任务组参数，仅打包任务上下文可用；示例 {example} → {value}。{}", definition.label, serde_json::to_value(&definition.kind).unwrap(), definition.description) });
        }
    }
    for (key, description, example) in [
        (
            "project_branch",
            "工程分支，来自任务组或独立准备运行所选分支",
            "main",
        ),
        (
            "project_path",
            "工程路径；准备脚本 --project_path 自动注入",
            "D:/workspace/game",
        ),
        (
            "engine_path",
            "关联引擎路径",
            "D:/Cocos/Creator/3.8.8/CocosCreator.exe",
        ),
        (
            "code_package_path",
            "任务代码包路径（独立准备运行为空）",
            "D:/workspace/task/code",
        ),
        (
            "code_repo_path",
            "code_package_path 的别名",
            "D:/workspace/task/code",
        ),
        (
            "remote_package_path",
            "任务资源包路径（独立准备运行为空）",
            "D:/workspace/task/assets",
        ),
        (
            "asset_repo_path",
            "remote_package_path 的别名",
            "D:/workspace/task/assets",
        ),
    ] {
        rows.push(crate::models::PlaceholderInfo { reference: format!("${{{key}}}"), builtin: true,
            description: format!("系统上下文 · 文本 · {description}；来源：服务运行环境，构建与准备执行时可用。示例：{example}") });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn identifier() -> ParamDefinition {
        ParamDefinition {
            key: "build_mode".into(),
            kind: ParamKind::Identifier,
            default_value: json!("PROD"),
            ..Default::default()
        }
    }

    #[test]
    fn version_rules_aliases_and_collisions() {
        let mut definition = builtin_version();
        for (value, prefix) in [
            ("1.2", "1"),
            ("1.2.3", "1.2"),
            ("6.8.0.0", "6.8.0"),
            ("01.002", "01"),
        ] {
            let mut json = json!(value);
            normalize_identifier(&definition, &mut json).unwrap();
            assert_eq!(json, value);
            assert_eq!(version_prefix(value), prefix);
            assert_eq!(
                render_aliases(
                    &[definition.clone()],
                    &BTreeMap::from([("version".into(), json)])
                )["version_prefix"],
                prefix
            );
        }
        for value in ["", "1", ".1", "1.", "1..2", " 1.2", "1.2 ", "１.２", "a.b"] {
            assert!(normalize_identifier(&definition, &mut json!(value)).is_err());
        }
        definition.key = "sdk_version".into();
        definition.required = false;
        normalize_identifier(&definition, &mut json!("")).unwrap();
        let aliases = render_aliases(
            &[definition.clone()],
            &BTreeMap::from([("sdk_version".into(), json!(""))]),
        );
        assert_eq!(aliases["sdk_version_prefix"], "");
        assert_eq!(aliases["param.sdk_version"], "");
        assert!(
            validate_aliases(&[
                definition,
                ParamDefinition {
                    key: "sdk_version_prefix".into(),
                    ..Default::default()
                }
            ])
            .is_err()
        );
        assert!(
            validate_aliases(&[
                builtin_version(),
                ParamDefinition {
                    key: "version_prefix".into(),
                    ..Default::default()
                }
            ])
            .is_err()
        );
    }
    #[test]
    fn version_conversion_preserves_values_and_reports_locations() {
        let mut settings = AppSettings {
            param_definitions: vec![ParamDefinition {
                key: "version".into(),
                default_value: json!("01.2.3"),
                ..Default::default()
            }],
            task_groups: vec![crate::models::TaskGroup {
                name: "demo".into(),
                branch: "main".into(),
                presets: vec![crate::models::GroupParamPreset {
                    name: "partial".into(),
                    params: BTreeMap::from([("version".into(), json!("2.3"))]),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        normalize_identifiers(&mut settings).unwrap();
        assert_eq!(settings.param_definitions, vec![builtin_version()]);
        assert_eq!(settings.task_groups[0].params["version"], "01.2.3");
        assert_eq!(settings.task_groups[0].presets[0].params["version"], "2.3");
        settings.task_groups[0].schemes.as_mut().unwrap()[0]
            .params
            .insert("version".into(), json!("1."));
        let before = settings.task_groups.clone();
        let error = normalize_identifiers(&mut settings)
            .unwrap_err()
            .to_string();
        assert!(error.contains("demo") && error.contains("pro"));
        assert_eq!(before, settings.task_groups);
    }

    #[test]
    fn lowercase_values_options_and_validate_before_membership() {
        let mut definition = identifier();
        definition.options = vec!["PROD".into(), "Japan_Test".into()];
        normalize_definition(&mut definition).unwrap();
        assert_eq!(definition.default_value, json!("prod"));
        let mut value = json!("JAPAN_TEST");
        normalize_identifier(&definition, &mut value).unwrap();
        assert_eq!(value, json!("japan_test"));
        for invalid in ["a1", "a b", " 中文", " prod", ""] {
            definition.required = true;
            assert!(normalize_identifier(&definition, &mut json!(invalid)).is_err());
        }
        definition.options = vec!["prod".into(), "PROD".into()];
        assert!(normalize_definition(&mut definition).is_err());
    }

    #[test]
    fn aliases_are_typed_and_collisions_are_rejected() {
        let definitions = vec![
            identifier(),
            ParamDefinition {
                key: "enabled".into(),
                kind: ParamKind::Switch,
                ..Default::default()
            },
        ];
        let params = BTreeMap::from([
            ("build_mode".into(), json!("prod")),
            ("enabled".into(), json!(true)),
        ]);
        let aliases = render_aliases(&definitions, &params);
        assert_eq!(aliases["BUILD_MODE"], "PROD");
        assert_eq!(aliases["ENABLED"], "TRUE");
        assert_eq!(aliases["param.build_mode"], "prod");
        let mut conflicting = definitions;
        conflicting.push(ParamDefinition {
            key: "BUILD_MODE".into(),
            ..Default::default()
        });
        assert!(validate_aliases(&conflicting).is_err());
        assert!(
            validate_aliases(&[ParamDefinition {
                key: "project_path".into(),
                ..Default::default()
            }])
            .is_err()
        );
    }

    #[test]
    fn existing_groups_and_presets_are_normalized_atomically() {
        let mut settings = AppSettings {
            param_definitions: vec![identifier()],
            task_groups: vec![crate::models::TaskGroup {
                params: BTreeMap::from([("build_mode".into(), json!("PROD"))]),
                presets: vec![crate::models::GroupParamPreset {
                    params: BTreeMap::from([("build_mode".into(), json!("Japan_Test"))]),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        normalize_identifiers(&mut settings).unwrap();
        assert_eq!(
            settings.task_groups[0].presets[0].params["build_mode"],
            json!("japan_test")
        );
        settings.task_groups[0]
            .params
            .insert("build_mode".into(), json!("INVALID1"));
        assert!(normalize_identifiers(&mut settings).is_err());
        assert_eq!(
            settings.task_groups[0].params["build_mode"],
            json!("INVALID1")
        );
    }
}
