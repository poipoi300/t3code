use super::super::{Registry, register};
use t3_contracts::pull_request_review_guide::*;
pub fn register_module(registry: &mut Registry) {
    register::<PullRequestReviewGuide>(registry, "PullRequestReviewGuide");
    register::<PullRequestReviewGuideAttention>(registry, "PullRequestReviewGuideAttention");
    register::<PullRequestReviewGuideExcerpt>(registry, "PullRequestReviewGuideExcerpt");
    register::<PullRequestReviewGuideNote>(registry, "PullRequestReviewGuideNote");
    register::<PullRequestReviewGuideResult>(registry, "PullRequestReviewGuideResult");
    register::<PullRequestReviewGuideRun>(registry, "PullRequestReviewGuideRun");
    register::<PullRequestReviewGuideStop>(registry, "PullRequestReviewGuideStop");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("pull_request_review_guide", register_module)
}
