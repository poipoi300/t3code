#[test]
fn ts_case_001() { check_test(0); } // resolveAssistantMessageCopyState / copies the rendered representation of Codex directives
#[test]
fn ts_case_002() { check_test(1); } // deriveMessagesTimelineRows / suppresses the empty fallback for %s in %s: %s ["html-render","reply-run",true]
#[test]
fn ts_case_003() { check_test(2); } // deriveMessagesTimelineRows / suppresses the empty fallback for %s in %s: %s ["mcp-app","reply-run",true]
#[test]
fn ts_case_004() { check_test(3); } // deriveMessagesTimelineRows / suppresses the empty fallback for %s in %s: %s ["html-render","other-run",false]
#[test]
fn ts_case_005() { check_test(4); } // deriveMessagesTimelineRows / suppresses the empty fallback for %s in %s: %s ["html-render",null,false]
#[test]
fn ts_case_006() { check_test(5); } // deriveMessagesTimelineRows / suppresses the empty fallback for %s in %s: %s [null,null,false]
#[test]
fn ts_case_007() { check_test(6); } // deriveMessagesTimelineRows / keeps an unfinished task update static %s "alone"
#[test]
fn ts_case_008() { check_test(7); } // deriveMessagesTimelineRows / keeps an unfinished task update static %s "before tool"
#[test]
fn ts_case_009() { check_test(8); } // deriveMessagesTimelineRows / keeps an unfinished task update static %s "before commentary and tool"
#[test]
fn ts_case_010() { check_test(9); } // deriveMessagesTimelineRows / stops stranded thinking after a steer and follows the next thought or tool
#[test]
fn ts_case_011() { check_test(10); } // deriveMessagesTimelineRows / recovers the failed Sky call stored as a generic js integration
#[test]
fn ts_case_012() { check_test(11); } // deriveMessagesTimelineRows / groups approval and user-input requests with commands without expanding them when %s "waiting"
#[test]
fn ts_case_013() { check_test(12); } // deriveMessagesTimelineRows / groups approval and user-input requests with commands without expanding them when %s "completed"
#[test]
fn ts_case_014() { check_test(13); } // deriveMessagesTimelineRows / keeps system notices visible as non-failing warnings through %s work "running"
#[test]
fn ts_case_015() { check_test(14); } // deriveMessagesTimelineRows / keeps system notices visible as non-failing warnings through %s work "completed"
#[test]
fn ts_case_016() { check_test(15); } // deriveMessagesTimelineRows / keeps system notices visible as non-failing warnings through %s work "superseded"
#[test]
fn ts_case_017() { check_test(16); } // deriveMessagesTimelineRows / gives live compaction the activity slot and restores Thinking (runless: %s) false
#[test]
fn ts_case_018() { check_test(17); } // deriveMessagesTimelineRows / gives live compaction the activity slot and restores Thinking (runless: %s) true
#[test]
fn ts_case_019() { check_test(18); } // deriveMessagesTimelineRows / shows each compaction trigger as one live status, then a reasoned token divider
#[test]
fn ts_case_020() { check_test(19); } // deriveMessagesTimelineRows / keeps a completed compaction divider visible while other settled work folds for %s "/compact"
#[test]
fn ts_case_021() { check_test(20); } // deriveMessagesTimelineRows / keeps a completed compaction divider visible while other settled work folds for %s "Continue"
#[test]
fn ts_case_022() { check_test(21); } // deriveMessagesTimelineRows / keeps only the latest compaction divider visible when a turn that compacted twice folds
#[test]
fn ts_case_023() { check_test(22); } // deriveMessagesTimelineRows / folds an answered compact-on-return question into its compaction row
#[test]
fn ts_case_024() { check_test(23); } // deriveMessagesTimelineRows / shows a skipped compaction as stopped, without a separate interrupted divider
#[test]
fn ts_case_025() { check_test(24); } // deriveMessagesTimelineRows / only enables assistant copy for the terminal assistant message in a turn
#[test]
fn ts_case_026() { check_test(25); } // deriveMessagesTimelineRows / projects assistant diffs and rewinds the first visible run to thread start after a rewind
#[test]
fn ts_case_027() { check_test(26); } // deriveMessagesTimelineRows / folds the first assistant message and settled work before the terminal response
#[test]
fn ts_case_028() { check_test(27); } // deriveMessagesTimelineRows / folds %i completed activities after the terminal response 1
#[test]
fn ts_case_029() { check_test(28); } // deriveMessagesTimelineRows / folds %i completed activities after the terminal response 2
#[test]
fn ts_case_030() { check_test(29); } // deriveMessagesTimelineRows / folds %i completed activities after the terminal response 3
#[test]
fn ts_case_031() { check_test(30); } // deriveMessagesTimelineRows / derives a sane duration for a steer-superseded turn with one instant commentary message
#[test]
fn ts_case_032() { check_test(31); } // deriveMessagesTimelineRows / uses latest-turn timings and the stopped label for an interrupted latest turn
#[test]
fn ts_case_033() { check_test(32); } // deriveMessagesTimelineRows / keeps the duration header at the initiating prompt when %s arrives before output "steer"
#[test]
fn ts_case_034() { check_test(33); } // deriveMessagesTimelineRows / keeps the duration header at the initiating prompt when %s arrives before output "promoted_queued_to_steer"
#[test]
fn ts_case_035() { check_test(34); } // deriveMessagesTimelineRows / splits a settled turn's work at each steer and keeps the total time on top
#[test]
fn ts_case_036() { check_test(35); } // deriveMessagesTimelineRows / stacks back-to-back steers as one burst with a single steer label
#[test]
fn ts_case_037() { check_test(36); } // deriveMessagesTimelineRows / keeps the previous turn folded while a newly sent message awaits its turn
#[test]
fn ts_case_038() { check_test(37); } // deriveMessagesTimelineRows / does not show Thinking once the open run's provider turn has ended
#[test]
fn ts_case_039() { check_test(38); } // deriveMessagesTimelineRows / keeps a promptless restart in one active visual response
#[test]
fn ts_case_040() { check_test(39); } // deriveMessagesTimelineRows / carries the latest thought on the live row while a later tool runs
#[test]
fn ts_case_041() { check_test(40); } // deriveMessagesTimelineRows / keeps an actually running tool in the shared activity row
#[test]
fn ts_case_042() { check_test(41); } // deriveMessagesTimelineRows / folds each run of a provider-native subagent thread like a normal turn
#[test]
fn ts_case_043() { check_test(42); } // deriveMessagesTimelineRows / keeps answered questions in their own row outside tool groups and turn folds
#[test]
fn ts_case_044() { check_test(43); } // deriveMessagesTimelineRows / shows images the agent viewed outside their collapsed tool group
#[test]
fn ts_case_045() { check_test(44); } // deriveMessagesTimelineRows / keeps imported V1 turns folded once the thread's first V2 run starts
#[test]
fn ts_case_046() { check_test(45); } // deriveMessagesTimelineRows / does not treat runless entries as live work on a thread with runs
#[test]
fn ts_case_047() { check_test(46); } // deriveMessagesTimelineRows / keeps separated in-progress tool runs visible
#[test]
fn ts_case_048() { check_test(47); } // deriveMessagesTimelineRows / does not revive stale in-progress tools before a fresh send has a turn id
#[test]
fn ts_case_049() { check_test(48); } // deriveMessagesTimelineRows / respects the %s lifecycle of trailing task progress ["inProgress",true]
#[test]
fn ts_case_050() { check_test(49); } // deriveMessagesTimelineRows / respects the %s lifecycle of trailing task progress ["completed",false]
#[test]
fn ts_case_051() { check_test(50); } // deriveMessagesTimelineRows / respects the %s lifecycle of trailing task progress ["failed",null]
#[test]
fn ts_case_052() { check_test(51); } // deriveMessagesTimelineRows / reuses one activity row for initial thinking and the latest tool
#[test]
fn ts_case_053() { check_test(52); } // deriveMessagesTimelineRows / does not fold the session's running turn when latestRun regresses
#[test]
fn ts_case_054() { check_test(53); } // deriveMessagesTimelineRows / uses the final tool call for mixed work groups when %s ["the later success is hidden",["failed","completed","info"],false]
#[test]
fn ts_case_055() { check_test(54); } // deriveMessagesTimelineRows / uses the final tool call for mixed work groups when %s ["the later success is visible",["failed","info","completed"],false]
#[test]
fn ts_case_056() { check_test(55); } // deriveMessagesTimelineRows / uses the final tool call for mixed work groups when %s ["an error-toned entry recovers",["error","info","completed"],false]
#[test]
fn ts_case_057() { check_test(56); } // deriveMessagesTimelineRows / uses the final tool call for mixed work groups when %s ["the final failure is hidden",["completed","failed","info"],true]
#[test]
fn ts_case_058() { check_test(57); } // deriveMessagesTimelineRows / uses the final tool call for mixed work groups when %s ["the final failure is visible",["failed","info","failed"],true]
#[test]
fn ts_case_059() { check_test(58); } // deriveMessagesTimelineRows / uses the final tool call for mixed work groups when %s ["the only failure is visible",["completed","info","failed"],true]
#[test]
fn ts_case_060() { check_test(59); } // v2 run and attempt history / folds settled-turn commentary and work behind a Worked-for row
#[test]
fn ts_case_061() { check_test(60); } // v2 run and attempt history / hides tool calls or everything but the final answer
#[test]
fn ts_case_062() { check_test(61); } // v2 run and attempt history / collapses only output from a superseded V2 attempt within the active logical run
#[test]
fn ts_case_063() { check_test(62); } // v2 run and attempt history / a steer that supersedes an attempt / retains phase %s ending at %s only when it is a final reply: %s ["final_answer",4,true]
#[test]
fn ts_case_064() { check_test(63); } // v2 run and attempt history / a steer that supersedes an attempt / retains phase %s ending at %s only when it is a final reply: %s ["final_answer",11,true]
#[test]
fn ts_case_065() { check_test(64); } // v2 run and attempt history / a steer that supersedes an attempt / retains phase %s ending at %s only when it is a final reply: %s ["commentary",4,false]
#[test]
fn ts_case_066() { check_test(65); } // v2 run and attempt history / a steer that supersedes an attempt / retains phase %s ending at %s only when it is a final reply: %s ["commentary",11,false]
#[test]
fn ts_case_067() { check_test(66); } // v2 run and attempt history / a steer that supersedes an attempt / retains phase %s ending at %s only when it is a final reply: %s ["partial_answer",11,false]
#[test]
fn ts_case_068() { check_test(67); } // v2 run and attempt history / a steer that supersedes an attempt / retains phase %s ending at %s only when it is a final reply: %s [null,4,true]
#[test]
fn ts_case_069() { check_test(68); } // v2 run and attempt history / a steer that supersedes an attempt / retains phase %s ending at %s only when it is a final reply: %s [null,11,false]
#[test]
fn ts_case_070() { check_test(69); } // v2 run and attempt history / hides the interruption request while keeping intervening work and the result
#[test]
fn ts_case_071() { check_test(70); } // keeps deferred fold anchors current from %j to %j [null,["first"]]
#[test]
fn ts_case_072() { check_test(71); } // keeps deferred fold anchors current from %j to %j [["first"],["second"]]
#[test]
fn ts_case_073() { check_test(72); } // keeps deferred fold anchors current from %j to %j [["first"],[]]
#[test]
fn ts_case_074() { check_test(73); } // keeps deferred fold anchors current from %j to %j [["first","second"],["second","first"]]
#[test]
fn ts_case_075() { check_test(74); } // keeps deferred fold anchors current from %j to %j [null,[]]
#[test]
fn ts_case_076() { check_test(75); } // streaming v2 row projection / keeps row parity and history identity from %j to %j ["","Now visible"]
#[test]
fn ts_case_077() { check_test(76); } // streaming v2 row projection / keeps row parity and history identity from %j to %j [" \n","Now visible"]
#[test]
fn ts_case_078() { check_test(77); } // streaming v2 row projection / keeps row parity and history identity from %j to %j ["Visible",""]
#[test]
fn ts_case_079() { check_test(78); } // streaming v2 row projection / keeps row parity and history identity from %j to %j [""," \t\n"]
#[test]
fn ts_case_080() { check_test(79); } // streaming v2 row projection / keeps row parity and history identity from %j to %j ["Visible","★ Insight\nKeep these line breaks"]
#[test]
fn ts_case_081() { check_test(80); } // streaming v2 row projection / keeps history references with %s previews through the v2 materializer "file"
#[test]
fn ts_case_082() { check_test(81); } // streaming v2 row projection / keeps history references with %s previews through the v2 materializer "image"
#[test]
fn ts_case_083() { check_test(82); } // streaming v2 row projection / keeps history references with %s previews through the v2 materializer "handoff"
#[test]
fn ts_case_084() { check_test(83); } // streaming v2 row projection / keeps history references with %s previews through the v2 materializer "streaming-image"
#[test]
fn ts_case_085() { check_test(84); } // streaming v2 row projection / rebuilds for completion, late failure, metadata, expansion, and older history
#[test]
fn ts_case_086() { check_test(85); } // streaming v2 row projection / keeps every row not holding streaming %s text when it changes "reasoning"
#[test]
fn ts_case_087() { check_test(86); } // streaming v2 row projection / keeps every row not holding streaming %s text when it changes "proposed_plan"
#[test]
fn ts_case_088() { check_test(87); } // streaming v2 row projection / matches a full derivation when streaming %s text turns blank or completes "reasoning"
#[test]
fn ts_case_089() { check_test(88); } // streaming v2 row projection / matches a full derivation when streaming %s text turns blank or completes "proposed_plan"
#[test]
fn ts_case_090() { check_test(89); } // streaming v2 row projection / matches a full derivation when streaming thinking sits in a %s "expanded live group"
#[test]
fn ts_case_091() { check_test(90); } // streaming v2 row projection / matches a full derivation when streaming thinking sits in a %s "expanded live group followed by a hidden tool"
#[test]
fn ts_case_092() { check_test(91); } // streaming v2 row projection / matches a full derivation when streaming thinking sits in a %s "thinking superseded by a running tool"
#[test]
fn ts_case_093() { check_test(92); } // streaming v2 row projection / matches a full derivation when streaming thinking sits in a %s "settled turn"
#[test]
fn ts_case_094() { check_test(93); } // linked timeline resources / groups adjacent subagents without merging across a resource or run boundary
#[test]
fn ts_case_095() { check_test(94); } // linked timeline resources / matches $status $role delegation calls by child identity with $envelope output {"status":"completed","envelope":"direct","role":"general"}
#[test]
fn ts_case_096() { check_test(95); } // linked timeline resources / matches $status $role delegation calls by child identity with $envelope output {"status":"completed","envelope":"structured","role":"general"}
#[test]
fn ts_case_097() { check_test(96); } // linked timeline resources / matches $status $role delegation calls by child identity with $envelope output {"status":"completed","envelope":"text","role":"general"}
#[test]
fn ts_case_098() { check_test(97); } // linked timeline resources / matches $status $role delegation calls by child identity with $envelope output {"status":"completed","envelope":"structured","role":"research"}
#[test]
fn ts_case_099() { check_test(98); } // linked timeline resources / matches $status $role delegation calls by child identity with $envelope output {"status":"running","envelope":"direct","role":"general"}
#[test]
fn ts_case_100() { check_test(99); } // linked timeline resources / matches $status $role delegation calls by child identity with $envelope output {"status":"running","envelope":"direct","role":"research"}
#[test]
fn ts_case_101() { check_test(100); } // linked timeline resources / keeps created-chat summaries after the final answer and folds only their timeline rows
#[test]
fn ts_case_102() { check_test(101); } // linked timeline resources / shows a created chat after a steer only when that steer's segment is open
#[test]
fn ts_case_103() { check_test(102); } // keeps completed responses folded across notification wakes, working=%s true
#[test]
fn ts_case_104() { check_test(103); } // keeps completed responses folded across notification wakes, working=%s false
#[test]
fn ts_case_105() { check_test(104); } // keeps the working header in place across worktree setup handoff
#[test]
fn ts_case_106() { check_test(105); } // failed turn transcript / keeps $position $failureClass failures visible with preceding work grouped appropriately {"position":"historical","failureClass":"provider_error"}
#[test]
fn ts_case_107() { check_test(106); } // failed turn transcript / keeps $position $failureClass failures visible with preceding work grouped appropriately {"position":"historical","failureClass":"usage_limit"}
#[test]
fn ts_case_108() { check_test(107); } // failed turn transcript / keeps $position $failureClass failures visible with preceding work grouped appropriately {"position":"latest","failureClass":"usage_limit"}
#[test]
fn ts_case_109() { check_test(108); } // live subagents after their parent turn settles / keeps a running %s child visible while the rest of the settled turn folds "app_owned"
#[test]
fn ts_case_110() { check_test(109); } // live subagents after their parent turn settles / keeps a running %s child visible while the rest of the settled turn folds "provider_native"
#[test]
fn ts_case_111() { check_test(110); } // live subagents after their parent turn settles / keeps a launch batch visible while any member is still running
#[test]
fn ts_case_112() { check_test(111); } // live subagents after their parent turn settles / keeps a running child visible when a steer supersedes its attempt
#[test]
fn ts_case_113() { check_test(112); } // parent run waiting on foreground subagents / names the running children while the open run has done nothing since launching them
#[test]
fn ts_case_114() { check_test(113); } // parent run waiting on foreground subagents / keeps Thinking when a hidden newer parent command follows the children
#[test]
fn ts_case_115() { check_test(114); } // subagent continued by a later turn / keeps each turn's fold between its own prompt and answer
#[test]
fn ts_case_116() { check_test(115); } // long active tool groups / preserves chronological order and the latest activity, expanded=%s false
#[test]
fn ts_case_117() { check_test(116); } // long active tool groups / preserves chronological order and the latest activity, expanded=%s true
#[test]
fn ts_case_118() { check_test(117); } // copy adapter differential probe 1
#[test]
fn ts_case_119() { check_test(118); } // copy adapter differential probe 2
#[test]
fn ts_case_120() { check_test(119); } // copy adapter differential probe 3
#[test]
fn ts_case_121() { check_test(120); } // copy adapter differential probe 4
#[test]
fn ts_case_122() { check_test(121); } // copy adapter differential probe 5
#[test]
fn ts_case_123() { check_test(122); } // copy adapter differential probe 6
#[test]
fn ts_case_124() { check_test(123); } // copy adapter differential probe 7
#[test]
fn ts_case_125() { check_test(124); } // copy adapter differential probe 8
#[test]
fn ts_case_126() { check_test(125); } // copy adapter differential probe 9
#[test]
fn ts_case_127() { check_test(126); } // copy adapter differential probe 10
#[test]
fn ts_case_128() { check_test(127); } // copy adapter differential probe 11
#[test]
fn ts_case_129() { check_test(128); } // copy adapter differential probe 12
#[test]
fn ts_case_130() { check_test(129); } // copy adapter differential probe 13
#[test]
fn ts_case_131() { check_test(130); } // copy adapter differential probe 14
#[test]
fn ts_case_132() { check_test(131); } // copy adapter differential probe 15
#[test]
fn ts_case_133() { check_test(132); } // copy adapter differential probe 16
#[test]
fn ts_case_134() { check_test(133); } // copy adapter differential probe 17
#[test]
fn ts_case_135() { check_test(134); } // copy adapter differential probe 18
#[test]
fn ts_case_136() { check_test(135); } // copy adapter differential probe 19
#[test]
fn ts_case_137() { check_test(136); } // copy adapter differential probe 20
#[test]
fn ts_case_138() { check_test(137); } // upstream theme tool labels and priority inProgress
#[test]
fn ts_case_139() { check_test(138); } // upstream theme tool labels and priority completed
#[test]
fn ts_case_140() { check_test(139); } // upstream theme tool labels and priority failed
#[test]
fn ts_case_141() { check_test(140); } // upstream theme tool labels and priority stopped
#[test]
fn ts_case_142() { check_test(141); } // tool metadata classifier differential probe 1
#[test]
fn ts_case_143() { check_test(142); } // tool metadata classifier differential probe 2
#[test]
fn ts_case_144() { check_test(143); } // tool metadata classifier differential probe 3
#[test]
fn ts_case_145() { check_test(144); } // tool metadata classifier differential probe 4
#[test]
fn ts_case_146() { check_test(145); } // tool metadata classifier differential probe 5
#[test]
fn ts_case_147() { check_test(146); } // upstream theme-set keeps actionable summary priority
