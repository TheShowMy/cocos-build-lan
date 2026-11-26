use std::collections::BTreeMap;

use serde_json::Value;

use crate::models::{Engine, ParamDefinition, Project};

fn normalize_placeholder_path(path: &str) -> String {
    path.replace('\\', "/")
}

#[derive(Debug, Clone)]
pub struct PlaceholderContext {
    pub project_path: String,
    replacements: BTreeMap<String, String>,
    params: BTreeMap<String, Value>,
}

impl PlaceholderContext {
    pub fn new(
        project: &Project,
        engine: &Engine,
        project_branch: String,
        project_path: String,
        code_package_path: String,
        remote_package_path: String,
    ) -> Self {
        Self::new_with_params(
            project,
            engine,
            project_branch,
            project_path,
            code_package_path,
            remote_package_path,
            &BTreeMap::new(),
        )
    }

    pub fn new_with_params(
        project: &Project,
        engine: &Engine,
        project_branch: String,
        project_path: String,
        code_package_path: String,
        remote_package_path: String,
        params: &BTreeMap<String, Value>,
    ) -> Self {
        let _ = project;
        let project_path = normalize_placeholder_path(&project_path);
        let replacements = BTreeMap::from([
            ("project_branch".into(), project_branch),
            ("project_path".into(), project_path.clone()),
            (
                "engine_path".into(),
                normalize_placeholder_path(&engine.path),
            ),
            (
                "code_package_path".into(),
                normalize_placeholder_path(&code_package_path),
            ),
            (
                "remote_package_path".into(),
                normalize_placeholder_path(&remote_package_path),
            ),
            (
                "code_repo_path".into(),
                normalize_placeholder_path(&code_package_path),
            ),
            (
                "asset_repo_path".into(),
                normalize_placeholder_path(&remote_package_path),
            ),
        ]);
        Self {
            project_path,
            replacements,
            params: params.clone(),
        }
        .with_definitions(&[])
    }

    pub fn with_definitions(mut self, definitions: &[ParamDefinition]) -> Self {
        self.replacements
            .retain(|key, _| super::parameters::SYSTEM_KEYS.contains(&key.as_str()));
        // Parameters cannot override the reserved system context even for invalid legacy settings.
        for (key, value) in super::parameters::render_aliases(definitions, &self.params) {
            if !super::parameters::SYSTEM_KEYS.contains(&key.as_str()) {
                self.replacements.insert(key, value);
            }
        }
        self
    }

    pub fn replace_text(&self, input: &str) -> String {
        let mut remaining = input;
        let mut output = String::with_capacity(input.len());
        while let Some(start) = remaining.find("${") {
            output.push_str(&remaining[..start]);
            let token = &remaining[start..];
            let Some(end) = token.find('}') else {
                output.push_str(token);
                return output;
            };
            match self.replacements.get(&token[2..end]) {
                Some(value) => output.push_str(value),
                None => output.push_str(&token[..=end]),
            }
            remaining = &token[end + 1..];
        }
        output.push_str(remaining);
        output
    }
}

#[cfg(test)]
mod tests {
    use crate::models::{BuildMode, Engine, Project};

    use super::*;

    fn sample_context() -> PlaceholderContext {
        PlaceholderContext::new_with_params(
            &Project {
                id: "project_1".to_string(),
                name: "demo".to_string(),
                workspace_dir_key: "workspace_1".to_string(),
                git_url: "https://example.com/demo.git".to_string(),
                engine_name: "3.8.8".to_string(),
                version: "6.7.3.1".to_string(),
                minor_version: 4,
                build_mode: BuildMode::Test,
                is_hot_update: true,
                enable_pay: false,
                review_mode: true,
            },
            &Engine {
                name: "3.8.8".to_string(),
                path: "C:\\engine\\Creator.exe".to_string(),
            },
            "main".to_string(),
            "C:\\workspace\\project".to_string(),
            "C:\\workspace\\code".to_string(),
            "C:\\workspace\\asset".to_string(),
            &BTreeMap::from([
                ("version".into(), serde_json::json!("6.7.3.1")),
                ("minor_version".into(), serde_json::json!(4)),
                ("build_mode".into(), serde_json::json!("test")),
                ("review_mode".into(), serde_json::json!(true)),
            ]),
        )
        .with_definitions(&[
            ParamDefinition {
                key: "build_mode".into(),
                kind: crate::models::ParamKind::Identifier,
                ..Default::default()
            },
            ParamDefinition {
                key: "review_mode".into(),
                kind: crate::models::ParamKind::Switch,
                ..Default::default()
            },
        ])
    }

    #[test]
    fn version_prefix_requires_group_context_and_uses_last_separator() {
        let standalone = PlaceholderContext::new(
            &Project {
                version: "9.9.9.9".into(),
                ..Default::default()
            },
            &Engine::default(),
            "main".into(),
            "path".into(),
            "".into(),
            "".into(),
        );
        assert_eq!(
            standalone.replace_text("${version}/${version_prefix}/${param.version}"),
            "${version}/${version_prefix}/${param.version}"
        );
        for value in ["1.2", "1.2.3", "6.8.0.0"] {
            let context = PlaceholderContext::new_with_params(
                &Project::default(),
                &Engine::default(),
                "main".into(),
                "path".into(),
                "".into(),
                "".into(),
                &BTreeMap::from([("version".into(), serde_json::json!(value))]),
            );
            assert_eq!(
                context.replace_text("${version_prefix}"),
                crate::services::parameters::version_prefix(value)
            );
        }
    }

    #[test]
    fn replace_text_should_render_all_placeholders() {
        let context = sample_context();
        let rendered = context.replace_text(
            "${version}|${version_prefix}|${minor_version}|${build_mode}|${BUILD_MODE}|${review_mode}|${project_path}|${code_package_path}|${code_repo_path}|${asset_repo_path}",
        );

        assert_eq!(
            rendered,
            "6.7.3.1|6.7.3|4|test|TEST|true|C:/workspace/project|C:/workspace/code|C:/workspace/code|C:/workspace/asset"
        );
    }

    #[test]
    fn replace_text_should_render_dynamic_parameters() {
        let mut params = BTreeMap::new();
        params.insert("channel".to_owned(), Value::String("official".to_owned()));
        let context = PlaceholderContext::new_with_params(
            &Project::default(),
            &Engine::default(),
            "main".to_owned(),
            String::new(),
            String::new(),
            String::new(),
            &params,
        );
        assert_eq!(context.replace_text("${param.channel}"), "official");
    }
    #[test]
    fn replacement_is_single_pass_and_manual_context_has_no_user_params() {
        let params = BTreeMap::from([("channel".into(), serde_json::json!("${project_path}"))]);
        let context = PlaceholderContext::new_with_params(
            &Project::default(),
            &Engine::default(),
            String::new(),
            "/project".into(),
            String::new(),
            String::new(),
            &params,
        );
        assert_eq!(
            context.replace_text("${channel}|${param.channel}|${missing}"),
            "${project_path}|${project_path}|${missing}"
        );
        let context = PlaceholderContext::new(
            &Project::default(),
            &Engine::default(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        );
        assert_eq!(
            context.replace_text("${build_mode}|${param.version}"),
            "${build_mode}|${param.version}"
        );
    }
}
