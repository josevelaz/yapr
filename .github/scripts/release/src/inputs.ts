/**
 * Per-operation input parsing for the release workflow.
 *
 * This module is pure: it validates raw string inputs into a typed
 * discriminated union, or returns a structured list of errors. It performs no
 * I/O, reads no `process.env`, and never touches git. Reachability of backport
 * commits and existence of branches/tags are validated elsewhere.
 */

import {
  formatReleaseLine,
  isInitialLineVersion,
  parseReleaseLine,
  parseVersion,
  type ReleaseLine,
  type ReleaseVersion
} from './semver.ts';

/** Every supported workflow operation. */
export const RELEASE_OPERATIONS = [
  'cut',
  'backport',
  'draft',
  'publish',
  'cancel',
  'retire',
  'restore'
] as const;

export type ReleaseOperation = (typeof RELEASE_OPERATIONS)[number];

/**
 * Raw, untrusted workflow inputs. Every field is a string or absent.
 *
 * The workflow exposes optional `resume_proof`. It is accepted
 * here for `cancel` only: repository state cannot distinguish a half-finished
 * cancel from a half-finished draft, so the operator must re-supply the prior
 * run's proof bytes when resuming tag deletion.
 */
export interface RawReleaseInputs {
  readonly operation?: string | undefined;
  readonly version?: string | undefined;
  readonly release_line?: string | undefined;
  readonly commits?: string | undefined;
  readonly confirmation?: string | undefined;
  /**
   * Exact JSON text of a cancel resume proof, or absent.
   *
   * Carried through as an opaque string for `cancel`; structural parsing is
   * `parseCancelResumeProofJson` in `operations/cancel.ts`. Empty /
   * whitespace-only is absent. Inner JSON bytes are never normalized.
   */
  readonly resume_proof?: string | undefined;
  readonly dry_run?: string | undefined;
}

/** A structured validation failure. */
export interface ReleaseInputError {
  /** The raw input field the error belongs to. */
  readonly field: keyof RawReleaseInputs;
  /** A stable machine-readable code. */
  readonly code: ReleaseInputErrorCode;
  /** A human-readable explanation. */
  readonly message: string;
}

export type ReleaseInputErrorCode =
  | 'missing'
  | 'invalid'
  | 'forbidden'
  | 'mismatch';

export interface CutRequest {
  readonly operation: 'cut';
  readonly version: ReleaseVersion;
  readonly releaseLine: ReleaseLine;
  readonly dryRun: boolean;
}

export interface BackportRequest {
  readonly operation: 'backport';
  readonly releaseLine: ReleaseLine;
  readonly commits: readonly string[];
  readonly dryRun: boolean;
}

export interface DraftRequest {
  readonly operation: 'draft';
  readonly releaseLine: ReleaseLine;
  readonly version: ReleaseVersion;
  readonly dryRun: boolean;
}

export interface PublishRequest {
  readonly operation: 'publish';
  readonly version: ReleaseVersion;
  readonly releaseLine: ReleaseLine;
  readonly dryRun: boolean;
}

export interface CancelRequest {
  readonly operation: 'cancel';
  readonly version: ReleaseVersion;
  readonly releaseLine: ReleaseLine;
  /**
   * Exact `resume_proof` input text, or `null` when absent / empty.
   *
   * Not parsed here. Call `parseCancelResumeProofJson` before planning.
   */
  readonly resumeProofJson: string | null;
  readonly dryRun: boolean;
}

export interface RetireRequest {
  readonly operation: 'retire';
  readonly releaseLine: ReleaseLine;
  readonly dryRun: boolean;
}

export interface RestoreRequest {
  readonly operation: 'restore';
  readonly releaseLine: ReleaseLine;
  readonly dryRun: boolean;
}

/** The typed result of a successful parse. */
export type ReleaseRequest =
  | CutRequest
  | BackportRequest
  | DraftRequest
  | PublishRequest
  | CancelRequest
  | RetireRequest
  | RestoreRequest;

export type ParseReleaseInputsResult =
  | { readonly ok: true; readonly request: ReleaseRequest }
  | { readonly ok: false; readonly errors: readonly ReleaseInputError[] };

const COMMIT_SHA_PATTERN = /^[0-9a-f]{40}$/;

/** Fields other than `operation` and `dry_run`, which every operation accepts. */
const SCOPED_FIELDS = [
  'version',
  'release_line',
  'commits',
  'confirmation',
  'resume_proof'
] as const;

type ScopedField = (typeof SCOPED_FIELDS)[number];

/** Which scoped fields each operation uses. Everything else is forbidden. */
type ALLOWEDFIELDSContract = {
  readonly [K in ReleaseOperation]: readonly ScopedField[];
};
const ALLOWED_FIELDS: ALLOWEDFIELDSContract = {
  cut: ['version'],
  backport: ['release_line', 'commits'],
  draft: ['release_line', 'version'],
  publish: ['version', 'confirmation'],
  // `resume_proof` is optional for cancel; required fields are enforced below.
  cancel: ['version', 'confirmation', 'resume_proof'],
  retire: ['release_line', 'confirmation'],
  restore: ['release_line']
};

