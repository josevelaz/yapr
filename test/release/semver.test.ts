import { describe, expect, it } from 'bun:test';
import {
  compareVersions,
  formatReleaseLine,
  formatVersion,
  isInitialLineVersion,
  majorAlias,
  parseReleaseLine,
  parseVersion,
  releaseBranchOf,
  releaseLineOf,
  releaseLinesEqual,
  tagName,
  type ReleaseVersion
} from '../../.github/scripts/release/src/semver.ts';

describe('parseVersion', () => {
  it.each([
    ['0.0.0', { major: 0, minor: 0, patch: 0, rc: null }],
    ['1.2.3', { major: 1, minor: 2, patch: 3, rc: null }],
    ['10.20.30', { major: 10, minor: 20, patch: 30, rc: null }],
    ['1.4.0-rc.1', { major: 1, minor: 4, patch: 0, rc: 1 }],
    ['2.0.0-rc.12', { major: 2, minor: 0, patch: 0, rc: 12 }]
  ])('accepts %s', (input, expected) => {
    expect(parseVersion(input)).toEqual(expected);
  });

  it.each([
    ['v1.2.3', 'leading v'],
    ['V1.2.3', 'leading uppercase V'],
    ['1.2.3+build.5', 'build metadata'],
    ['1.2.3-rc.1+build', 'build metadata on an rc'],
    ['1.2.3-alpha.1', 'non-rc prerelease id'],
    ['1.2.3-beta', 'non-rc prerelease id'],
    ['1.2.3-rc', 'rc without a number'],
    ['1.2.3-rc.', 'rc with an empty number'],
    ['1.2.3-rc.0', 'rc.0'],
    ['1.2.3-rc.01', 'leading zero in rc number'],
    ['1.2.3-RC.1', 'uppercase RC'],
    ['01.2.3', 'leading zero in major'],
    ['1.02.3', 'leading zero in minor'],
    ['1.2.03', 'leading zero in patch'],
    ['1.2', 'missing patch'],
    ['1.2.3.4', 'extra component'],
    ['1.2.x', 'non-numeric patch'],
    [' 1.2.3', 'leading whitespace'],
    ['1.2.3 ', 'trailing whitespace'],
    ['1.2.3\n', 'trailing newline'],
    ['', 'empty string'],
    ['release/v1.2', 'release line']
  ])('rejects %s (%s)', (input) => {
    expect(parseVersion(input)).toBeNull();
  });
});

describe('formatVersion', () => {
  it.each(['0.0.0', '1.2.3', '1.4.0-rc.1', '10.20.30-rc.99'])(
    'round-trips %s',
    (input) => {
      const parsed = parseVersion(input);
      expect(parsed).not.toBeNull();
      expect(formatVersion(parsed!)).toBe(input);
    }
  );
});

function v(input: string): ReleaseVersion {
  const parsed = parseVersion(input);
  if (parsed === null) {
    throw new Error(`test fixture is not a valid version: ${input}`);
  }
  return parsed;
}

describe('compareVersions', () => {
  it('orders an rc before its stable release', () => {
    expect(compareVersions(v('1.4.0-rc.1'), v('1.4.0'))).toBeLessThan(0);
    expect(compareVersions(v('1.4.0'), v('1.4.0-rc.1'))).toBeGreaterThan(0);
  });

  it('orders rc chains by their rc number', () => {
    expect(compareVersions(v('1.4.0-rc.1'), v('1.4.0-rc.2'))).toBeLessThan(0);
    expect(compareVersions(v('1.4.0-rc.2'), v('1.4.0-rc.10'))).toBeLessThan(0);
    expect(compareVersions(v('1.4.0-rc.10'), v('1.4.0-rc.9'))).toBeGreaterThan(
      0
    );
  });

  it('treats identical versions as equal', () => {
    expect(compareVersions(v('1.2.3'), v('1.2.3'))).toBe(0);
    expect(compareVersions(v('1.2.3-rc.4'), v('1.2.3-rc.4'))).toBe(0);
  });

  it('orders by major, then minor, then patch', () => {
    expect(compareVersions(v('1.9.9'), v('2.0.0'))).toBeLessThan(0);
    expect(compareVersions(v('1.2.9'), v('1.3.0'))).toBeLessThan(0);
    expect(compareVersions(v('1.2.3'), v('1.2.4'))).toBeLessThan(0);
  });

  it('ranks core precedence above prerelease status', () => {
    expect(compareVersions(v('1.5.0-rc.1'), v('1.4.9'))).toBeGreaterThan(0);
  });

  it('sorts a full release chain', () => {
    const sorted = [
      '1.4.0',
      '1.4.0-rc.2',
      '2.0.0-rc.1',
      '1.4.1',
      '1.4.0-rc.1',
      '1.3.7',
      '2.0.0',
      '1.4.0-rc.10'
    ]
      .map(v)
      .sort(compareVersions)
      .map(formatVersion);

    expect(sorted).toEqual([
      '1.3.7',
      '1.4.0-rc.1',
      '1.4.0-rc.2',
      '1.4.0-rc.10',
      '1.4.0',
      '1.4.1',
      '2.0.0-rc.1',
      '2.0.0'
    ]);
  });
});

