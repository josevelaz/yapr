/**
 * Strict version grammar for the release workflow.
 *
 * Only two shapes are legal:
 *   - stable:  `X.Y.Z`
 *   - release candidate: `X.Y.Z-rc.N` where `N >= 1`
 *
 * Rejected on purpose: a leading `v`, build metadata (`+meta`), any prerelease
 * identifier other than `rc`, `rc.0`, and leading zeros in any numeric field.
 *
 * This module is pure: no I/O, no `process.env`, no clocks.
 */

/** A parsed release version. `rc` is `null` for stable versions. */
export interface ReleaseVersion {
  readonly major: number;
  readonly minor: number;
  readonly patch: number;
  readonly rc: number | null;
}

/** A parsed release line (the `X.Y` pair behind `release/vX.Y`). */
export interface ReleaseLine {
  readonly major: number;
  readonly minor: number;
}

/** `0` or a non-zero-leading positive integer. */
const NUMERIC = '(?:0|[1-9][0-9]*)';

/** `rc.N` with `N >= 1` and no leading zeros. */
const RC = '(?:[1-9][0-9]*)';

const VERSION_PATTERN = new RegExp(
  `^(${NUMERIC})\\.(${NUMERIC})\\.(${NUMERIC})(?:-rc\\.(${RC}))?$`
);

const RELEASE_LINE_PATTERN = new RegExp(
  `^release/v(${NUMERIC})\\.(${NUMERIC})$`
);

/**
 * Parse a version string under the strict grammar.
 *
 * Returns `null` when the input does not match exactly. No normalization,
 * trimming, or coercion is performed.
 */
export function parseVersion(input: string): ReleaseVersion | null {
  const match = VERSION_PATTERN.exec(input);
  if (match === null) {
    return null;
  }

  const [, major, minor, patch, rc] = match;
  if (major === undefined || minor === undefined || patch === undefined) {
    return null;
  }

  return {
    major: Number(major),
    minor: Number(minor),
    patch: Number(patch),
    rc: rc === undefined ? null : Number(rc)
  };
}

/** Render a version back to its canonical string form (no leading `v`). */
export function formatVersion(version: ReleaseVersion): string {
  const core = `${version.major}.${version.minor}.${version.patch}`;
  return version.rc === null ? core : `${core}-rc.${version.rc}`;
}

/**
 * SemVer precedence comparison.
 *
 * Returns a negative number when `a < b`, zero when equal, positive when
 * `a > b`. A release candidate sorts before the stable version that shares its
 * core, and release candidates sort by their `rc` number.
 */
export function compareVersions(a: ReleaseVersion, b: ReleaseVersion): number {
  if (a.major !== b.major) {
    return a.major - b.major;
  }
  if (a.minor !== b.minor) {
    return a.minor - b.minor;
  }
  if (a.patch !== b.patch) {
    return a.patch - b.patch;
  }
  if (a.rc === null && b.rc === null) {
    return 0;
  }
  if (a.rc === null) {
    return 1;
  }
  if (b.rc === null) {
    return -1;
  }
  return a.rc - b.rc;
}

/** Extract the `X.Y` release line a version belongs to. */
export function releaseLineOf(version: ReleaseVersion): ReleaseLine {
  return { major: version.major, minor: version.minor };
}

/** Render a release line as its exact branch name, `release/vX.Y`. */
export function formatReleaseLine(line: ReleaseLine): string {
  return `release/v${line.major}.${line.minor}`;
}

/** Convenience: the branch name of the release line owning `version`. */
export function releaseBranchOf(version: ReleaseVersion): string {
  return formatReleaseLine(releaseLineOf(version));
}

/**
 * Parse an exact `release/vX.Y` branch name.
 *
 * Nothing is normalized: no trimming, no `refs/heads/` prefix, no trailing
 * `.Z`, no leading zeros. Returns `null` on any deviation.
 */
export function parseReleaseLine(input: string): ReleaseLine | null {
  const match = RELEASE_LINE_PATTERN.exec(input);
  if (match === null) {
    return null;
  }

  const [, major, minor] = match;
  if (major === undefined || minor === undefined) {
    return null;
  }

  return { major: Number(major), minor: Number(minor) };
}

/** True when two release lines refer to the same `X.Y`. */
export function releaseLinesEqual(a: ReleaseLine, b: ReleaseLine): boolean {
  return a.major === b.major && a.minor === b.minor;
}

/**
 * True when the version is the first version of a new release line, i.e. its
 * patch component is `0`. Both `1.4.0` and `1.4.0-rc.1` qualify.
 */
export function isInitialLineVersion(version: ReleaseVersion): boolean {
  return version.patch === 0;
}

/** The git tag for a version: the unprefixed canonical version string. */
export function tagName(version: ReleaseVersion): string {
  return formatVersion(version);
}

/** The floating major alias tag for a version, e.g. `1`. */
export function majorAlias(version: ReleaseVersion): string {
  return String(version.major);
}
