#![allow(clippy::unwrap_used)]

use serde_json::{Value, json};
use t3_orchestration::{error::Result, ports::thread_project::thread_settlement_service::*};
// Pure decision cases from ThreadSettlementService.test.ts; background workers remain unported.
const ORACLE: &str = r###"{"count":15,"oracle":[{"name":"settles a thread whose only background work is a command left running","method":"isAutoSettlementCandidate","args":[{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[{"taskId":"dev","kind":"command","description":"vp run dev --share"}],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":null,"latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null},1781092800000],"expected":true},{"name":"settles a thread whose only background work is a command left running","method":"isAutoSettlementCandidate","args":[{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[{"taskId":"pull-request-watch:github.com/acme/app#1","kind":"monitor"}],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":null,"latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null},1781092800000],"expected":false},{"name":"never settles, or keeps snoozed, a thread whose guest is waiting on the owner","method":"isAutoSettlementCandidate","args":[{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":null,"latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null,"pendingGuestMessageCount":1},1781092800000],"expected":false},{"name":"never settles, or keeps snoozed, a thread whose guest is waiting on the owner","method":"isSnoozed","args":[{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":null,"latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":"2026-06-10T13:00:00.000Z","snoozedAt":"2026-06-10T11:00:00.000Z","pinnedAt":null,"pendingGuestMessageCount":1},1781092800000],"expected":false},{"name":"keeps snoozed threads parked until a fresh failure or timer wake","method":"isAutoSettlementCandidate","args":[{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":null,"latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":"2026-06-10T13:00:00.000Z","snoozedAt":"2026-06-10T11:00:00.000Z","pinnedAt":null},1781092800000],"expected":false},{"name":"keeps snoozed threads parked until a fresh failure or timer wake","method":"isAutoSettlementCandidate","args":[{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"failed","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":"2026-06-10T11:30:00.000Z","latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":"2026-06-10T13:00:00.000Z","snoozedAt":"2026-06-10T11:00:00.000Z","pinnedAt":null},1781092800000],"expected":true},{"name":"keeps snoozed threads parked until a fresh failure or timer wake","method":"isAutoSettlementCandidate","args":[{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"failed","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":"2026-06-10T10:00:00.000Z","latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":"2026-06-10T13:00:00.000Z","snoozedAt":"2026-06-10T11:00:00.000Z","pinnedAt":null},1781092800000],"expected":false},{"name":"keeps snoozed threads parked until a fresh failure or timer wake","method":"isAutoSettlementCandidate","args":[{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"failed","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":"2026-06-10T11:00:00.000Z","latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":"2026-06-10T13:00:00.000Z","snoozedAt":"2026-06-10T11:00:00.000Z","pinnedAt":null},1781092800000],"expected":false},{"name":"keeps snoozed threads parked until a fresh failure or timer wake","method":"isAutoSettlementCandidate","args":[{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"failed","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":null,"latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":"2026-06-10T13:00:00.000Z","snoozedAt":"2026-06-10T11:00:00.000Z","pinnedAt":null},1781092800000],"expected":false},{"name":"keeps snoozed threads parked until a fresh failure or timer wake","method":"isAutoSettlementCandidate","args":[{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":null,"latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":"2026-06-10T11:59:59.999Z","snoozedAt":"2026-06-10T11:00:00.000Z","pinnedAt":null},1781092800000],"expected":true},{"name":"keeps threads with a PR from inactivity settlement only when asked","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":{"projectId":"project-1","repository":"owner/repo","number":1,"url":"https://example.test/owner/repo/pull/1"},"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":null,"latestUserMessageAt":"2026-06-06T12:00:00.000Z","latestUserAuthoredMessageAt":"2026-06-06T12:00:00.000Z","createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null},"pullRequest":null,"nowMs":1781092800000,"autoSettleAfterDays":3,"autoSettleOnMerge":true,"autoSettleKeepPullRequests":true}],"expected":null},{"name":"keeps threads with a PR from inactivity settlement only when asked","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":{"projectId":"project-1","repository":"owner/repo","number":1,"url":"https://example.test/owner/repo/pull/1"},"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":null,"latestUserMessageAt":"2026-06-06T12:00:00.000Z","latestUserAuthoredMessageAt":"2026-06-06T12:00:00.000Z","createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null},"pullRequest":null,"nowMs":1781092800000,"autoSettleAfterDays":3,"autoSettleOnMerge":true,"autoSettleKeepPullRequests":false}],"expected":"2026-06-06T12:00:00.000Z"},{"name":"keeps threads with a PR from inactivity settlement only when asked","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":{"projectId":"project-1","repository":"owner/repo","number":1,"url":"https://example.test/owner/repo/pull/1"},"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":null,"latestUserMessageAt":"2026-06-06T12:00:00.000Z","latestUserAuthoredMessageAt":"2026-06-06T12:00:00.000Z","createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null},"pullRequest":{"state":"merged","mergedAt":"2026-06-10T11:59:59.999Z"},"nowMs":1781092800000,"autoSettleAfterDays":3,"autoSettleOnMerge":true,"autoSettleKeepPullRequests":true}],"expected":"2026-06-06T12:00:00.000Z"},{"name":"settles an idle thread with an open persisted link only while retention is off","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":null,"latestUserMessageAt":"2026-06-06T12:00:00.000Z","latestUserAuthoredMessageAt":"2026-06-06T12:00:00.000Z","createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null,"pullRequests":[{"host":"example.test","repository":"owner/repository","number":42,"url":"https://example.test/owner/repository/pull/42","source":"manual","linkedAt":"2026-06-05T12:00:00.000Z","snapshot":{"state":"open","title":"Pull request","headBranch":"feature","baseBranch":"main","isDraft":false,"updatedAt":"2026-06-05T12:00:00.000Z","syncedAt":"2026-06-05T12:00:00.000Z"},"stack":null}]},"pullRequest":null,"nowMs":1781092800000,"autoSettleAfterDays":3,"autoSettleOnMerge":true,"autoSettleKeepPullRequests":false}],"expected":"2026-06-06T12:00:00.000Z"},{"name":"settles an idle thread with an open persisted link only while retention is off","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":null,"latestUserMessageAt":"2026-06-06T12:00:00.000Z","latestUserAuthoredMessageAt":"2026-06-06T12:00:00.000Z","createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null,"pullRequests":[{"host":"example.test","repository":"owner/repository","number":42,"url":"https://example.test/owner/repository/pull/42","source":"manual","linkedAt":"2026-06-05T12:00:00.000Z","snapshot":{"state":"open","title":"Pull request","headBranch":"feature","baseBranch":"main","isDraft":false,"updatedAt":"2026-06-05T12:00:00.000Z","syncedAt":"2026-06-05T12:00:00.000Z"},"stack":null}]},"pullRequest":null,"nowMs":1781092800000,"autoSettleAfterDays":3,"autoSettleOnMerge":true,"autoSettleKeepPullRequests":true}],"expected":null},{"name":"settles an idle thread with an open persisted link only while retention is off","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":null,"latestUserMessageAt":"2026-06-06T12:00:00.000Z","latestUserAuthoredMessageAt":"2026-06-06T12:00:00.000Z","createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null,"pullRequests":[{"host":"example.test","repository":"owner/repository","number":42,"url":"https://example.test/owner/repository/pull/42","source":"manual","linkedAt":"2026-06-05T12:00:00.000Z","snapshot":{"state":"open","title":"Pull request","headBranch":"feature","baseBranch":"main","isDraft":false,"updatedAt":"2026-06-05T12:00:00.000Z","syncedAt":"2026-06-05T12:00:00.000Z"},"stack":null}]},"pullRequest":{"state":"merged","mergedAt":"2026-06-10T11:59:59.999Z"},"nowMs":1781092800000,"autoSettleAfterDays":null,"autoSettleOnMerge":true,"autoSettleKeepPullRequests":false}],"expected":null},{"name":"keeps an idle thread whose persisted link was never synced","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":null,"latestUserMessageAt":"2026-06-06T12:00:00.000Z","latestUserAuthoredMessageAt":"2026-06-06T12:00:00.000Z","createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null,"pullRequests":[{"host":"example.test","repository":"owner/repository","number":42,"url":"https://example.test/owner/repository/pull/42","source":"manual","linkedAt":"2026-06-05T12:00:00.000Z","snapshot":null,"stack":null}]},"pullRequest":null,"nowMs":1781092800000,"autoSettleAfterDays":3,"autoSettleOnMerge":true,"autoSettleKeepPullRequests":false}],"expected":null},{"name":"uses the latest activity time when the inactivity window elapses","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":"2026-06-06T12:00:00.000Z","latestRunStartedAt":"2026-06-06T12:00:00.000Z","latestRunCompletedAt":"2026-06-07T12:00:00.000Z","latestUserMessageAt":"2026-06-06T12:00:00.000Z","latestUserAuthoredMessageAt":"2026-06-06T12:00:00.000Z","createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null},"pullRequest":null,"nowMs":1781092800000,"autoSettleAfterDays":2,"autoSettleOnMerge":true}],"expected":"2026-06-07T12:00:00.000Z"},{"name":"uses the latest activity time when the inactivity window elapses","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":"2026-06-06T12:00:00.000Z","latestRunStartedAt":"2026-06-06T12:00:00.000Z","latestRunCompletedAt":"2026-06-07T12:00:00.000Z","latestUserMessageAt":"2026-06-06T12:00:00.000Z","latestUserAuthoredMessageAt":"2026-06-06T12:00:00.000Z","createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null},"pullRequest":null,"nowMs":1781092800000,"autoSettleAfterDays":5,"autoSettleOnMerge":true}],"expected":null},{"name":"uses the latest activity time when the inactivity window elapses","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":"2026-06-06T12:00:00.000Z","latestRunStartedAt":"2026-06-06T12:00:00.000Z","latestRunCompletedAt":"2026-06-07T12:00:00.000Z","latestUserMessageAt":"2026-06-06T12:00:00.000Z","latestUserAuthoredMessageAt":"2026-06-06T12:00:00.000Z","createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null},"pullRequest":null,"nowMs":1781092800000,"autoSettleAfterDays":null,"autoSettleOnMerge":true}],"expected":null},{"name":"anchors an empty thread's inactivity at its creation time","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":null,"latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-06-07T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null},"pullRequest":null,"nowMs":1781092800000,"autoSettleAfterDays":2,"autoSettleOnMerge":false}],"expected":"2026-06-07T12:00:00.000Z"},{"name":"anchors an empty thread's inactivity at its creation time","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":null,"latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-06-09T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null},"pullRequest":null,"nowMs":1781092800000,"autoSettleAfterDays":2,"autoSettleOnMerge":false}],"expected":null},{"name":"anchors an empty thread's inactivity at its creation time","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":null,"latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-06-07T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null,"lastSnoozeWakeAt":"2026-06-09T12:00:00.000Z"},"pullRequest":null,"nowMs":1781092800000,"autoSettleAfterDays":2,"autoSettleOnMerge":false}],"expected":null},{"name":"restarts inactivity on a manual snooze wake","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":"2026-06-06T12:00:00.000Z","latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null,"lastSnoozeWakeAt":"2026-06-10T11:59:59.000Z"},"pullRequest":null,"nowMs":1781092800000,"autoSettleAfterDays":1,"autoSettleOnMerge":false}],"expected":null},{"name":"restarts inactivity on a manual snooze wake","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":"2026-06-06T12:00:00.000Z","latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null,"lastSnoozeWakeAt":"2026-06-10T11:59:59.000Z"},"pullRequest":null,"nowMs":1781179200000,"autoSettleAfterDays":1,"autoSettleOnMerge":false}],"expected":"2026-06-10T11:59:59.000Z"},{"name":"restarts inactivity on a timer snooze wake","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":"2026-06-06T12:00:00.000Z","latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":"2026-06-10T11:59:59.000Z","snoozedAt":"2026-06-07T12:00:00.000Z","pinnedAt":null},"pullRequest":null,"nowMs":1781092800000,"autoSettleAfterDays":1,"autoSettleOnMerge":false}],"expected":null},{"name":"restarts inactivity on a timer snooze wake","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":"2026-06-06T12:00:00.000Z","latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":"2026-06-10T11:59:59.000Z","snoozedAt":"2026-06-07T12:00:00.000Z","pinnedAt":null},"pullRequest":null,"nowMs":1781179200000,"autoSettleAfterDays":1,"autoSettleOnMerge":false}],"expected":"2026-06-10T11:59:59.000Z"},{"name":"settles on merge only after the user's last action and preserves the activity time","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":"2026-06-10T10:30:00.000Z","latestUserMessageAt":"2026-06-10T10:00:00.000Z","latestUserAuthoredMessageAt":"2026-06-10T10:00:00.000Z","createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null},"pullRequest":{"state":"merged","mergedAt":"2026-06-10T11:00:00.000Z"},"nowMs":1781092800000,"autoSettleAfterDays":null,"autoSettleOnMerge":true}],"expected":"2026-06-10T10:30:00.000Z"},{"name":"settles on merge only after the user's last action and preserves the activity time","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":"2026-06-10T10:30:00.000Z","latestUserMessageAt":"2026-06-10T10:00:00.000Z","latestUserAuthoredMessageAt":"2026-06-10T10:00:00.000Z","createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null},"pullRequest":{"state":"merged","mergedAt":"2026-06-10T11:00:00.000Z"},"nowMs":1781092800000,"autoSettleAfterDays":null,"autoSettleOnMerge":false}],"expected":null},{"name":"settles on merge only after the user's last action and preserves the activity time","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":"2026-06-10T10:30:00.000Z","latestUserMessageAt":"2026-06-10T10:00:00.000Z","latestUserAuthoredMessageAt":"2026-06-10T10:00:00.000Z","createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null},"pullRequest":{"state":"merged","mergedAt":"2026-06-10T09:00:00.000Z"},"nowMs":1781092800000,"autoSettleAfterDays":null,"autoSettleOnMerge":true}],"expected":null},{"name":"settles on merge only after the user's last action and preserves the activity time","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":null,"latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null},"pullRequest":{"state":"closed","closedAt":"2026-06-10T11:59:59.999Z"},"nowMs":1781092800000,"autoSettleAfterDays":null,"autoSettleOnMerge":false}],"expected":"2026-05-11T12:00:00.000Z"},{"name":"settles on merge after agent-started runs, but not after the user writes again","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":"2026-06-10T11:30:00.000Z","latestRunStartedAt":null,"latestRunCompletedAt":"2026-06-10T11:31:00.000Z","latestUserMessageAt":"2026-06-10T11:30:00.000Z","latestUserAuthoredMessageAt":"2026-06-10T10:00:00.000Z","createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null},"pullRequest":{"state":"merged","mergedAt":"2026-06-10T11:00:00.000Z"},"nowMs":1781092800000,"autoSettleAfterDays":null,"autoSettleOnMerge":true}],"expected":"2026-06-10T11:31:00.000Z"},{"name":"settles on merge after agent-started runs, but not after the user writes again","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":"2026-06-10T11:30:00.000Z","latestRunStartedAt":null,"latestRunCompletedAt":"2026-06-10T11:31:00.000Z","latestUserMessageAt":"2026-06-10T11:30:00.000Z","latestUserAuthoredMessageAt":"2026-06-10T11:30:00.000Z","createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null},"pullRequest":{"state":"merged","mergedAt":"2026-06-10T11:00:00.000Z"},"nowMs":1781092800000,"autoSettleAfterDays":null,"autoSettleOnMerge":true}],"expected":null},{"name":"does not settle on pull requests that were already merged when linked","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":"2026-06-10T11:15:00.000Z","latestUserMessageAt":"2026-06-10T10:00:00.000Z","latestUserAuthoredMessageAt":"2026-06-10T10:00:00.000Z","createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null,"pullRequests":[{"host":"example.test","repository":"owner/repository","number":1,"url":"https://example.test/owner/repository/pull/1","source":"agent","linkedAt":"2026-06-10T11:10:00.000Z","snapshot":{"state":"merged","title":"Pull request","headBranch":"feature-1","baseBranch":"main","isDraft":false,"updatedAt":"2026-06-10T11:05:00.000Z","syncedAt":"2026-06-10T11:10:00.000Z","mergedAt":"2026-06-10T11:05:00.000Z"},"stack":null}]},"pullRequest":null,"nowMs":1781092800000,"autoSettleAfterDays":null,"autoSettleOnMerge":true}],"expected":null},{"name":"does not settle on pull requests that were already merged when linked","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":"2026-06-10T11:15:00.000Z","latestUserMessageAt":"2026-06-10T10:00:00.000Z","latestUserAuthoredMessageAt":"2026-06-10T10:00:00.000Z","createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null,"pullRequests":[{"host":"example.test","repository":"owner/repository","number":1,"url":"https://example.test/owner/repository/pull/1","source":"agent","linkedAt":"2026-06-10T11:10:00.000Z","snapshot":{"state":"merged","title":"Pull request","headBranch":"feature-1","baseBranch":"main","isDraft":false,"updatedAt":"2026-06-10T11:05:00.000Z","syncedAt":"2026-06-10T11:10:00.000Z","mergedAt":"2026-06-10T11:05:00.000Z"},"stack":null},{"host":"example.test","repository":"owner/repository","number":2,"url":"https://example.test/owner/repository/pull/2","source":"agent","linkedAt":"2026-06-10T10:30:00.000Z","snapshot":{"state":"merged","title":"Pull request","headBranch":"feature-2","baseBranch":"main","isDraft":false,"updatedAt":"2026-06-10T11:20:00.000Z","syncedAt":"2026-06-10T10:30:00.000Z","mergedAt":"2026-06-10T11:20:00.000Z"},"stack":null}]},"pullRequest":null,"nowMs":1781092800000,"autoSettleAfterDays":null,"autoSettleOnMerge":true}],"expected":"2026-06-10T11:15:00.000Z"},{"name":"defers existing input while new questions and approvals wake the thread","method":"isSnoozed","args":[{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":{"id":"question","kind":"user_input","createdAt":"2026-06-08T12:00:00.000Z"},"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":null,"latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":"2026-06-11T12:00:00.000Z","snoozedAt":"2026-06-08T12:00:00.000Z","pinnedAt":null},1781092800000],"expected":true},{"name":"defers existing input while new questions and approvals wake the thread","method":"isSnoozed","args":[{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":{"id":"question","kind":"user_input","createdAt":"2026-06-09T12:00:00.000Z"},"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":null,"latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":"2026-06-11T12:00:00.000Z","snoozedAt":"2026-06-08T12:00:00.000Z","pinnedAt":null},1781092800000],"expected":false},{"name":"defers existing input while new questions and approvals wake the thread","method":"isSnoozed","args":[{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":{"id":"question","kind":"command","createdAt":"2026-06-08T12:00:00.000Z"},"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":null,"latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":"2026-06-11T12:00:00.000Z","snoozedAt":"2026-06-08T12:00:00.000Z","pinnedAt":null},1781092800000],"expected":false},{"name":"defers existing input while new questions and approvals wake the thread","method":"isSnoozed","args":[{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"idle","activityRunStatus":null,"pendingRuntimeRequest":{"id":"question","kind":"user_input","createdAt":"2026-06-08T12:00:00.000Z"},"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":null,"latestRunStartedAt":null,"latestRunCompletedAt":null,"latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":"2026-06-11T12:00:00.000Z","snoozedAt":null,"pinnedAt":null},1781092800000],"expected":false},{"name":"keeps a run requested and completed after snoozing parked until the timer","method":"isSnoozed","args":[{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"completed","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":"2026-06-08T12:00:00.001Z","latestRunStartedAt":"2026-06-09T11:59:59.999Z","latestRunCompletedAt":"2026-06-09T12:00:00.000Z","latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":"2026-06-11T12:00:00.000Z","snoozedAt":"2026-06-08T12:00:00.000Z","pinnedAt":null},1781092800000],"expected":true},{"name":"keeps a run requested and completed after snoozing parked until the timer","method":"isAutoSettlementCandidate","args":[{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"completed","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":"2026-06-08T12:00:00.001Z","latestRunStartedAt":"2026-06-09T11:59:59.999Z","latestRunCompletedAt":"2026-06-09T12:00:00.000Z","latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":"2026-06-11T12:00:00.000Z","snoozedAt":"2026-06-08T12:00:00.000Z","pinnedAt":null},1781092800000],"expected":false},{"name":"keeps a run requested and completed after snoozing parked until the timer","method":"resolveAutoSettlementAt","args":[{"thread":{"id":"thread-1","projectId":"project-1","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-1","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","activeRunId":null,"latestVisibleMessage":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,"lastVisitedAt":null,"deletedAt":null,"branch":null,"linkedPullRequest":null,"status":"completed","activityRunStatus":null,"pendingRuntimeRequest":null,"pendingBackgroundTasks":[],"latestRunId":null,"latestRunRequestedAt":"2026-06-08T12:00:00.001Z","latestRunStartedAt":"2026-06-09T11:59:59.999Z","latestRunCompletedAt":"2026-06-09T12:00:00.000Z","latestUserMessageAt":null,"latestUserAuthoredMessageAt":null,"createdAt":"2026-05-11T12:00:00.000Z","updatedAt":"2026-05-31T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"snoozedUntil":"2026-06-11T12:00:00.000Z","snoozedAt":"2026-06-08T12:00:00.000Z","pinnedAt":null},"pullRequest":{"state":"merged","mergedAt":"2026-06-10T11:59:59.999Z"},"nowMs":1781092800000,"autoSettleAfterDays":null,"autoSettleOnMerge":true}],"expected":null}]}"###;
fn case(name: &str) -> Result<()> {
    let rows: Value = serde_json::from_str(ORACLE)?;
    for row in rows["oracle"]
        .as_array()
        .expect("oracle")
        .iter()
        .filter(|row| row["name"] == name)
    {
        let args = &row["args"];
        let shell = if row["method"] == "resolveAutoSettlementAt" {
            &args[0]["thread"]
        } else {
            &args[0]
        };
        let mut thread: Value = serde_json::from_str(
            include_str!("../../../../fixtures/samples/OrchestrationV2ThreadShell.jsonl")
                .lines()
                .next()
                .expect("shell"),
        )?;
        thread
            .as_object_mut()
            .expect("shell")
            .extend(shell.as_object().expect("shell object").clone());
        let actual = match row["method"].as_str().expect("method") {
            "isAutoSettlementCandidate" => json!(is_auto_settlement_candidate(
                &serde_json::from_value(thread)?,
                args[1].as_i64().expect("now")
            )),
            "isSnoozed" => json!(is_snoozed(
                &serde_json::from_value(thread)?,
                args[1].as_i64().expect("now")
            )),
            "threadHasQueuedTurnStart" => json!(thread_has_queued_turn_start(
                &serde_json::from_value(thread)?,
                args[1].as_i64().expect("now")
            )),
            "resolveAutoSettlementAt" => {
                let input = &args[0];
                json!(resolve_auto_settlement_at(
                    &serde_json::from_value(thread)?,
                    serde_json::from_value::<Option<SettlementPullRequest>>(
                        input["pullRequest"].clone()
                    )?
                    .as_ref(),
                    input["nowMs"].as_i64().expect("now"),
                    input["autoSettleAfterDays"].as_f64(),
                    input["autoSettleOnMerge"].as_bool().expect("merge"),
                    input["autoSettleKeepPullRequests"]
                        .as_bool()
                        .unwrap_or(false)
                ))
            }
            _ => panic!("unknown method"),
        };
        assert_eq!(actual, row["expected"], "{name}: {row}");
    }
    Ok(())
}

