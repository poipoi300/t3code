use crate::Git;

impl Git {
    pub async fn is_worktree_container(&self) -> bool {
        false
    }
}
