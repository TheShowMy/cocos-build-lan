//! Tool-specific contracts belong here; framework lifecycle contracts are re-exported.

//! 用户项目拥有的业务配置和状态类型。
//!
//! 在这里增加字段、状态枚举或业务 DTO；控制端与服务端都直接使用这些本地类型。

use serde::{Deserialize, Serialize};

pub use cocos_build_lan_core::*;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolSettings {
    /// 本机网络配置。
    #[serde(default)]
    pub network: NetworkSettings,
    /// 更新与发布配置。
    pub update: UpdateSettings,
    /// 工具业务配置。可按项目需求扩展字段。
    pub business: BusinessSettings,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct NetworkSettings {
    /// 局域网业务页面与 API 的 TCP 端口；0 仅表示首次启动时尚未分配。
    pub lan_port: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateSettings {
    /// GitHub Releases（或兼容静态托管）上的完整版本包清单 URL。留空即关闭 Release 检查。
    pub release_manifest_url: String,
    /// 当 RestartReadiness 为 Ready 时自动切换已下载的更新。
    pub auto_apply_updates: bool,
    /// 是否允许控制端监听可信局域网内的 LAN Dev UDP 广播。
    pub lan_dev_enabled: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            release_manifest_url: String::new(),
            auto_apply_updates: true,
            lan_dev_enabled: false,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BusinessSettings {
    /// 仅由本机控制端保存；网页 API 从不返回密码。
    #[serde(default)]
    pub git_username: String,
    #[serde(default)]
    pub git_password: String,
    /// 构建工作区根目录；留空使用默认数据目录。请使用短路径（如 E:\lan-workspaces），
    /// 避免 Cocos Creator 因路径过长在自动图集步骤崩溃。
    #[serde(default)]
    pub workspace_root: String,
}

impl BusinessSettings {
    #[must_use]
    pub fn git_config(&self) -> GitConfig {
        GitConfig {
            username: self.git_username.clone(),
            password: self.git_password.clone(),
        }
    }

    pub fn set_git_config(&mut self, git_config: GitConfig) {
        self.git_username = git_config.username;
        self.git_password = git_config.password;
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct GitConfig {
    pub username: String,
    pub password: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolStatus {
    /// 给控制端展示的业务摘要。请替换成真实业务状态。
    pub summary: String,
    /// 模板示例业务指标；可按需删除或扩展。
    pub completed_jobs: u64,
}

impl Default for ToolStatus {
    fn default() -> Self {
        Self {
            summary: "服务正在运行，等待你的业务逻辑".to_owned(),
            completed_jobs: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_settings_gain_an_unassigned_network_port_and_drop_greeting() {
        let old = serde_json::json!({
            "update": {
                "release_manifest_url": "",
                "auto_apply_updates": true,
                "lan_dev_enabled": false
            },
            "business": {
                "git_username": "user",
                "git_password": "token",
                "greeting": "legacy"
            }
        });
        let settings: ToolSettings = serde_json::from_value(old).expect("old settings");
        assert_eq!(settings.network.lan_port, 0);
        assert_eq!(settings.business.git_username, "user");
        let serialized = serde_json::to_value(settings).expect("serialize settings");
        assert!(serialized["business"].get("greeting").is_none());
    }
}
