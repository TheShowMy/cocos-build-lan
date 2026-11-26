//! Persistent identity, data paths and active-operation guards.
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
};
use uuid::Uuid;
const BUSINESS_PORT_RANGE: (u16, u16) = (43_000, 44_999);
const PORT_CANDIDATE_COUNT: usize = 8;
pub fn business_port_candidates(tool_id: Uuid) -> Vec<u16> {
    port_candidates(tool_id, b"business", BUSINESS_PORT_RANGE)
}

#[must_use]
fn port_candidates(tool_id: Uuid, purpose: &[u8], range: (u16, u16)) -> Vec<u16> {
    let width = u32::from(range.1 - range.0) + 1;
    let mut ports = Vec::with_capacity(PORT_CANDIDATE_COUNT);
    let mut index = 0_u32;
    while ports.len() < PORT_CANDIDATE_COUNT {
        let mut hash = Sha256::new();
        hash.update(tool_id.as_bytes());
        hash.update(purpose);
        hash.update(index.to_le_bytes());
        let digest = hash.finalize();
        let offset = u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]]) % width;
        let port = range.0 + offset as u16;
        if !ports.contains(&port) {
            ports.push(port);
        }
        index += 1;
    }
    ports
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolIdentity {
    pub tool_id: Uuid,
    pub display_name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RestartBlocker {
    pub code: String,
    pub summary: String,
    pub started_at_unix_secs: u64,
    pub cancellable: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RestartReadiness {
    Ready,
    Deferred {
        reasons: Vec<RestartBlocker>,
        retry_after_secs: Option<u64>,
    },
    ConfirmationRequired {
        reasons: Vec<RestartBlocker>,
    },
    Blocked {
        reasons: Vec<RestartBlocker>,
    },
}

impl RestartReadiness {
    #[must_use]
    pub const fn permits_automatic_apply(&self) -> bool {
        matches!(self, Self::Ready)
    }

    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Deferred { .. } => "Deferred",
            Self::ConfirmationRequired { .. } => "ConfirmationRequired",
            Self::Blocked { .. } => "Blocked",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RestartProtection {
    Deferred,
    ConfirmationRequired,
    Blocked,
}

#[derive(Clone, Default)]
pub struct RestartRegistry {
    blockers: Arc<StdMutex<BTreeMap<Uuid, RegisteredBlocker>>>,
}

#[derive(Clone, Debug)]
struct RegisteredBlocker {
    protection: RestartProtection,
    blocker: RestartBlocker,
}

impl RestartRegistry {
    #[must_use]
    pub fn protect(&self, protection: RestartProtection, blocker: RestartBlocker) -> RestartGuard {
        let id = Uuid::new_v4();
        self.blockers
            .lock()
            .expect("restart registry lock poisoned")
            .insert(
                id,
                RegisteredBlocker {
                    protection,
                    blocker,
                },
            );
        RestartGuard {
            id,
            blockers: Arc::clone(&self.blockers),
        }
    }

    #[must_use]
    pub fn readiness(&self) -> RestartReadiness {
        let blockers = self
            .blockers
            .lock()
            .expect("restart registry lock poisoned");
        let reasons = |protection| {
            blockers
                .values()
                .filter(|entry| entry.protection == protection)
                .map(|entry| entry.blocker.clone())
                .collect::<Vec<_>>()
        };
        let blocked = reasons(RestartProtection::Blocked);
        if !blocked.is_empty() {
            return RestartReadiness::Blocked { reasons: blocked };
        }
        let confirmation = reasons(RestartProtection::ConfirmationRequired);
        if !confirmation.is_empty() {
            return RestartReadiness::ConfirmationRequired {
                reasons: confirmation,
            };
        }
        let deferred = reasons(RestartProtection::Deferred);
        if deferred.is_empty() {
            RestartReadiness::Ready
        } else {
            RestartReadiness::Deferred {
                reasons: deferred,
                retry_after_secs: Some(30),
            }
        }
    }
}

pub struct RestartGuard {
    id: Uuid,
    blockers: Arc<StdMutex<BTreeMap<Uuid, RegisteredBlocker>>>,
}

impl Drop for RestartGuard {
    fn drop(&mut self) {
        self.blockers
            .lock()
            .expect("restart registry lock poisoned")
            .remove(&self.id);
    }
}

#[derive(Clone, Debug)]
pub struct ToolPaths {
    root: PathBuf,
}
impl ToolPaths {
    pub fn for_tool(tool_id: Uuid) -> std::io::Result<Self> {
        let dirs = ProjectDirs::from("dev", "lan-toolkit", "lan-toolkit")
            .ok_or_else(|| std::io::Error::other("无法定位本机应用数据目录"))?;
        let root = std::env::var_os("COCOS_BUILD_LAN_DATA_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| dirs.data_local_dir().join(tool_id.to_string()));
        for name in ["config", "data", "logs"] {
            fs::create_dir_all(root.join(name))?;
        }
        Ok(Self { root })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn config_file(&self) -> PathBuf {
        self.root.join("config/tool-settings.json")
    }
    pub fn server_log(&self) -> PathBuf {
        self.root.join("logs/server.log")
    }
}
