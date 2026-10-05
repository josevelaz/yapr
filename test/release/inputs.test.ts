import { describe, expect, it } from 'bun:test';
import {
  confirmationMatches,
  parseCommitList,
  parseReleaseInputs,
  RELEASE_OPERATIONS,
  type RawReleaseInputs,
  type ReleaseInputError,
  type ReleaseRequest
} from '../../.github/scripts/release/src/inputs.ts';

const SHA_A = 'a'.repeat(40);
const SHA_B = 'b'.repeat(40);
const SHA_C = '0123456789abcdef0123456789abcdef01234567';

function expectOk(raw: RawReleaseInputs): ReleaseRequest {
  const result = parseReleaseInputs(raw);
  if (!result.ok) {
    throw new Error(
      `expected success, got errors: ${JSON.stringify(result.errors)}`
    );
  }
  return result.request;
}

function expectErrors(raw: RawReleaseInputs): readonly ReleaseInputError[] {
  const result = parseReleaseInputs(raw);
  if (result.ok) {
    throw new Error(
      `expected failure, got request: ${JSON.stringify(result.request)}`
    );
  }
  expect(result.errors.length).toBeGreaterThan(0);
  return result.errors;
}

function fields(errors: readonly ReleaseInputError[]): string[] {
  return errors.map((e) => e.field).sort();
}

describe('operation validation', () => {
  it('rejects a missing operation', () => {
    expect(fields(expectErrors({}))).toEqual(['operation']);
    expect(fields(expectErrors({ operation: '   ' }))).toEqual(['operation']);
  });

  it('rejects an unknown operation', () => {
    const errors = expectErrors({ operation: 'delete' });
    expect(errors[0]?.code).toBe('invalid');
  });

  it('exposes exactly the supported operations', () => {
    expect([...RELEASE_OPERATIONS]).toEqual([
      'cut',
      'backport',
      'draft',
      'publish',
      'cancel',
      'retire',
      'restore'
    ]);
  });
});

describe('cut', () => {
  it('accepts an initial stable line version', () => {
    expect(expectOk({ operation: 'cut', version: '1.4.0' })).toEqual({
      operation: 'cut',
      version: { major: 1, minor: 4, patch: 0, rc: null },
      releaseLine: { major: 1, minor: 4 },
      dryRun: false
    });
  });

  it('accepts an initial rc line version', () => {
    const request = expectOk({ operation: 'cut', version: '2.0.0-rc.1' });
    expect(request).toMatchObject({
      operation: 'cut',
      releaseLine: { major: 2, minor: 0 }
    });
  });

  it('rejects a non-initial version', () => {
    const errors = expectErrors({ operation: 'cut', version: '1.4.1' });
    expect(
      errors.some((e) => e.field === 'version' && e.code === 'invalid')
    ).toBe(true);
  });

  it('rejects a missing version', () => {
    expect(fields(expectErrors({ operation: 'cut' }))).toEqual(['version']);
  });

  it('rejects an invalid version', () => {
    expect(
      fields(expectErrors({ operation: 'cut', version: 'v1.4.0' }))
    ).toEqual(['version']);
  });

  it.each(['release_line', 'commits', 'confirmation', 'resume_proof'] as const)(
    'rejects a populated %s',
    (field) => {
      const raw: RawReleaseInputs = {
        operation: 'cut',
        version: '1.4.0',
        [field]: 'something'
      };
      expect(fields(expectErrors(raw))).toEqual([field]);
    }
  );
});