function isPopulated(value: string | undefined): value is string {
  return value !== undefined && value.trim() !== '';
}

function isReleaseOperation(value: string): value is ReleaseOperation {
  // SAFETY: this boundary established the `readonly string[]` contract before the assertion.
  return (RELEASE_OPERATIONS as readonly string[]).includes(value);
}

function error(
  field: keyof RawReleaseInputs,
  code: ReleaseInputErrorCode,
  message: string
): ReleaseInputError {
  return { field, code, message };
}

/**
 * Parse `dry_run`. Absent or empty means `false`; only the exact strings
 * `true` and `false` are accepted.
 */
function parseDryRun(
  raw: string | undefined,
  errors: ReleaseInputError[]
): boolean {
  if (raw === undefined || raw.trim() === '') {
    return false;
  }
  const value = raw.trim();
  if (value === 'true') {
    return true;
  }
  if (value === 'false') {
    return false;
  }
  errors.push(
    error(
      'dry_run',
      'invalid',
      `dry_run must be "true" or "false", got "${raw}"`
    )
  );
  return false;
}

/**
 * Split a commit list on commas and/or whitespace and validate each entry as a
 * full 40-character lowercase hex SHA. Duplicates are rejected. Whether a SHA
 * exists, is reachable, or is a merge commit is decided by a later layer that
 * has repository access.
 */
type ParseCommitListContract = {
  readonly commits: readonly string[];
  readonly invalid: readonly string[];
  readonly duplicates: readonly string[];
};
export function parseCommitList(raw: string): ParseCommitListContract {
  const tokens = raw.split(/[\s,]+/).filter((token) => token !== '');
  const commits: string[] = [];
  const invalid: string[] = [];
  const duplicates: string[] = [];
  const seen = new Set<string>();

  for (const token of tokens) {
    if (!COMMIT_SHA_PATTERN.test(token)) {
      invalid.push(token);
      continue;
    }
    if (seen.has(token)) {
      duplicates.push(token);
      continue;
    }
    seen.add(token);
    commits.push(token);
  }

  return { commits, invalid, duplicates };
}

/**
 * Compare a confirmation input against its expected phrase. Only the outer
 * whitespace of the raw input is trimmed; the remainder must match exactly.
 */
export function confirmationMatches(raw: string, expected: string): boolean {
  return raw.trim() === expected;
}

function requireVersion(
  raw: string | undefined,
  errors: ReleaseInputError[]
): ReleaseVersion | null {
  if (!isPopulated(raw)) {
    errors.push(error('version', 'missing', 'version is required'));
    return null;
  }
  const version = parseVersion(raw);
  if (version === null) {
    errors.push(
      error(
        'version',
        'invalid',
        `version must be X.Y.Z or X.Y.Z-rc.N, got "${raw}"`
      )
    );
    return null;
  }
  return version;
}

function requireReleaseLine(
  raw: string | undefined,
  errors: ReleaseInputError[]
): ReleaseLine | null {
  if (!isPopulated(raw)) {
    errors.push(error('release_line', 'missing', 'release_line is required'));
    return null;
  }
  const line = parseReleaseLine(raw);
  if (line === null) {
    errors.push(
      error(
        'release_line',
        'invalid',
        `release_line must be exactly release/vX.Y, got "${raw}"`
      )
    );
    return null;
  }
  return line;
}

function requireConfirmation(
  raw: string | undefined,
  expected: string,
  errors: ReleaseInputError[]
): boolean {
  if (raw === undefined || raw.trim() === '') {
    errors.push(
      error(
        'confirmation',
        'missing',
        `confirmation is required and must be "${expected}"`
      )
    );
    return false;
  }
  if (!confirmationMatches(raw, expected)) {
    errors.push(
      error(
        'confirmation',
        'mismatch',
        `confirmation must be exactly "${expected}", got "${raw.trim()}"`
      )
    );
    return false;
  }
  return true;
}

function rejectForbiddenFields(
  operation: ReleaseOperation,
  raw: RawReleaseInputs,
  errors: ReleaseInputError[]
): void {
  const allowed = ALLOWED_FIELDS[operation];
  for (const field of SCOPED_FIELDS) {
    if (allowed.includes(field)) {
      continue;
    }
    if (isPopulated(raw[field])) {
      errors.push(
        error(field, 'forbidden', `${field} is not accepted for ${operation}`)
      );
    }
  }
}

/**
 * Validate raw workflow inputs into a typed request or a list of errors.
 *
 * `dry_run` is accepted for every operation. Every other field is required,
 * or forbidden, per operation; a populated irrelevant field is an error.
 */
