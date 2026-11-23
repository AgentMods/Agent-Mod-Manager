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
    /// Forbidden is rejected by the dispatcher; AskUser waits for an answer, not write consent.
    pub fn needs_approval(self, effect: Effect) -> bool {
        match effect {
            Effect::Read => false,
            Effect::AskUser | Effect::Forbidden => true,
            Effect::GameWrite
            | Effect::ManagerWrite
            | Effect::ExternalWrite
            | Effect::Publish
            | Effect::UncatalogedDownload => match self {
                Self::RequestApproval => true,
                Self::AutoApprove => effect != Effect::GameWrite,
                Self::FullTrust => false,
            },
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
            assert_eq!(
                mode.needs_approval(Effect::GameWrite),
                mode == PermissionMode::RequestApproval
            );
            for effect in [
                Effect::ManagerWrite,
                Effect::ExternalWrite,
                Effect::Publish,
                Effect::UncatalogedDownload,
            ] {
                assert_eq!(
                    mode.needs_approval(effect),
                    mode != PermissionMode::FullTrust
                );
            }
        }
    }
}