describe('backport', () => {
  it('accepts a comma-separated commit list', () => {
    expect(
      expectOk({
        operation: 'backport',
        release_line: 'release/v1.4',
        commits: `${SHA_A},${SHA_B}`
      })
    ).toEqual({
      operation: 'backport',
      releaseLine: { major: 1, minor: 4 },
      commits: [SHA_A, SHA_B],
      dryRun: false
    });
  });

  it('accepts whitespace and mixed separators', () => {
    const request = expectOk({
      operation: 'backport',
      release_line: 'release/v1.4',
      commits: ` ${SHA_A} , ${SHA_B}\n${SHA_C} `
    });
    expect(request).toMatchObject({ commits: [SHA_A, SHA_B, SHA_C] });
  });

  it('rejects a missing release_line', () => {
    expect(
      fields(expectErrors({ operation: 'backport', commits: SHA_A }))
    ).toEqual(['release_line']);
  });

  it('rejects missing commits', () => {
    expect(
      fields(
        expectErrors({ operation: 'backport', release_line: 'release/v1.4' })
      )
    ).toEqual(['commits']);
  });

  it.each([
    ['abc1234', 'short sha'],
    ['A'.repeat(40), 'uppercase hex'],
    ['g'.repeat(40), 'non-hex characters'],
    ['a'.repeat(41), 'over-long sha']
  ])('rejects %s (%s)', (commit) => {
    const errors = expectErrors({
      operation: 'backport',
      release_line: 'release/v1.4',
      commits: commit
    });
    expect(fields(errors)).toContain('commits');
  });

  it('rejects duplicate commits', () => {
    const errors = expectErrors({
      operation: 'backport',
      release_line: 'release/v1.4',
      commits: `${SHA_A} ${SHA_A}`
    });
    expect(errors.some((e) => e.message.includes('duplicate'))).toBe(true);
  });

  it.each(['version', 'confirmation', 'resume_proof'] as const)(
    'rejects a populated %s',
    (field) => {
      const raw: RawReleaseInputs = {
        operation: 'backport',
        release_line: 'release/v1.4',
        commits: SHA_A,
        [field]: 'something'
      };
      expect(fields(expectErrors(raw))).toEqual([field]);
    }
  );
});

describe('draft', () => {
  it('accepts a matching line and version', () => {
    expect(
      expectOk({
        operation: 'draft',
        release_line: 'release/v1.4',
        version: '1.4.0-rc.1'
      })
    ).toEqual({
      operation: 'draft',
      releaseLine: { major: 1, minor: 4 },
      version: { major: 1, minor: 4, patch: 0, rc: 1 },
      dryRun: false
    });
  });

  it('accepts a patch version on the line', () => {
    const request = expectOk({
      operation: 'draft',
      release_line: 'release/v1.4',
      version: '1.4.7'
    });
    expect(request).toMatchObject({ operation: 'draft' });
  });

  it('rejects a version outside the line', () => {
    const errors = expectErrors({
      operation: 'draft',
      release_line: 'release/v1.4',
      version: '1.5.0'
    });
    expect(errors.some((e) => e.code === 'mismatch')).toBe(true);
  });

  it('rejects missing inputs', () => {
    expect(fields(expectErrors({ operation: 'draft' }))).toEqual([
      'release_line',
      'version'
    ]);
  });

  it.each(['commits', 'confirmation', 'resume_proof'] as const)(
    'rejects a populated %s',
    (field) => {
      const raw: RawReleaseInputs = {
        operation: 'draft',
        release_line: 'release/v1.4',
        version: '1.4.0',
        [field]: 'something'
      };
      expect(fields(expectErrors(raw))).toEqual([field]);
    }
  );
});