export function parseReleaseInputs(
  raw: RawReleaseInputs
): ParseReleaseInputsResult {
  const errors: ReleaseInputError[] = [];

  if (!isPopulated(raw.operation)) {
    return {
      ok: false,
      errors: [error('operation', 'missing', 'operation is required')]
    };
  }

  const operation = raw.operation.trim();
  if (!isReleaseOperation(operation)) {
    return {
      ok: false,
      errors: [
        error(
          'operation',
          'invalid',
          `operation must be one of ${RELEASE_OPERATIONS.join(', ')}, got "${operation}"`
        )
      ]
    };
  }

  rejectForbiddenFields(operation, raw, errors);
  const dryRun = parseDryRun(raw.dry_run, errors);

  switch (operation) {
    case 'cut': {
      const version = requireVersion(raw.version, errors);
      if (version !== null && !isInitialLineVersion(version)) {
        errors.push(
          error(
            'version',
            'invalid',
            `cut requires an initial line version with patch 0, got "${raw.version ?? ''}"`
          )
        );
      }
      if (version === null || errors.length > 0) {
        break;
      }
      return {
        ok: true,
        request: {
          operation: 'cut',
          version,
          releaseLine: { major: version.major, minor: version.minor },
          dryRun
        }
      };
    }

    case 'backport': {
      const line = requireReleaseLine(raw.release_line, errors);
      let commits: readonly string[] = [];
      if (!isPopulated(raw.commits)) {
        errors.push(error('commits', 'missing', 'commits is required'));
      } else {
        const parsed = parseCommitList(raw.commits);
        for (const token of parsed.invalid) {
          errors.push(
            error(
              'commits',
              'invalid',
              `commits entries must be full 40-character lowercase hex SHAs, got "${token}"`
            )
          );
        }
        for (const token of parsed.duplicates) {
          errors.push(
            error(
              'commits',
              'invalid',
              `commits contains duplicate SHA "${token}"`
            )
          );
        }
        if (parsed.commits.length === 0 && parsed.invalid.length === 0) {
          errors.push(
            error('commits', 'missing', 'commits must list at least one SHA')
          );
        }
        commits = parsed.commits;
      }
      if (line === null || errors.length > 0) {
        break;
      }
      return {
        ok: true,
        request: { operation: 'backport', releaseLine: line, commits, dryRun }
      };
    }

    case 'draft': {
      const line = requireReleaseLine(raw.release_line, errors);
      const version = requireVersion(raw.version, errors);
      if (
        line !== null &&
        version !== null &&
        (version.major !== line.major || version.minor !== line.minor)
      ) {
        errors.push(
          error(
            'version',
            'mismatch',
            `version ${raw.version ?? ''} does not belong to ${formatReleaseLine(line)}`
          )
        );
      }
      if (line === null || version === null || errors.length > 0) {
        break;
      }
      return {
        ok: true,
        request: { operation: 'draft', releaseLine: line, version, dryRun }
      };
    }

    case 'publish':
    case 'cancel': {
      const version = requireVersion(raw.version, errors);
      const expected =
        version === null
          ? null
          : `${operation} ${formatVersionInput(raw.version)}`;
      if (expected !== null) {
        requireConfirmation(raw.confirmation, expected, errors);
      } else if (!isPopulated(raw.confirmation)) {
        errors.push(
          error('confirmation', 'missing', 'confirmation is required')
        );
      }
      if (version === null || errors.length > 0) {
        break;
      }
      const releaseLine = { major: version.major, minor: version.minor };
      if (operation === 'publish') {
        return {
          ok: true,
          request: { operation: 'publish', version, releaseLine, dryRun }
        };
      }
      // Keep the exact input bytes when present. Empty / whitespace-only is
      // absent. Do not parse or re-stringify here — the workflow wires the
      // optional input; parseCancelResumeProofJson validates shape.
      const resumeProofJson = isPopulated(raw.resume_proof)
        ? raw.resume_proof
        : null;
      return {
        ok: true,
        request: {
          operation: 'cancel',
          version,
          releaseLine,
          resumeProofJson,
          dryRun
        }
      };
    }

    case 'retire': {
      const line = requireReleaseLine(raw.release_line, errors);
      if (line !== null) {
        requireConfirmation(
          raw.confirmation,
          `retire ${formatReleaseLine(line)}`,
          errors
        );
      } else if (!isPopulated(raw.confirmation)) {
        errors.push(
          error('confirmation', 'missing', 'confirmation is required')
        );
      }
      if (line === null || errors.length > 0) {
        break;
      }
      return {
        ok: true,
        request: { operation: 'retire', releaseLine: line, dryRun }
      };
    }

    case 'restore': {
      const line = requireReleaseLine(raw.release_line, errors);
      if (line === null || errors.length > 0) {
        break;
      }
      return {
        ok: true,
        request: { operation: 'restore', releaseLine: line, dryRun }
      };
    }
  }

  return { ok: false, errors };
}

/** The already-validated version text used to build a confirmation phrase. */
function formatVersionInput(raw: string | undefined): string {
  return raw === undefined ? '' : raw.trim();
}
