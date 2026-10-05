/**
 * Release workflow CLI. Reads RELEASE_* inputs, plans exactly one operation,
 * writes artifacts, and maps the outcome to an exit code.
 *
 * Exit codes:
 * - 0 success or dry run
 * - 2 input validation refusal
 * - 3 operation failure
 */

import { appendFile } from "node:fs/promises"

import { parseReleaseInputs, type RawReleaseInputs } from "./inputs.ts"
import { executeRelease, writeArtifacts, type OperationResult } from "./operations.ts"

const OUTPUT_DIR_ENV = "RELEASE_OUTPUT_DIR"

function rawInputsFromEnv(env: NodeJS.ProcessEnv): RawReleaseInputs {
  return {
    operation: env.RELEASE_OPERATION,
    version: env.RELEASE_VERSION,
    release_line: env.RELEASE_LINE,
    commits: env.RELEASE_COMMITS,
    confirmation: env.RELEASE_CONFIRMATION,
    resume_proof: env.RELEASE_RESUME_PROOF,
    dry_run: env.RELEASE_DRY_RUN,
  }
}

async function printSummary(result: OperationResult): Promise<void> {
  const lines = [
    `## Release ${result.plan.operation}`,
    "",
    result.dryRun ? "Dry run: no mutations were applied." : "Live run: mutations were applied.",
    "",
    "### Preconditions",
    ...result.plan.preconditions.map((item) => `- ${item}`),
    "",
    "### Mutations",
    ...result.plan.mutations.map((item) => `- \`${item.kind}\`: ${item.summary}`),
  ]
  if (result.error !== undefined) {
    lines.push("", "### Error", "", result.error)
  }
  const body = `${lines.join("\n")}\n`
  console.log(body)
  if (process.env.GITHUB_STEP_SUMMARY) {
    await appendFile(process.env.GITHUB_STEP_SUMMARY, body)
  }
}

export async function runReleaseCli(env: NodeJS.ProcessEnv = process.env): Promise<number> {
  const parsed = parseReleaseInputs(rawInputsFromEnv(env))
  if (!parsed.ok) {
    console.error(JSON.stringify({ ok: false, errors: parsed.errors }, null, 2))
    return 2
  }

  const outputDir = env[OUTPUT_DIR_ENV]
  if (outputDir === undefined || outputDir.trim() === "") {
    console.error(`${OUTPUT_DIR_ENV} is required`)
    return 2
  }

  let result: OperationResult
  try {
    result = await executeRelease(parsed.request)
  } catch (error) {
    result = {
      ok: false,
      dryRun: parsed.request.dryRun,
      plan: {
        operation: parsed.request.operation,
        dryRun: parsed.request.dryRun,
        preconditions: [],
        mutations: [],
      },
      error: error instanceof Error ? error.message : String(error),
    }
  }

  await writeArtifacts(outputDir, result)
  await printSummary(result)
  return result.ok ? 0 : 3
}

if (import.meta.main) {
  process.exit(await runReleaseCli())
}