describe('publish', () => {
  it('accepts an exact confirmation', () => {
    expect(
      expectOk({
        operation: 'publish',
        version: '1.4.0',
        confirmation: 'publish 1.4.0'
      })
    ).toEqual({
      operation: 'publish',
      version: { major: 1, minor: 4, patch: 0, rc: null },
      releaseLine: { major: 1, minor: 4 },
      dryRun: false
    });
  });

  it('accepts an rc confirmation', () => {
    const request = expectOk({
      operation: 'publish',
      version: '1.4.0-rc.2',
      confirmation: 'publish 1.4.0-rc.2'
    });
    expect(request).toMatchObject({ operation: 'publish' });
  });

  it('trims only outer whitespace on the confirmation', () => {
    const request = expectOk({
      operation: 'publish',
      version: '1.4.0',
      confirmation: '  publish 1.4.0\n'
    });
    expect(request).toMatchObject({ operation: 'publish' });
  });

  it.each([
    ['publish 1.4.1', 'wrong version'],
    ['publish  v1.4.0', 'extra inner whitespace'],
    ['Publish v1.4.0', 'wrong case'],
    ['publish v1.4.0', 'leading v'],
    ['publish 1.4.0 now', 'trailing words'],
    ['cancel 1.4.0', 'wrong verb']
  ])('rejects the confirmation %s (%s)', (confirmation) => {
    const errors = expectErrors({
      operation: 'publish',
      version: '1.4.0',
      confirmation
    });
    expect(
      errors.some((e) => e.field === 'confirmation' && e.code === 'mismatch')
    ).toBe(true);
  });

  it('rejects a missing confirmation', () => {
    const errors = expectErrors({ operation: 'publish', version: '1.4.0' });
    expect(
      errors.some((e) => e.field === 'confirmation' && e.code === 'missing')
    ).toBe(true);
  });

  it('rejects a missing version', () => {
    const errors = expectErrors({
      operation: 'publish',
      confirmation: 'publish 1.4.0'
    });
    expect(fields(errors)).toContain('version');
  });

  it.each(['release_line', 'commits', 'resume_proof'] as const)(
    'rejects a populated %s',
    (field) => {
      const raw: RawReleaseInputs = {
        operation: 'publish',
        version: '1.4.0',
        confirmation: 'publish 1.4.0',
        [field]: 'something'
      };
      expect(fields(expectErrors(raw))).toEqual([field]);
    }
  );
});

describe('cancel', () => {
  it('accepts an exact confirmation', () => {
    expect(
      expectOk({
        operation: 'cancel',
        version: '1.4.0-rc.1',
        confirmation: 'cancel 1.4.0-rc.1'
      })
    ).toEqual({
      operation: 'cancel',
      version: { major: 1, minor: 4, patch: 0, rc: 1 },
      releaseLine: { major: 1, minor: 4 },
      resumeProofJson: null,
      dryRun: false
    });
  });

  it('treats empty resume_proof as absent and keeps exact JSON bytes when set', () => {
    expect(
      expectOk({
        operation: 'cancel',
        version: '1.4.0',
        confirmation: 'cancel 1.4.0',
        resume_proof: '   '
      })
    ).toMatchObject({ resumeProofJson: null });

    const exact = '{\n  "kind": "prior-cancel-continuation"\n}';
    expect(
      expectOk({
        operation: 'cancel',
        version: '1.4.0',
        confirmation: 'cancel 1.4.0',
        resume_proof: exact
      })
    ).toMatchObject({ resumeProofJson: exact });
  });

  it('rejects a publish confirmation', () => {
    const errors = expectErrors({
      operation: 'cancel',
      version: '1.4.0',
      confirmation: 'publish 1.4.0'
    });
    expect(errors.some((e) => e.code === 'mismatch')).toBe(true);
  });

  it.each(['release_line', 'commits'] as const)(
    'rejects a populated %s',
    (field) => {
      const raw: RawReleaseInputs = {
        operation: 'cancel',
        version: '1.4.0',
        confirmation: 'cancel 1.4.0',
        [field]: 'something'
      };
      expect(fields(expectErrors(raw))).toEqual([field]);
    }
  );
});

describe('retire', () => {
  it('accepts an exact confirmation', () => {
    expect(
      expectOk({
        operation: 'retire',
        release_line: 'release/v1.4',
        confirmation: 'retire release/v1.4'
      })
    ).toEqual({
      operation: 'retire',
      releaseLine: { major: 1, minor: 4 },
      dryRun: false
    });
  });

  it.each([
    ['retire release/v1.5', 'wrong line'],
    ['retire v1.4', 'missing prefix'],
    ['retire release/v1.4.0', 'patch included'],
    ['Retire release/v1.4', 'wrong case'],
    ['restore release/v1.4', 'wrong verb']
  ])('rejects the confirmation %s (%s)', (confirmation) => {
    const errors = expectErrors({
      operation: 'retire',
      release_line: 'release/v1.4',
      confirmation
    });
    expect(errors.some((e) => e.code === 'mismatch')).toBe(true);
  });

  it('rejects a missing confirmation', () => {
    const errors = expectErrors({
      operation: 'retire',
      release_line: 'release/v1.4'
    });
    expect(fields(errors)).toEqual(['confirmation']);
  });

  it.each(['version', 'commits', 'resume_proof'] as const)(
    'rejects a populated %s',
    (field) => {
      const raw: RawReleaseInputs = {
        operation: 'retire',
        release_line: 'release/v1.4',
        confirmation: 'retire release/v1.4',
        [field]: 'something'
      };
      expect(fields(expectErrors(raw))).toEqual([field]);
    }
  );
});