describe('release line extraction', () => {
  it('extracts X.Y from a version', () => {
    expect(releaseLineOf(v('1.4.2'))).toEqual({ major: 1, minor: 4 });
    expect(releaseLineOf(v('1.4.0-rc.3'))).toEqual({ major: 1, minor: 4 });
  });

  it('formats a release line branch name', () => {
    expect(formatReleaseLine({ major: 1, minor: 4 })).toBe('release/v1.4');
    expect(formatReleaseLine({ major: 0, minor: 0 })).toBe('release/v0.0');
  });

  it('maps a version to its release branch', () => {
    expect(releaseBranchOf(v('1.4.2'))).toBe('release/v1.4');
    expect(releaseBranchOf(v('10.0.0-rc.1'))).toBe('release/v10.0');
  });

  it('compares release lines', () => {
    expect(
      releaseLinesEqual({ major: 1, minor: 4 }, { major: 1, minor: 4 })
    ).toBe(true);
    expect(
      releaseLinesEqual({ major: 1, minor: 4 }, { major: 1, minor: 5 })
    ).toBe(false);
  });
});

describe('parseReleaseLine', () => {
  it.each([
    ['release/v1.4', { major: 1, minor: 4 }],
    ['release/v0.0', { major: 0, minor: 0 }],
    ['release/v10.20', { major: 10, minor: 20 }]
  ])('accepts %s', (input, expected) => {
    expect(parseReleaseLine(input)).toEqual(expected);
  });

  it.each([
    ['release/1.4', 'missing v'],
    ['release/v1.4.0', 'includes a patch'],
    ['release/v1', 'missing minor'],
    ['refs/heads/release/v1.4', 'fully qualified ref'],
    ['releases/v1.4', 'wrong prefix'],
    ['release/v01.4', 'leading zero in major'],
    ['release/v1.04', 'leading zero in minor'],
    [' release/v1.4', 'leading whitespace'],
    ['release/v1.4 ', 'trailing whitespace'],
    ['RELEASE/v1.4', 'uppercase prefix'],
    ['release/V1.4', 'uppercase V'],
    ['1.4', 'bare line'],
    ['', 'empty string']
  ])('rejects %s (%s)', (input) => {
    expect(parseReleaseLine(input)).toBeNull();
  });
});

describe('isInitialLineVersion', () => {
  it.each(['1.4.0', '1.4.0-rc.1', '0.0.0'])('accepts %s', (input) => {
    expect(isInitialLineVersion(v(input))).toBe(true);
  });

  it.each(['1.4.1', '1.4.2-rc.1', '0.0.9'])('rejects %s', (input) => {
    expect(isInitialLineVersion(v(input))).toBe(false);
  });
});

describe('tagName and majorAlias', () => {
  it('uses unprefixed canonical tags', () => {
    expect(tagName(v('1.4.2'))).toBe('1.4.2');
    expect(tagName(v('1.4.0-rc.1'))).toBe('1.4.0-rc.1');
  });

  it('derives the floating major alias', () => {
    expect(majorAlias(v('1.4.2'))).toBe('1');
    expect(majorAlias(v('10.0.0-rc.3'))).toBe('10');
    expect(majorAlias(v('0.1.0'))).toBe('0');
  });
});
