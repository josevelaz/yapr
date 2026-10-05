import { mkdir, readFile, writeFile } from "node:fs/promises"
import { join } from "node:path"

import type { ReleaseRequest } from "./inputs.ts"
import {
  compareVersions,
  formatReleaseLine,
  formatVersion,
  majorAlias,
  parseVersion,
  tagName,
  type ReleaseLine,
  type ReleaseVersion,
} from "./semver.ts"

export type Mutation = {
  readonly kind: string
  readonly summary: string
  readonly [key: string]: string
}

export type OperationPlan = {
  readonly operation: ReleaseRequest["operation"]
  readonly dryRun: boolean
  readonly version?: string
  readonly releaseLine?: string
  readonly preconditions: readonly string[]
  readonly mutations: readonly Mutation[]
  readonly notes?: string
}

export type OperationResult = {
  readonly ok: boolean
  readonly dryRun: boolean
  readonly plan: OperationPlan
  readonly error?: string
}

type CommandResult = {
  readonly stdout: string
  readonly stderr: string
  readonly code: number
}

export type CommandRunner = (
  command: readonly string[],
  options?: { readonly allowFail?: boolean },
) => Promise<CommandResult>

const APP_NAME = "Yapr"
const CARGO_TOML = "app/Cargo.toml"
const CARGO_LOCK = "app/Cargo.lock"
const ASSET_DIR = "dist"

function assetPath(version: string): string {
  return join(process.cwd(), ASSET_DIR, `${APP_NAME}-${version}.zip`)
}

export async function defaultRunner(
  command: readonly string[],
  options?: { readonly allowFail?: boolean },
): Promise<CommandResult> {
  const process = Bun.spawn([...command], { stdout: "pipe", stderr: "pipe" })
  const stdout = await new Response(process.stdout).text()
  const stderr = await new Response(process.stderr).text()
  const code = await process.exited
  if (code !== 0 && options?.allowFail !== true) {
    throw new Error(`${command.join(" ")} failed (${code}): ${stderr.trim() || stdout.trim()}`)
  }
  return { stdout: stdout.trim(), stderr: stderr.trim(), code }
}

function lineName(line: ReleaseLine): string {
  return formatReleaseLine(line)
}

function versionText(version: ReleaseVersion): string {
  return formatVersion(version)
}

async function git(run: CommandRunner, args: readonly string[], allowFail = false): Promise<CommandResult> {
  return run(["git", ...args], { allowFail })
}

async function gh(run: CommandRunner, args: readonly string[], allowFail = false): Promise<CommandResult> {
  return run(["gh", ...args], { allowFail })
}

async function requireBranch(run: CommandRunner, name: string): Promise<string> {
  const result = await git(run, ["rev-parse", "--verify", `origin/${name}`], true)
  if (result.code !== 0) {
    throw new Error(`missing branch origin/${name}`)
  }
  return result.stdout
}

async function tagSha(run: CommandRunner, name: string): Promise<string | null> {
  const result = await git(run, ["rev-parse", "--verify", `refs/tags/${name}^{commit}`], true)
  return result.code === 0 ? result.stdout : null
}

async function fetchAll(run: CommandRunner): Promise<void> {
  await git(run, ["fetch", "origin", "--tags", "--force", "--prune"])
}

async function releaseJson(run: CommandRunner, tag: string): Promise<unknown> {
  const result = await gh(run, ["release", "view", tag, "--json", "isDraft,isPrerelease,tagName,targetCommitish"], true)
  if (result.code !== 0) return null
  return JSON.parse(result.stdout) as unknown
}

function isDraftRelease(value: unknown): boolean {
  return typeof value === "object" && value !== null && "isDraft" in value && value.isDraft === true
}

async function commitsOnMain(run: CommandRunner, shas: readonly string[]): Promise<void> {
  for (const sha of shas) {
    const exists = await git(run, ["cat-file", "-t", sha], true)
    if (exists.code !== 0 || exists.stdout !== "commit") {
      throw new Error(`commit ${sha} does not exist`)
    }
    const ancestor = await git(run, ["merge-base", "--is-ancestor", sha, "origin/main"], true)
    if (ancestor.code !== 0) {
      throw new Error(`commit ${sha} is not on origin/main`)
    }
  }
}

