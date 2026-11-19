use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    #[default]
    RequestApproval,
    AutoApprove,
    FullTrust,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Read,
    GameWrite,
    ManagerWrite,
    ExternalWrite,
    Publish,
    UncatalogedDownload,
    AskUser,
    Forbidden,
}

impl PermissionMode {
    pub fn needs_approval(self, effect: Effect) -> bool {
        match effect {
            Effect::Read => false,
            Effect::AskUser | Effect::Forbidden | Effect::UncatalogedDownload => true,
            _ if self == Self::FullTrust => false,
            Effect::GameWrite if self == Self::AutoApprove => false,
            _ => true,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_permission_matrix() {
        for mode in [
            PermissionMode::RequestApproval,
            PermissionMode::AutoApprove,
            PermissionMode::FullTrust,
        ] {
            assert!(!mode.needs_approval(Effect::Read));
            assert!(mode.needs_approval(Effect::AskUser));
            assert!(mode.needs_approval(Effect::Forbidden));
            assert!(mode.needs_approval(Effect::UncatalogedDownload));
            assert_eq!(
                mode.needs_approval(Effect::GameWrite),
                mode == PermissionMode::RequestApproval
            );
            for effect in [Effect::ManagerWrite, Effect::ExternalWrite, Effect::Publish] {
                assert_eq!(
                    mode.needs_approval(effect),
                    mode != PermissionMode::FullTrust
                );
            }
        }
    }
}
