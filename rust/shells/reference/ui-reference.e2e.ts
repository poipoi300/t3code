// Reference captures of the real web app for the Rust UI's "done" check: the same seeded data as
// t3-ui's DemoBackend (`rust/crates/t3-ui/src/runtime/demo/fixtures.rs`, which a test keeps in
// step with this file), every route, at 1280x800 and 390x844. Runs in the repo's scenario
// harness; `rust/shells/REFERENCE.md` gives the one command.
import { test, WEB } from "./t3.ts";

const SHOWCASE_PROMPT = "Give me a short tour.";
const FOLLOW_UP = "Show me the selector that keeps streaming cheap.";
const PROJECTS = ["t3code", "nsh-spikes"] as const;
// [project, title], in the fixture's order: the first is the newest, so it is seeded last.
const THREADS: ReadonlyArray<readonly [(typeof PROJECTS)[number], string]> = [
  ["t3code", "Build the shared Rust UI slice"],
  ["t3code", "Timeline virtualisation with long code blocks"],
  ["t3code", "Rich composer: chips, IME and paste"],
  ["t3code", "Pairing flow on phone widths"],
  ["t3code", "Terminal emulator fidelity"],
  ["t3code", "Port theme tokens from index.css"],
  ["nsh-spikes", "UIA Text pattern for Windows"],
  ["nsh-spikes", "Scrollbar keeps the GPUI frame loop alive"],
  ["nsh-spikes", "Select all across recycled rows"],
];
const SETTINGS = [
  "general", "appearance", "chat", "threads", "diffs", "keybindings", "voice", "providers",
  "connections", "devices", "access", "secrets", "projects", "worktrees", "source-control",
  "scheduled-tasks", "browser", "integrations", "storage", "maintenance", "diagnostics",
  "archived", "beta", "labs", "snap-shot", "open-source-licenses", "about",
];
const PAGES = [
  "/", "/new", "/pull-requests", "/usage", "/settings",
  ...SETTINGS.map((page) => `/settings/${page}`),
  "/welcome", "/connect", "/guest", "/connect-agent", "/approve-agent",
];
const SIZES = [
  { name: "desktop", width: 1280, height: 800 },
  { name: "phone", width: 390, height: 844 },
] as const;

test(
  "Reference captures for the Rust UI: every route at desktop and phone sizes",
  WEB,
  async ({ t3, app, browser }) => {
    const roots: Record<string, string> = {};
    for (const title of PROJECTS) roots[title] = await t3.project(title);
    const threads: Array<{ readonly slug: string; readonly thread: { threadId: string } }> = [];
    for (const [index, [project, title]] of [...THREADS.entries()].reverse()) {
      const thread =
        index === 0
          ? await t3.thread({ project: roots[project]!, title, messages: [SHOWCASE_PROMPT], model: "showcase" })
          : await t3.thread({
              project: roots[project]!,
              title,
              messages: [`Let's work on ${title}.`, FOLLOW_UP],
              model: "multi-turn",
            });
      if (index < 2) threads.unshift({ slug: index === 0 ? "thread-showcase" : "thread-multi-turn", thread });
    }
    await t3.open();
    // Fonts and the first frame after a navigation, before each capture.
    const settled = () =>
      browser.evaluate(
        () =>
          document.fonts.ready.then(
            () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
          ),
      );
    for (const size of SIZES) {
      await browser.setViewport({ width: size.width, height: size.height });
      for (const { slug, thread } of threads) {
        await t3.open(thread);
        await settled();
        await app.screenshot(`${size.name}-${slug}`);
      }
      for (const page of PAGES) {
        await app.open(page);
        await settled();
        await app.screenshot(`${size.name}-${page === "/" ? "home" : page.slice(1).replaceAll("/", "-")}`);
      }
    }
  },
);