async function highestStableForMajor(run: CommandRunner, version: ReleaseVersion): Promise<boolean> {
  const tags = await git(run, ["tag", "--list"])
  const names = tags.stdout === "" ? [] : tags.stdout.split("\n")
  let highest: ReleaseVersion | null = version
  for (const name of names) {
    const parsed = parseVersion(name)
    if (parsed === null || parsed.major !== version.major || parsed.rc !== null) continue
    if (compareVersions(parsed, highest) > 0) highest = parsed
  }
  return compareVersions(highest, version) === 0
}

async function previousTagOnLine(run: CommandRunner, version: ReleaseVersion): Promise<string | null> {
  const tags = await git(run, ["tag", "--list"])
  const names = tags.stdout === "" ? [] : tags.stdout.split("\n")
  const matching = names
    .map((name) => parseVersion(name))
    .filter((parsed): parsed is ReleaseVersion => parsed !== null)
    .filter((parsed) => parsed.major === version.major && parsed.minor === version.minor)
    .filter((parsed) => compareVersions(parsed, version) < 0)
    .sort(compareVersions)
  const last = matching.at(-1)
  return last === undefined ? null : tagName(last)
}

async function collectNotes(run: CommandRunner, version: ReleaseVersion, line: string): Promise<string> {
  const previous = await previousTagOnLine(run, version)
  const range = previous === null ? `origin/${line}` : `${previous}..origin/${line}`
  const log = await git(run, ["log", "--pretty=format:- %s (%h)", range], true)
  const commits = log.code === 0 && log.stdout !== "" ? log.stdout : "- Initial release line cut from main."
  const versionString = versionText(version)
  return [
    `# ${APP_NAME} ${versionString}`,
    "",
    "## Changes",
    "",
    commits,
    "",
    "## Install",
    "",
    `Download \`${APP_NAME}-${versionString}.zip\`, unzip it, and move \`${APP_NAME}.app\` to Applications.`,
    "It is signed with a Developer ID and notarized by Apple. Requires macOS 14 or later on Apple Silicon.",
    "",
  ].join("\n")
}

async function replaceVersion(path: string, pattern: RegExp, version: string): Promise<boolean> {
  const raw = await readFile(join(process.cwd(), path), "utf8")
  const match = pattern.exec(raw)
  if (match === null) throw new Error(`no yapr version found in ${path}`)
  const updated = raw.replace(pattern, `$1${version}$2`)
  if (updated === raw) return false
  await writeFile(join(process.cwd(), path), updated)
  return true
}