describe('restore', () => {
  it('accepts a release line alone', () => {
    expect(
      expectOk({ operation: 'restore', release_line: 'release/v1.4' })
    ).toEqual({
      operation: 'restore',
      releaseLine: { major: 1, minor: 4 },
      dryRun: false
    });
  });

  it('rejects a missing release line', () => {
    expect(fields(expectErrors({ operation: 'restore' }))).toEqual([
      'release_line'
    ]);
  });

  it.each(['version', 'commits', 'confirmation', 'resume_proof'] as const)(
    'rejects a populated %s',
    (field) => {
      const raw: RawReleaseInputs = {
        operation: 'restore',
        release_line: 'release/v1.4',
        [field]: 'something'
      };
      expect(fields(expectErrors(raw))).toEqual([field]);
    }
  );
});

describe('dry_run', () => {
  interface BaseContract {
    readonly [key: string]: RawReleaseInputs;
  }
  const base: BaseContract = {
    cut: { operation: 'cut', version: '1.4.0' },
    backport: {
      operation: 'backport',
      release_line: 'release/v1.4',
      commits: SHA_A
    },
    draft: {
      operation: 'draft',
      release_line: 'release/v1.4',
      version: '1.4.0'
    },
    publish: {
      operation: 'publish',
      version: '1.4.0',
      confirmation: 'publish 1.4.0'
    },
    cancel: {
      operation: 'cancel',
      version: '1.4.0',
      confirmation: 'cancel 1.4.0'
    },
    retire: {
      operation: 'retire',
      release_line: 'release/v1.4',
      confirmation: 'retire release/v1.4'
    },
    restore: { operation: 'restore', release_line: 'release/v1.4' }
  };

  it.each(RELEASE_OPERATIONS)('is accepted for %s', (operation) => {
    const raw = base[operation];
    expect(raw).toBeDefined();
    expect(expectOk({ ...raw, dry_run: 'true' })).toMatchObject({
      dryRun: true
    });
    expect(expectOk({ ...raw, dry_run: 'false' })).toMatchObject({
      dryRun: false
    });
    expect(expectOk({ ...raw, dry_run: '' })).toMatchObject({ dryRun: false });
  });

  it('rejects a non-boolean value', () => {
    const errors = expectErrors({
      operation: 'restore',
      release_line: 'release/v1.4',
      dry_run: 'yes'
    });
    expect(fields(errors)).toEqual(['dry_run']);
  });
});

describe('parseCommitList', () => {
  it('separates valid, invalid, and duplicate entries', () => {
    const result = parseCommitList(`${SHA_A}, bad, ${SHA_B} ${SHA_A}`);
    expect(result.commits).toEqual([SHA_A, SHA_B]);
    expect(result.invalid).toEqual(['bad']);
    expect(result.duplicates).toEqual([SHA_A]);
  });

  it('returns nothing for an empty list', () => {
    expect(parseCommitList('   ')).toEqual({
      commits: [],
      invalid: [],
      duplicates: []
    });
  });
});

describe('confirmationMatches', () => {
  it('trims only outer whitespace', () => {
    expect(confirmationMatches('  publish 1.0.0  ', 'publish 1.0.0')).toBe(
      true
    );
    expect(confirmationMatches('publish  v1.0.0', 'publish 1.0.0')).toBe(false);
    expect(confirmationMatches('publish 1.0.0', 'publish 1.0.1')).toBe(false);
  });
});

describe('errors accumulate', () => {
  it('reports several problems at once', () => {
    const errors = expectErrors({
      operation: 'restore',
      version: '1.4.0',
      commits: SHA_A,
      confirmation: 'restore release/v1.4'
    });
    expect(fields(errors)).toEqual([
      'commits',
      'confirmation',
      'release_line',
      'version'
    ]);
  });
});