#[test]
fn settlement_case_1() -> Result<()> {
    case("settles a thread whose only background work is a command left running")
}

#[test]
fn settlement_case_2() -> Result<()> {
    case("never settles, or keeps snoozed, a thread whose guest is waiting on the owner")
}

#[test]
fn settlement_case_3() -> Result<()> {
    case("keeps snoozed threads parked until a fresh failure or timer wake")
}

#[test]
fn settlement_case_4() -> Result<()> {
    case("keeps threads with a PR from inactivity settlement only when asked")
}

#[test]
fn settlement_case_5() -> Result<()> {
    case("settles an idle thread with an open persisted link only while retention is off")
}

#[test]
fn settlement_case_6() -> Result<()> {
    case("keeps an idle thread whose persisted link was never synced")
}

#[test]
fn settlement_case_7() -> Result<()> {
    case("uses the latest activity time when the inactivity window elapses")
}

#[test]
fn settlement_case_8() -> Result<()> {
    case("anchors an empty thread's inactivity at its creation time")
}

#[test]
fn settlement_case_9() -> Result<()> {
    case("restarts inactivity on a manual snooze wake")
}

#[test]
fn settlement_case_10() -> Result<()> {
    case("restarts inactivity on a timer snooze wake")
}

#[test]
fn settlement_case_11() -> Result<()> {
    case("settles on merge only after the user's last action and preserves the activity time")
}

