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
    /// 工具业务配置。可按项目需求扩展字段。
    pub business: BusinessSettings,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct NetworkSettings {
    /// 局域网业务页面与 API 的 TCP 端口；0 仅表示首次启动时尚未分配。
    pub lan_port: u16,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BusinessSettings {
    /// 仅由本机 Web 设置保存；读取 API 从不返回密码。
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