async function setCargoVersion(version: string): Promise<boolean> {
  const manifest = await replaceVersion(CARGO_TOML, /(\[package\]\nname = "yapr"\nversion = ")[^"]+(")/, version)
  const lock = await replaceVersion(CARGO_LOCK, /(\nname = "yapr"\nversion = ")[^"]+(")/, version)
  return manifest || lock
}

async function applyMutations(
  run: CommandRunner,
  plan: OperationPlan,
  apply: boolean,
): Promise<void> {
  if (!apply) return
  for (const mutation of plan.mutations) {
    switch (mutation.kind) {
      case "checkout-branch":
        await git(run, ["checkout", "-B", mutation.name, `origin/${mutation.name}`])
        break
      case "create-branch":
        await git(run, ["branch", mutation.name, mutation.sha])
        await git(run, ["push", "origin", `refs/heads/${mutation.name}`])
        break
      case "open-pr":
        await gh(run, [
          "pr",
          "create",
          "--base",
          mutation.base,
          "--head",
          mutation.head,
          "--title",
          mutation.title,
          "--body",
          mutation.body,
        ])
        break
      case "commit-version": {
        const changed = await setCargoVersion(mutation.version)
        if (changed) {
          await git(run, ["add", CARGO_TOML, CARGO_LOCK])
          await git(run, ["commit", "-m", mutation.message])
        }
        break
      }
      case "build-app":
        await run(["scripts/package-release.sh", mutation.version, mutation.asset])
        break
      case "push-branch":
        await git(run, ["push", "origin", `HEAD:refs/heads/${mutation.branch}`])
        break
      case "create-tag": {
        const sha = mutation.sha === "HEAD" ? (await git(run, ["rev-parse", "HEAD"])).stdout : mutation.sha
        await git(run, ["tag", "-a", mutation.name, sha, "-m", mutation.message])
        await git(run, ["push", "origin", `refs/tags/${mutation.name}`])
        break
      }
      case "create-draft-release":
        await gh(run, [
          "release",
          "create",
          mutation.tag,
          "--draft",
          "--title",
          mutation.title,
          "--notes",
          mutation.notes,
          ...(mutation.prerelease === "true" ? ["--prerelease"] : []),
          mutation.asset,
        ])
        break
      case "move-alias":
        await git(run, ["tag", "-f", mutation.name, mutation.sha])
        await git(run, ["push", "--force", "origin", `refs/tags/${mutation.name}`])
        break
      case "publish-release":
        await gh(run, [
          "release",
          "edit",
          mutation.tag,
          "--draft=false",
          ...(mutation.latest === "true" ? ["--latest"] : ["--latest=false"]),
        ])
        break
      case "delete-release":
        await gh(run, ["release", "delete", mutation.tag, "--yes", "--cleanup-tag"], true)
        break
      case "delete-tag":
        await git(run, ["push", "origin", `:refs/tags/${mutation.name}`], true)
        await git(run, ["tag", "-d", mutation.name], true)
        break
      case "delete-branch":
        await git(run, ["push", "origin", `:refs/heads/${mutation.name}`])
        break
      case "cherry-pick-push":
        await git(run, ["checkout", "-B", mutation.head, mutation.base])
        for (const sha of mutation.commits.split(",")) {
          await git(run, ["cherry-pick", "-x", sha])
        }
        await git(run, ["push", "-u", "origin", mutation.head])
        break
      default:
        throw new Error(`unknown mutation ${mutation.kind}`)
    }
  }
}

export async function executeRelease(
  request: ReleaseRequest,
  run: CommandRunner = defaultRunner,
): Promise<OperationResult> {
  await fetchAll(run)
  const plan = await planRelease(request, run)
  try {
    await applyMutations(run, plan, !request.dryRun)
    return { ok: true, dryRun: request.dryRun, plan }
  } catch (error) {
    return {
      ok: false,
      dryRun: request.dryRun,
      plan,
      error: error instanceof Error ? error.message : String(error),
    }
  }
}

export async function planRelease(request: ReleaseRequest, run: CommandRunner): Promise<OperationPlan> {
  switch (request.operation) {
    case "cut":
      return planCut(request, run)
    case "backport":
      return planBackport(request, run)
    case "draft":
      return planDraft(request, run)
    case "publish":
      return planPublish(request, run)
    case "cancel":
      return planCancel(request, run)
    case "retire":
      return planRetire(request, run)
    case "restore":
      return planRestore(request, run)
  }
}

async function planCut(
  request: Extract<ReleaseRequest, { operation: "cut" }>,
  run: CommandRunner,
): Promise<OperationPlan> {
  const name = lineName(request.releaseLine)
  const existing = await git(run, ["rev-parse", "--verify", `origin/${name}`], true)
  if (existing.code === 0) throw new Error(`${name} already exists`)
  const mainSha = await requireBranch(run, "main")
  return {
    operation: "cut",
    dryRun: request.dryRun,
    version: versionText(request.version),
    releaseLine: name,
    preconditions: [`origin/main is ${mainSha}`, `${name} does not exist`],
    mutations: [
      {
        kind: "create-branch",
        summary: `create ${name} at origin/main`,
        name,
        sha: mainSha,
      },
    ],
  }
}

async function planBackport(
  request: Extract<ReleaseRequest, { operation: "backport" }>,
  run: CommandRunner,
): Promise<OperationPlan> {
  const name = lineName(request.releaseLine)
  const lineSha = await requireBranch(run, name)
  await commitsOnMain(run, request.commits)
  const head = `backport/${name.replace("release/", "")}-${request.commits[0]!.slice(0, 12)}`
  return {
    operation: "backport",
    dryRun: request.dryRun,
    releaseLine: name,
    preconditions: [
      `${name} is ${lineSha}`,
      `${request.commits.length} commit(s) are on origin/main`,
    ],
    mutations: [
      {
        kind: "cherry-pick-push",
        summary: `cherry-pick onto ${head}`,
        head,
        base: `origin/${name}`,
        commits: request.commits.join(","),
      },
      {
        kind: "open-pr",
        summary: `open squash PR into ${name}`,
        head,
        base: name,
        title: `backport: ${request.commits.length} commit(s) onto ${name}`,
        body: request.commits.map((sha) => `- ${sha}`).join("\n"),
      },
    ],
  }
}

async function planDraft(
  request: Extract<ReleaseRequest, { operation: "draft" }>,
  run: CommandRunner,
): Promise<OperationPlan> {
  const name = lineName(request.releaseLine)
  const version = versionText(request.version)
  const tag = tagName(request.version)
  const lineSha = await requireBranch(run, name)
  if (await tagSha(run, tag)) throw new Error(`tag ${tag} already exists`)
  if (await releaseJson(run, tag)) throw new Error(`GitHub release ${tag} already exists`)
  const notes = await collectNotes(run, request.version, name)
  const asset = assetPath(version)
  const mutations: Mutation[] = [
    {
      kind: "checkout-branch",
      summary: `check out ${name}`,
      name,
    },
    {
      kind: "commit-version",
      summary: `set ${CARGO_TOML} version to ${version}`,
      message: `chore(release): ${version}`,
      version,
    },
    {
      kind: "build-app",
      summary: `build, sign, and notarize ${APP_NAME}-${version}.zip`,
      version,
      asset,
    },
    {
      kind: "push-branch",
      summary: `push the version commit to ${name}`,
      branch: name,
    },
    {
      kind: "create-tag",
      summary: `tag ${tag} at ${name} HEAD`,
      name: tag,
      sha: "HEAD",
      message: `${APP_NAME} ${version}`,
    },
    {
      kind: "create-draft-release",
      summary: `create draft GitHub release ${tag} with ${APP_NAME}-${version}.zip`,
      tag,
      title: version,
      notes,
      prerelease: request.version.rc === null ? "false" : "true",
      asset,
    },
  ]
  return {
    operation: "draft",
    dryRun: request.dryRun,
    version,
    releaseLine: name,
    preconditions: [`${name} is ${lineSha}`, `tag ${tag} does not exist`],
    mutations,
    notes,
  }
}

async function planPublish(
  request: Extract<ReleaseRequest, { operation: "publish" }>,
  run: CommandRunner,
): Promise<OperationPlan> {
  const version = versionText(request.version)
  const tag = tagName(request.version)
  const sha = await tagSha(run, tag)
  if (sha === null) throw new Error(`tag ${tag} does not exist`)
  const release = await releaseJson(run, tag)
  if (release === null) throw new Error(`GitHub release ${tag} does not exist`)
  if (!isDraftRelease(release)) throw new Error(`GitHub release ${tag} is not a draft`)
  const moveAlias = request.version.rc === null && await highestStableForMajor(run, request.version)
  const mutations: Mutation[] = []
  if (moveAlias) {
    mutations.push({
      kind: "move-alias",
      summary: `point alias ${majorAlias(request.version)} at ${tag}`,
      name: majorAlias(request.version),
      sha,
    })
  }
  mutations.push({
    kind: "publish-release",
    summary: `publish GitHub release ${tag}`,
    tag,
    latest: moveAlias ? "true" : "false",
  })
  return {
    operation: "publish",
    dryRun: request.dryRun,
    version,
    releaseLine: lineName(request.releaseLine),
    preconditions: [`tag ${tag} is ${sha}`, `GitHub release ${tag} is a draft`],
    mutations,
  }
}

async function planCancel(
  request: Extract<ReleaseRequest, { operation: "cancel" }>,
  run: CommandRunner,
): Promise<OperationPlan> {
  const version = versionText(request.version)
  const tag = tagName(request.version)
  const sha = await tagSha(run, tag)
  const release = await releaseJson(run, tag)
  if (request.resumeProofJson === null) {
    if (release === null) throw new Error(`GitHub release ${tag} does not exist`)
    if (!isDraftRelease(release)) throw new Error(`GitHub release ${tag} is not a draft; refuse to cancel a published release`)
  } else {
    const proof = JSON.parse(request.resumeProofJson) as { kind?: string; tag?: string; tagSha?: string }
    if (proof.kind !== "prior-cancel-continuation" || proof.tag !== tag) {
      throw new Error("resume proof does not match this cancel")
    }
    if (sha !== null && proof.tagSha !== sha) {
      throw new Error("resume proof tag SHA does not match the live tag")
    }
  }
  return {
    operation: "cancel",
    dryRun: request.dryRun,
    version,
    releaseLine: lineName(request.releaseLine),
    preconditions: [`tag ${tag} may be deleted only while unpublished`],
    mutations: [
      { kind: "delete-release", summary: `delete draft GitHub release ${tag}`, tag },
      { kind: "delete-tag", summary: `delete tag ${tag}`, name: tag },
    ],
  }
}

async function planRetire(
  request: Extract<ReleaseRequest, { operation: "retire" }>,
  run: CommandRunner,
): Promise<OperationPlan> {
  const name = lineName(request.releaseLine)
  await requireBranch(run, name)
  return {
    operation: "retire",
    dryRun: request.dryRun,
    releaseLine: name,
    preconditions: [`${name} exists`, "published tags are left in place"],
    mutations: [{ kind: "delete-branch", summary: `delete ${name}`, name }],
  }
}

async function planRestore(
  request: Extract<ReleaseRequest, { operation: "restore" }>,
  run: CommandRunner,
): Promise<OperationPlan> {
  const name = lineName(request.releaseLine)
  const existing = await git(run, ["rev-parse", "--verify", `origin/${name}`], true)
  if (existing.code === 0) throw new Error(`${name} already exists`)
  const tags = await git(run, ["tag", "--list"])
  const names = tags.stdout === "" ? [] : tags.stdout.split("\n")
  const matching = names
    .map((value) => parseVersion(value))
    .filter((parsed): parsed is ReleaseVersion => parsed !== null)
    .filter((parsed) => parsed.major === request.releaseLine.major && parsed.minor === request.releaseLine.minor && parsed.rc === null)
    .sort(compareVersions)
  const highest = matching.at(-1)
  if (highest === undefined) throw new Error(`no published stable tag for ${name}`)
  const sha = await tagSha(run, tagName(highest))
  if (sha === null) throw new Error(`tag ${tagName(highest)} is missing`)
  return {
    operation: "restore",
    dryRun: request.dryRun,
    releaseLine: name,
    preconditions: [`highest stable tag is ${tagName(highest)}`],
    mutations: [
      {
        kind: "create-branch",
        summary: `restore ${name} at ${tagName(highest)}`,
        name,
        sha,
      },
    ],
  }
}

export async function writeArtifacts(outputDir: string, result: OperationResult): Promise<void> {
  await mkdir(outputDir, { recursive: true })
  await writeFile(join(outputDir, "release-plan.json"), `${JSON.stringify(result.plan, null, 2)}\n`)
  await writeFile(join(outputDir, "release-result.json"), `${JSON.stringify({
    ok: result.ok,
    dryRun: result.dryRun,
    error: result.error ?? null,
  }, null, 2)}\n`)
  if (result.plan.notes !== undefined) {
    await writeFile(join(outputDir, "release-notes.md"), result.plan.notes)
  }
}