#[test]
fn settlement_case_12() -> Result<()> {
    case("settles on merge after agent-started runs, but not after the user writes again")
}

#[test]
fn settlement_case_13() -> Result<()> {
    case("does not settle on pull requests that were already merged when linked")
}

#[test]
fn settlement_case_14() -> Result<()> {
    case("defers existing input while new questions and approvals wake the thread")
}

#[test]
fn settlement_case_15() -> Result<()> {
    case("keeps a run requested and completed after snoozing parked until the timer")
}

#[test]
fn settings_key_distinguishes_inheritance_from_explicit_disable() -> Result<()> {
    let mut settings: Value = serde_json::from_str(
        include_str!("../../../../fixtures/samples/ServerSettings.jsonl")
            .lines()
            .next()
            .expect("settings"),
    )?;
    settings["projectSettingsOverrides"] = json!({"project": {"sidebarAutoSettleOnMerge": true}});
    let inherits = auto_settlement_settings_key(&serde_json::from_value(settings.clone())?)?;
    settings["projectSettingsOverrides"]["project"]["sidebarAutoSettleAfterDays"] = Value::Null;
    let disabled = auto_settlement_settings_key(&serde_json::from_value(settings.clone())?)?;
    assert_ne!(inherits, disabled);
    settings["projectSettingsOverrides"] =
        json!({"b":{"sidebarAutoSettleAfterDays":1},"a":{"sidebarAutoSettleAfterDays":2}});
    let left = auto_settlement_settings_key(&serde_json::from_value(settings.clone())?)?;
    settings["projectSettingsOverrides"] =
        json!({"a":{"sidebarAutoSettleAfterDays":2},"b":{"sidebarAutoSettleAfterDays":1}});
    assert_eq!(
        left,
        auto_settlement_settings_key(&serde_json::from_value(settings)?)?
    );
    Ok(())
}

