import { describe, expect, test } from "bun:test"
import { parseReleaseInputs } from "../../.github/scripts/release/src/inputs.ts"
import { planRelease, type CommandRunner } from "../../.github/scripts/release/src/operations.ts"

function runner(responses: Record<string, { stdout?: string; code?: number }>): CommandRunner {
  return async (command) => {
    const key = command.join(" ")
    const match = Object.entries(responses).find(([pattern]) => key === pattern || key.startsWith(pattern))
    const value = match?.[1] ?? { stdout: "", code: 1 }
    return { stdout: value.stdout ?? "", stderr: "", code: value.code ?? 0 }
  }
}

describe("planRelease", () => {
  test("cut creates the release line from origin/main", async () => {
    const parsed = parseReleaseInputs({ operation: "cut", version: "1.0.0", dry_run: "true" })
    if (!parsed.ok) throw new Error("fixture")
    const plan = await planRelease(parsed.request, runner({
      "git fetch origin --tags --force --prune": { code: 0 },
      "git rev-parse --verify origin/release/v1.0": { code: 1 },
      "git rev-parse --verify origin/main": { stdout: "a".repeat(40), code: 0 },
    }))
    expect(plan.mutations).toEqual([
      {
        kind: "create-branch",
        summary: "create release/v1.0 at origin/main",
        name: "release/v1.0",
        sha: "a".repeat(40),
      },
    ])
  })

  test("restore recreates the line at the highest stable tag", async () => {
    const parsed = parseReleaseInputs({ operation: "restore", release_line: "release/v1.0", dry_run: "true" })
    if (!parsed.ok) throw new Error("fixture")
    const plan = await planRelease(parsed.request, runner({
      "git fetch origin --tags --force --prune": { code: 0 },
      "git rev-parse --verify origin/release/v1.0": { code: 1 },
      "git tag --list": { stdout: "1.0.0-rc.1\n1.0.0\n1.0.1\n1.1.0", code: 0 },
      "git rev-parse --verify refs/tags/1.0.1^{commit}": { stdout: "b".repeat(40), code: 0 },
    }))
    expect(plan.mutations[0]).toMatchObject({
      kind: "create-branch",
      name: "release/v1.0",
      sha: "b".repeat(40),
    })
  })
})
