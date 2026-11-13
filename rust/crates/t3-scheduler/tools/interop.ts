import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";
import { DatabaseSync } from "node:sqlite";
import Path from "node:path";

const require = createRequire(Path.join(process.cwd(), "package.json"));
const Schema = await import(pathToFileURL(require.resolve("effect/Schema")).href);
const { ScheduledTask, ScheduledTaskSchedule } = await import(
  pathToFileURL(Path.resolve("../../packages/contracts/src/scheduledTask.ts")).href
);

if (process.argv[2] === "--schedule") {
  const DateTime = await import(pathToFileURL(require.resolve("effect/DateTime")).href);
  const { nextScheduledRunAt } = await import(
    pathToFileURL(Path.resolve("src/scheduledTasks/Schedule.ts")).href
  );
  let text = "";
  for await (const chunk of process.stdin) text += chunk;
  const samples = JSON.parse(text);
  console.log(
    JSON.stringify(
      samples.map(({ schedule, from, zone }) => {
        const next = nextScheduledRunAt(schedule, DateTime.makeZonedUnsafe(from, { timeZone: zone }));
        return next === null ? null : DateTime.formatIso(next);
      }),
    ),
  );
  process.exit(0);
}

const db = new DatabaseSync(process.argv[2]);
const decode = Schema.decodeUnknownSync(ScheduledTask);
const encodeSchedule = Schema.encodeSync(ScheduledTaskSchedule);
const rows = db.prepare("SELECT * FROM scheduled_tasks ORDER BY task_id").all();
for (const row of rows) {
  const schedule = Schema.decodeUnknownSync(ScheduledTaskSchedule)(JSON.parse(row.schedule_json));
  const task = decode({
    id: row.task_id,
    title: row.title,
    prompt: row.prompt,
    enabled: row.enabled === 1,
    schedule,
    projectId: row.project_id,
    threadId: row.thread_id,
    threadMode: row.thread_mode,
    workspaceStrategy: JSON.parse(row.workspace_strategy_json),
    modelSelection: JSON.parse(row.model_selection_json),
    runtimeMode: row.runtime_mode,
    interactionMode: row.interaction_mode,
    createdBy: row.created_by,
    creationSource: row.creation_source,
    createdAt: row.created_at,
    updatedAt: row.updated_at,
    nextRunAt: row.next_run_at,
    lastRunAt: row.last_run_at,
    lastRunStatus: row.last_run_status,
    lastRunError: row.last_run_error,
    runCount: row.run_count,
  });
  const next = {
    ...row,
    task_id: `ts:${task.id}`,
    schedule_json: JSON.stringify(encodeSchedule(task.schedule)),
  };
  const columns = Object.keys(next);
  db.prepare(
    `INSERT INTO scheduled_tasks (${columns.join(",")}) VALUES (${columns.map(() => "?").join(",")})`,
  ).run(...Object.values(next));
}
console.log(JSON.stringify({ decoded: rows.length }));
db.close();
