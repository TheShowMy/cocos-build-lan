use std::collections::BTreeMap;

use serde_json::Value;

use crate::models::{Engine, Project};

fn normalize_placeholder_path(path: &str) -> String {
    path.replace('\\', "/")
}

#[derive(Debug, Clone)]
pub struct PlaceholderContext {
    pub version: String,
    pub version_prefix: String,
    pub minor_version: String,
    pub build_mode: String,
    pub build_mode_uppercase: String,
    pub is_hot_update: String,
    pub enable_pay: String,
    pub review_mode: String,
    pub project_branch: String,
    pub project_path: String,
    pub engine_path: String,
    pub code_package_path: String,
    pub remote_package_path: String,
    pub params: BTreeMap<String, String>,
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
        let version_prefix = project
            .version
            .split('.')
            .take(3)
            .collect::<Vec<_>>()
            .join(".");
        let build_mode = project.build_mode.as_str().to_string();
        let build_mode_uppercase = build_mode.to_uppercase();

        Self {
            version: project.version.clone(),
            version_prefix,
            minor_version: project.minor_version.to_string(),
            build_mode,
            build_mode_uppercase,
            is_hot_update: if project.is_hot_update {
                "true".to_string()
            } else {
                "false".to_string()
            },
            enable_pay: if project.enable_pay {
                "true".to_string()
            } else {
                "false".to_string()
            },
            review_mode: if project.review_mode {
                "true".to_string()
            } else {
                "false".to_string()
            },
            project_branch,
            project_path: normalize_placeholder_path(&project_path),
            engine_path: normalize_placeholder_path(&engine.path),
            code_package_path: normalize_placeholder_path(&code_package_path),
            remote_package_path: normalize_placeholder_path(&remote_package_path),
            params: params
                .iter()
                .map(|(key, value)| (key.clone(), render_param_value(value)))
                .collect(),
        }
    }

    pub fn replace_text(&self, input: &str) -> String {
        let replacements = [
            ("${version}", self.version.as_str()),
            ("${version_prefix}", self.version_prefix.as_str()),
            ("${minor_version}", self.minor_version.as_str()),
            ("${build_mode}", self.build_mode.as_str()),
            ("${BUILD_MODE}", self.build_mode_uppercase.as_str()),
            ("${is_hot_update}", self.is_hot_update.as_str()),
            ("${enable_pay}", self.enable_pay.as_str()),
            ("${review_mode}", self.review_mode.as_str()),
            ("${project_branch}", self.project_branch.as_str()),
            ("${project_path}", self.project_path.as_str()),
            ("${engine_path}", self.engine_path.as_str()),
            ("${code_package_path}", self.code_package_path.as_str()),
            ("${remote_package_path}", self.remote_package_path.as_str()),
            ("${code_repo_path}", self.code_package_path.as_str()),
            ("${asset_repo_path}", self.remote_package_path.as_str()),
        ];

        let mut result = input.to_string();
        for (placeholder, value) in replacements {
            result = result.replace(placeholder, value);
        }
        for (key, value) in &self.params {
            result = result.replace(&format!("${{param.{key}}}"), value);
        }
        result
    }
}

fn render_param_value(value: &Value) -> String {
    value
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use crate::models::{BuildMode, Engine, Project};

    use super::*;

    fn sample_context() -> PlaceholderContext {
        PlaceholderContext::new(
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
        )
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
}
