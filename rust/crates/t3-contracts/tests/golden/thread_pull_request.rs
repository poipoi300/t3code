use super::super::{Registry, register};
use t3_contracts::thread_pull_request::*;
pub fn register_module(registry: &mut Registry) {
    register::<ThreadLinkedPullRequest>(registry, "ThreadLinkedPullRequest");
    register::<ThreadPullRequestKey>(registry, "ThreadPullRequestKey");
    register::<ThreadPullRequestLink>(registry, "ThreadPullRequestLink");
    register::<ThreadPullRequestLinkSource>(registry, "ThreadPullRequestLinkSource");
    register::<ThreadPullRequestSnapshot>(registry, "ThreadPullRequestSnapshot");
    register::<ThreadPullRequestStack>(registry, "ThreadPullRequestStack");
    register::<ThreadPullRequestStackLayer>(registry, "ThreadPullRequestStackLayer");
    register::<ThreadPullRequestWatch>(registry, "ThreadPullRequestWatch");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("thread_pull_request", register_module)
}