use super::{
    orchestrator_receipt_race_test::Harness,
    thread_management_service_test::{emit, settings},
};
use std::sync::Arc;
use t3_orchestration::{
    api::*,
    testing::{SequenceIds, TEST_TIME},
};

mod lookups {
    use super::super::thread_pull_request_service_test::{Registry, Workspace, branch, project};
    use super::*;
    use async_trait::async_trait;
    use std::sync::{
        Mutex as StdMutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use t3_contracts::{pull_request::*, thread_pull_request::*};
    use t3_orchestration::error::Error;
    use tokio_util::sync::CancellationToken;

    #[derive(Default)]
    struct Backends {
        closes: AtomicUsize,
        scripts: AtomicUsize,
        completion: StdMutex<Option<tokio::sync::oneshot::Sender<Result<()>>>>,
        summary: StdMutex<Option<PullRequestSummary>>,
        reengage: StdMutex<Option<(Arc<dyn EventCommitter>, OrchestrationV2DomainEvent)>>,
        fail_script: AtomicBool,
        inputs: StdMutex<Vec<SettleScriptInput>>,
    }
    #[async_trait]
    impl ResourceCleanup for Backends {
        async fn cleanup(&self, _: ThreadId, _: ResourceKind) -> Result<()> {
            panic!("settlement must not delete terminals");
        }
        async fn close_on_settle(&self, _: ThreadId) -> Result<()> {
            self.closes.fetch_add(1, Ordering::SeqCst);
            let reengage = self.reengage.lock().unwrap().take();
            if let Some((sink, event)) = reengage {
                sink.commit(CommitRequest {
                    receipt: None,
                    decision: Decision {
                        events: vec![event],
                        ..Default::default()
                    },
                    guard: CommitGuard::None,
                })
                .await?;
            }
            Ok(())
        }
    }
    #[async_trait]
    impl SettleScriptRunner for Backends {
        async fn run(
            &self,
            input: SettleScriptInput,
            _: CancellationToken,
        ) -> Result<SettleScriptRun> {
            self.scripts.fetch_add(1, Ordering::SeqCst);
            self.inputs.lock().unwrap().push(input);
            if self.fail_script.load(Ordering::SeqCst) {
                return Err(Error::Unsupported("start failed"));
            }
            let (tx, rx) = tokio::sync::oneshot::channel();
            *self.completion.lock().unwrap() = Some(tx);
            Ok(SettleScriptRun::Started { completion: rx })
        }
    }
    #[async_trait]
    impl PullRequestBackend for Backends {
        async fn summary(&self, _: PullRequestRef) -> Result<PullRequestSummary> {
            self.summary
                .lock()
                .unwrap()
                .clone()
                .ok_or(Error::Unsupported("summary"))
        }
        async fn refresh(
            &self,
            _: std::path::PathBuf,
            _: Vec<ThreadPullRequestLink>,
        ) -> Result<Vec<ThreadPullRequestLink>> {
            panic!("strict summary/branch ports required")
        }
        async fn discover(
            &self,
            _: std::path::PathBuf,
            _: String,
        ) -> Result<Vec<ThreadPullRequestLink>> {
            panic!("fresh branch port required")
        }
    }
    fn service(
        harness: &Harness,
        workspace: Arc<Workspace>,
        backends: Arc<Backends>,
    ) -> ThreadSettlementService {
        ThreadSettlementService::with_backends(
            ThreadSettlementServices {
                projections: harness.reader.clone(),
                dispatcher: Arc::new(harness.dispatcher.clone()),
                committer: harness.sink.clone(),
                settings: settings(
                    json!({"sidebarAutoSettleAfterDays":null,"sidebarAutoSettleOnMerge":true}),
                ),
                clock: harness.clock.clone(),
                ids: Arc::new(SequenceIds::new("settle-lookup")),
            },
            ThreadSettlementBackends {
                workspaces: workspace,
                pull_requests: backends.clone(),
                identities: Arc::new(Registry),
                resources: backends.clone(),
                scripts: backends,
                closes_pull_request: Arc::new(|input| input == "host pr close"),
            },
        )
    }
    async fn prepare(harness: &Harness) -> Result<()> {
        project(harness, "C:/repo").await?;
        settlement_thread(harness, "thread").await?;
        let mut thread = json!(
            harness
                .reader
                .thread(ThreadId("thread".into()))
                .await?
                .unwrap()
        );
        thread["branch"] = json!("feature");
        thread["worktreePath"] = json!("C:/missing-settlement-worktree");
        emit(harness, "branch", "thread.metadata-updated", thread).await
    }
    #[tokio::test]
    async fn settlement_confirms_terminal_git_state_and_rejects_reopened_branch() -> Result<()> {
        for reopened in [true, false] {
            let harness = Harness::new(None, false);
            prepare(&harness).await?;
            let workspace = Arc::new(Workspace::new());
            let mut terminal = branch("merged");
            terminal.merged_at = Some(serde_json::from_value(json!(TEST_TIME))?);
            *workspace.detected.lock().unwrap() = Some(terminal.clone());
            *workspace.fresh.lock().unwrap() =
                Some(Some(if reopened { branch("open") } else { terminal }));
            let service = service(&harness, workspace.clone(), Arc::new(Backends::default()));
            service.sweep(Some(ThreadId("thread".into()))).await?;
            {
                let queries = workspace.queries.lock().unwrap();
                assert_eq!(queries.len(), 2);
                assert_eq!(queries[0].cwd, std::path::PathBuf::from("C:/repo"));
                assert!(!queries[0].confirm_terminal && !queries[0].fresh_git_state);
                assert!(queries[1].confirm_terminal && queries[1].fresh_git_state);
            }
            assert_eq!(
                harness
                    .reader
                    .thread(ThreadId("thread".into()))
                    .await?
                    .unwrap()
                    .settled_at
                    .is_some(),
                !reopened
            );
            service.shutdown().await;
        }
        Ok(())
    }

    #[tokio::test]
    async fn settlement_remembers_empty_branches_but_targeted_and_shell_close_bypass_the_delay()
    -> Result<()> {
        let harness = Harness::new(None, false);
        prepare(&harness).await?;
        let workspace = Arc::new(Workspace::new());
        *workspace.detected.lock().unwrap() = None;
        let service = service(&harness, workspace.clone(), Arc::new(Backends::default()));
        service.sweep(None).await?;
        service.sweep(None).await?;
        assert_eq!(workspace.queries.lock().unwrap().len(), 1);
        service.sweep(Some(ThreadId("thread".into()))).await?;
        assert_eq!(workspace.queries.lock().unwrap().len(), 2);
        let mut item = json!({"id":"close-item","threadId":"thread","runId":"run","nodeId":"node","providerThreadId":"provider-thread","providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"title":null,"updatedAt":TEST_TIME,"type":"command_execution","input":"host pr close","status":"completed","output":"","exitCode":0,"startedAt":TEST_TIME,"completedAt":TEST_TIME});
        item["type"] = json!("command_execution");
        item["input"] = json!("host pr close");
        item["status"] = json!("completed");
        let event = serde_json::from_value(
            json!({"id":"close","threadId":"thread","type":"turn-item.updated","occurredAt":TEST_TIME,"payload":item}),
        )?;
        service.process_event(&event).await?;
        service.drain().await;
        assert_eq!(workspace.queries.lock().unwrap().len(), 3);
        assert!(workspace.queries.lock().unwrap()[2].refresh);
        service.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn settlement_cleanup_deduplicates_scripts_and_shutdown_joins_completion() -> Result<()> {
        let harness = Harness::new(None, false);
        settlement_thread(&harness, "thread").await?;
        let path = std::env::temp_dir().join(format!("orch-d-settle-{}", std::process::id()));
        std::fs::create_dir_all(&path)?;
        let mut thread = json!(
            harness
                .reader
                .thread(ThreadId("thread".into()))
                .await?
                .unwrap()
        );
        thread["worktreePath"] = json!(path);
        thread["settledOverride"] = json!("settled");
        thread["settledAt"] = json!(TEST_TIME);
        emit(
            &harness,
            "settled-metadata",
            "thread.metadata-updated",
            thread,
        )
        .await?;
        let backends = Arc::new(Backends::default());
        let service = service(&harness, Arc::new(Workspace::new()), backends.clone());
        let event = serde_json::from_value(
            json!({"id":"settled","threadId":"thread","type":"thread.settled","occurredAt":TEST_TIME,"payload":json!(harness.reader.thread(ThreadId("thread".into())).await?.unwrap())}),
        )?;
        service.process_event(&event).await?;
        service.process_event(&event).await?;
        assert_eq!(backends.closes.load(Ordering::SeqCst), 2);
        assert_eq!(backends.scripts.load(Ordering::SeqCst), 1);
        assert_eq!(backends.inputs.lock().unwrap()[0].worktree_path, path);
        let mut next = json!(
            harness
                .reader
                .thread(ThreadId("thread".into()))
                .await?
                .unwrap()
        );
        next["settledAt"] = json!("2026-09-04T00:00:00.000Z");
        emit(&harness, "resettled", "thread.metadata-updated", next).await?;
        backends.fail_script.store(true, Ordering::SeqCst);
        service.process_event(&event).await?;
        assert_eq!(backends.scripts.load(Ordering::SeqCst), 2);
        backends.fail_script.store(false, Ordering::SeqCst);
        service.process_event(&event).await?;
        service.process_event(&event).await?;
        assert_eq!(backends.scripts.load(Ordering::SeqCst), 3);

        service.shutdown().await;
        assert!(
            backends
                .completion
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(Ok(()))
                .is_err()
        );
        assert!(matches!(
            service.process_event(&event).await,
            Err(Error::Cancelled)
        ));
        std::fs::remove_dir(path)?;
        Ok(())
    }
    #[tokio::test]
    async fn settlement_cleanup_rechecks_reengagement_and_skips_shared_parent_worktree()
    -> Result<()> {
        for mode in ["before", "during", "parent", "root", "missing"] {
            let harness = Harness::new(None, false);
            settlement_thread(&harness, "thread").await?;
            let path = std::env::temp_dir().join(format!(
                "orch-d-settle-guards-{}-{mode}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path)?;
            let mut thread = json!(
                harness
                    .reader
                    .thread(ThreadId("thread".into()))
                    .await?
                    .unwrap()
            );
            thread["worktreePath"] = if mode == "root" {
                Value::Null
            } else if mode == "missing" {
                json!(path.join("absent"))
            } else {
                json!(path)
            };
            thread["settledOverride"] = if mode == "before" {
                Value::Null
            } else {
                json!("settled")
            };
            thread["settledAt"] = json!(TEST_TIME);
            if mode == "parent" {
                harness.create("parent").await?;
                let mut parent = json!(
                    harness
                        .reader
                        .thread(ThreadId("parent".into()))
                        .await?
                        .unwrap()
                );
                parent["worktreePath"] = json!(path);
                harness.sink.commit(CommitRequest{receipt:None,decision:Decision{events:vec![serde_json::from_value(json!({"id":"parent-path","threadId":"parent","type":"thread.metadata-updated","occurredAt":TEST_TIME,"payload":parent}))?],..Default::default()},guard:CommitGuard::None}).await?;
                thread["lineage"]["relationshipToParent"] = json!("subagent");
                thread["lineage"]["parentThreadId"] = json!("parent");
            }
            emit(
                &harness,
                "cleanup-state",
                "thread.metadata-updated",
                thread.clone(),
            )
            .await?;
            let backends = Arc::new(Backends::default());
            if mode == "during" {
                thread["settledOverride"] = Value::Null;
                thread["settledAt"] = Value::Null;
                *backends.reengage.lock().unwrap() = Some((
                    harness.sink.clone(),
                    serde_json::from_value(
                        json!({"id":"reengaged","threadId":"thread","type":"thread.metadata-updated","occurredAt":TEST_TIME,"payload":thread}),
                    )?,
                ));
            }
            let service = service(&harness, Arc::new(Workspace::new()), backends.clone());
            let event = serde_json::from_value(
                json!({"id":"settled-event","threadId":"thread","type":"thread.settled","occurredAt":TEST_TIME,"payload":json!(harness.reader.thread(ThreadId("thread".into())).await?.unwrap())}),
            )?;
            service.process_event(&event).await?;
            assert_eq!(
                backends.closes.load(Ordering::SeqCst),
                usize::from(mode != "before"),
                "{mode}"
            );
            assert_eq!(backends.scripts.load(Ordering::SeqCst), 0, "{mode}");
            service.shutdown().await;
            std::fs::remove_dir(path)?;
        }
        Ok(())
    }
    #[tokio::test]
    async fn settlement_starts_the_ninth_group_while_a_slow_first_group_keeps_its_slot()
    -> Result<()> {
        struct Observed {
            inner: Arc<dyn CommandDispatcher>,
            settled: tokio::sync::mpsc::UnboundedSender<()>,
        }
        #[async_trait]
        impl CommandDispatcher for Observed {
            async fn dispatch(
                &self,
                command: OrchestrationV2ServerCommand,
            ) -> Result<t3_orchestration::stores::receipts::CommandReceipt> {
                let result = self.inner.dispatch(command).await?;
                self.settled.send(()).unwrap();
                Ok(result)
            }
        }
        let harness = Harness::new(None, false);
        project(&harness, "C:/repo").await?;
        for number in 0..9 {
            let id = format!("thread-{number}");
            settlement_thread(&harness, &id).await?;
            let mut thread = json!(harness.reader.thread(ThreadId(id.clone())).await?.unwrap());
            thread["branch"] = json!(format!("branch-{number}"));
            thread["linkedPullRequest"] = json!({"projectId":"project","repository":"owner/repo","number":42,"url":"https://github.com/owner/repo/pull/42"});
            harness.sink.commit(CommitRequest{receipt:None,decision:Decision{events:vec![serde_json::from_value(json!({"id":format!("branch-{number}"),"threadId":id,"type":"thread.metadata-updated","occurredAt":TEST_TIME,"payload":thread}))?],..Default::default()},guard:CommitGuard::None}).await?;
        }
        let (entered, mut slow) = tokio::sync::mpsc::unbounded_channel();
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let mut workspace = Workspace::new();
        workspace.gate = Some((entered, gate.clone()));
        workspace.gate_branch = Some("branch-0".into());
        let mut terminal = branch("merged");
        terminal.merged_at = Some(serde_json::from_value(json!(TEST_TIME))?);
        *workspace.detected.lock().unwrap() = Some(terminal);
        let workspace = Arc::new(workspace);
        let backends = Arc::new(Backends::default());
        let mut summary =
            super::super::thread_management_service_test::sample("PullRequestSummary");
        summary["state"] = json!("merged");
        summary["mergedAt"] = json!(TEST_TIME);
        *backends.summary.lock().unwrap() = Some(serde_json::from_value(summary)?);
        let (settled, mut settlements) = tokio::sync::mpsc::unbounded_channel();
        let service = Arc::new(ThreadSettlementService::with_backends(
            ThreadSettlementServices {
                projections: harness.reader.clone(),
                dispatcher: Arc::new(Observed {
                    inner: Arc::new(harness.dispatcher.clone()),
                    settled,
                }),
                committer: harness.sink.clone(),
                settings: settings(
                    json!({"sidebarAutoSettleAfterDays":null,"sidebarAutoSettleOnMerge":true}),
                ),
                clock: harness.clock.clone(),
                ids: Arc::new(SequenceIds::new("settle-slots")),
            },
            ThreadSettlementBackends {
                workspaces: workspace.clone(),
                pull_requests: backends.clone(),
                identities: Arc::new(Registry),
                resources: backends.clone(),
                scripts: backends,
                closes_pull_request: Arc::new(|_| false),
            },
        ));
        let sweep = {
            let service = service.clone();
            tokio::spawn(async move { service.sweep(None).await })
        };
        slow.recv().await.unwrap();
        for _ in 0..7 {
            settlements.recv().await.unwrap();
        }
        let ninth_started = workspace
            .queries
            .lock()
            .unwrap()
            .iter()
            .any(|query| query.branch == "branch-8");
        gate.add_permits(1);
        sweep.await.unwrap()?;
        service.shutdown().await;
        assert!(
            ninth_started,
            "completed groups must release slots for later groups while the first lookup stays blocked"
        );
        Ok(())
    }
}

async fn settlement_thread(harness: &Harness, id: &str) -> Result<()> {
    harness.create(id).await?;
    let mut thread = json!(harness.reader.thread(ThreadId(id.into())).await?.unwrap());
    thread["createdAt"] = json!("2026-08-01T00:00:00.000Z");
    thread["updatedAt"] = json!("2026-08-01T00:00:00.000Z");
    let event = serde_json::from_value(
        json!({"id":format!("old:{id}"),"threadId":id,"type":"thread.metadata-updated","occurredAt":TEST_TIME,"payload":thread}),
    )?;
    harness
        .sink
        .commit(CommitRequest {
            receipt: None,
            decision: Decision {
                events: vec![event],
                ..Default::default()
            },
            guard: CommitGuard::None,
        })
        .await?;
    Ok(())
}

fn settlement_service(
    harness: &Harness,
    settings: Arc<dyn SettingsService>,
    dispatcher: Arc<dyn CommandDispatcher>,
) -> Arc<ThreadSettlementService> {
    Arc::new(ThreadSettlementService::new(ThreadSettlementServices {
        projections: harness.reader.clone(),
        dispatcher,
        committer: harness.sink.clone(),
        settings,
        clock: harness.clock.clone(),
        ids: Arc::new(SequenceIds::new("settlement")),
    }))
}

#[tokio::test]
async fn settlement_shutdown_refuses_late_sweeps_and_joins_active_reads() -> Result<()> {
    use t3_orchestration::error::Error;

    struct BlockedSettings {
        entered: tokio::sync::mpsc::UnboundedSender<()>,
    }
    #[async_trait::async_trait]
    impl SettingsService for BlockedSettings {
        async fn get(&self) -> Result<t3_contracts::settings::ServerSettings> {
            self.entered.send(()).unwrap();
            std::future::pending().await
        }
    }

    let harness = Harness::new(None, false);
    let closed = settlement_service(
        &harness,
        settings(json!({})),
        Arc::new(harness.dispatcher.clone()),
    );
    closed.shutdown().await;
    assert!(matches!(closed.sweep(None).await, Err(Error::Cancelled)));
    assert!(matches!(closed.start().await, Err(Error::Cancelled)));
    assert!(matches!(closed.enqueue(None).await, Err(Error::Cancelled)));

    let (entered, mut reading) = tokio::sync::mpsc::unbounded_channel();
    let service = settlement_service(
        &harness,
        Arc::new(BlockedSettings { entered }),
        Arc::new(harness.dispatcher.clone()),
    );
    let sweep = tokio::spawn({
        let service = service.clone();
        async move { service.sweep(None).await }
    });
    reading.recv().await.unwrap();
    service.shutdown().await;
    assert!(matches!(sweep.await.unwrap(), Err(Error::Cancelled)));
    service.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn targeted_settlement_runs_while_a_full_sweep_is_blocked_and_shutdown_joins_both()
-> Result<()> {
    struct FirstReadBlocked {
        config: Arc<dyn SettingsService>,
        entered: tokio::sync::mpsc::UnboundedSender<()>,
        calls: std::sync::atomic::AtomicUsize,
    }
    #[async_trait::async_trait]
    impl SettingsService for FirstReadBlocked {
        async fn get(&self) -> Result<t3_contracts::settings::ServerSettings> {
            if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                self.entered.send(()).unwrap();
                std::future::pending::<()>().await;
            }
            self.config.get().await
        }
    }
    let harness = Harness::new(None, false);
    settlement_thread(&harness, "thread").await?;
    let (entered, mut blocked) = tokio::sync::mpsc::unbounded_channel();
    let service = settlement_service(
        &harness,
        Arc::new(FirstReadBlocked {
            config: settings(json!({"sidebarAutoSettleAfterDays":2})),
            entered,
            calls: std::sync::atomic::AtomicUsize::new(0),
        }),
        Arc::new(harness.dispatcher.clone()),
    );
    let mut committed = harness.sink.subscribe();
    service.enqueue(None).await?;
    blocked.recv().await.unwrap();
    service.enqueue(Some(ThreadId("thread".into()))).await?;
    loop {
        if let t3_contracts::application_event::ApplicationStoredEvent::Orchestration(stored) =
            committed.recv().await.unwrap()
            && matches!(stored.event, OrchestrationV2DomainEvent::ThreadSettled(_))
        {
            break;
        }
    }
    assert!(
        harness
            .reader
            .thread(ThreadId("thread".into()))
            .await?
            .unwrap()
            .settled_at
            .is_some()
    );
    service.shutdown().await;
    service.drain().await;
    Ok(())
}

#[tokio::test]
async fn settlement_reacts_to_settings_changes_without_waiting_for_the_periodic_sweep() -> Result<()>
{
    let harness = Harness::new(None, false);
    settlement_thread(&harness, "thread").await?;
    let config =
        settings(json!({"sidebarAutoSettleAfterDays":null,"sidebarAutoSettleOnMerge":false}));
    let service = settlement_service(
        &harness,
        config.clone(),
        Arc::new(harness.dispatcher.clone()),
    );
    service.start().await?;
    service.drain().await;
    assert!(
        harness
            .reader
            .thread(ThreadId("thread".into()))
            .await?
            .unwrap()
            .settled_at
            .is_none()
    );
    let mut committed = harness.sink.subscribe();
    config.0.send_replace(Arc::new(serde_json::from_value(
        json!({"sidebarAutoSettleAfterDays":2}),
    )?));
    loop {
        if let t3_contracts::application_event::ApplicationStoredEvent::Orchestration(stored) =
            committed.recv().await.unwrap()
            && matches!(stored.event, OrchestrationV2DomainEvent::ThreadSettled(_))
        {
            break;
        }
    }
    service.drain().await;
    assert!(
        harness
            .reader
            .thread(ThreadId("thread".into()))
            .await?
            .unwrap()
            .settled_at
            .is_some()
    );
    service.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn sweep_settles_idle_threads_and_persisted_merges_with_fresh_project_settings() -> Result<()>
{
    for (merge, configured) in [(false, true), (true, false)] {
        let harness = Harness::new(None, false);
        settlement_thread(&harness, "thread").await?;
        if merge {
            let mut thread = json!(
                harness
                    .reader
                    .thread(ThreadId("thread".into()))
                    .await?
                    .unwrap()
            );
            thread["pullRequests"] = json!([{"host":"example.test","repository":"owner/repo","number":42,"url":"https://example.test/owner/repo/pull/42","source":"manual","linkedAt":"2026-08-20T00:00:00.000Z","stack":null,"snapshot":{"state":"merged","title":"Review","headBranch":"feature","baseBranch":"main","isDraft":false,"updatedAt":TEST_TIME,"syncedAt":TEST_TIME,"mergedAt":TEST_TIME}}]);
            emit(&harness, "linked", "thread.metadata-updated", thread).await?;
        }
        let config = settings(
            json!({"sidebarAutoSettleAfterDays":null,"sidebarAutoSettleOnMerge":true,"projectSettingsOverrides":{"project":{"sidebarAutoSettleAfterDays": if configured { json!(2) } else { Value::Null }}}}),
        );
        let service = settlement_service(&harness, config, Arc::new(harness.dispatcher.clone()));
        service.enqueue(None).await?;
        service.drain().await;
        let thread = json!(
            harness
                .reader
                .thread(ThreadId("thread".into()))
                .await?
                .unwrap()
        );
        assert_eq!(thread["settledOverride"], "settled");
        assert_eq!(thread["settledAt"], "2026-08-01T00:00:00.000Z");
        service.shutdown().await;
    }
    Ok(())
}

#[tokio::test]
async fn sweep_rereads_settings_before_each_dispatch_and_skips_recent_branchless_threads()
-> Result<()> {
    let harness = Harness::new(None, false);
    settlement_thread(&harness, "thread").await?;
    harness.create("recent").await?;
    let config =
        settings(json!({"sidebarAutoSettleAfterDays":null,"sidebarAutoSettleOnMerge":false}));
    let service = settlement_service(
        &harness,
        config.clone(),
        Arc::new(harness.dispatcher.clone()),
    );
    service.sweep(None).await?;
    assert!(
        harness
            .reader
            .thread(ThreadId("thread".into()))
            .await?
            .unwrap()
            .settled_override
            .is_none()
    );
    config.0.send_replace(Arc::new(serde_json::from_value(
        json!({"sidebarAutoSettleAfterDays":2}),
    )?));
    service.sweep(None).await?;
    assert!(
        harness
            .reader
            .thread(ThreadId("thread".into()))
            .await?
            .unwrap()
            .settled_override
            .is_some()
    );
    assert!(
        harness
            .reader
            .thread(ThreadId("recent".into()))
            .await?
            .unwrap()
            .settled_override
            .is_none()
    );
    service.shutdown().await;
    Ok(())
}

#[test]
fn tied_terminal_links_keep_the_first_link_as_in_typescript() -> Result<()> {
    let rows: Value = serde_json::from_str(ORACLE)?;
    let input = &rows["oracle"]
        .as_array()
        .expect("oracle")
        .iter()
        .find(|row| row["method"] == "resolveAutoSettlementAt")
        .expect("settlement")["args"][0];
    let mut thread: Value = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationV2ThreadShell.jsonl")
            .lines()
            .next()
            .expect("shell"),
    )?;
    thread
        .as_object_mut()
        .expect("shell")
        .extend(input["thread"].as_object().expect("shell").clone());
    thread["linkedPullRequest"] = Value::Null;
    let link = |state: &str, number: u64| json!({"host":"example.test","repository":"o/r","number":number,"url":format!("https://example.test/o/r/pull/{number}"),"source":"manual","linkedAt":"2026-06-01T00:00:00.000Z","stack":null,"snapshot":{"state":state,"title":"Review","headBranch":"feature","baseBranch":"main","isDraft":false,"updatedAt":"2026-06-10T00:00:00.000Z","syncedAt":"2026-06-10T00:00:00.000Z","mergedAt":"2026-06-09T00:00:00.000Z","closedAt":"2026-06-09T00:00:00.000Z"}});
    thread["pullRequests"] = json!([link("merged", 1), link("closed", 2)]);
    let now = input["nowMs"].as_i64().expect("clock");
    assert!(
        resolve_auto_settlement_at(
            &serde_json::from_value(thread.clone())?,
            None,
            now,
            None,
            false,
            false
        )
        .is_none()
    );
    thread["pullRequests"] = json!([link("closed", 2), link("merged", 1)]);
    assert!(
        resolve_auto_settlement_at(
            &serde_json::from_value(thread)?,
            None,
            now,
            None,
            false,
            false
        )
        .is_some()
    );
    Ok(())
}
