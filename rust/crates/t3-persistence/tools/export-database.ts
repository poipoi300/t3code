// Run with Node from the repository root. The shared exporter uses a scoped OS temp database.
import * as Assert from "node:assert/strict";
import * as FS from "node:fs/promises";
import { exportDatabase } from "../../../tools/export/database.ts";

const database = await exportDatabase();
const files = new Map([
  ["schema.sql", database.sql],
  ["migrations.json", JSON.stringify(database.migrations, null, 2) + "\n"],
  ["pragmas.json", JSON.stringify(database.pragmas, null, 2) + "\n"],
]);
for (const [name, content] of files) {
  const destination = new URL(`../../../fixtures/database/${name}`, import.meta.url);
  if (process.argv.includes("--check")) {
    Assert.equal(await FS.readFile(destination, "utf8"), content, name);
  } else {
    await FS.writeFile(destination, content);
  }
}
console.log(`Database fixtures: head ${database.migrations.at(-1)?.id}, ${database.tables} tables`);
