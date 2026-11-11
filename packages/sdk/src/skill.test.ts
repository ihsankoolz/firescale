import { describe, expect, it } from 'vitest';
import { parseSkill, validateSkill } from './skill.js';

const RULE_ID = 'core/example-rule';

interface DocParts {
  frontmatter: string;
  sections: string[];
  deterministic: boolean;
  report: boolean;
  verify: string;
}

function build(overrides: Partial<DocParts> = {}): string {
  const parts: DocParts = {
    frontmatter: 'name: example-rule\ndescription: Fixes the example rule.',
    sections: ['What this rule enforces and why', 'Manual fix', 'Agent fix instructions', 'Verify'],
    deterministic: true,
    report: true,
    verify: '```\nfscale check --rules core/example-rule\n```',
    ...overrides,
  };
  const bodies: Record<string, string> = {
    'What this rule enforces and why': 'Something must be true.',
    'Manual fix': '1. Do it.',
    'Agent fix instructions': [
      'Fix only the reported files.',
      ...(parts.deterministic ? ['', '### DETERMINISTIC', '', '1. Step.'] : []),
      ...(parts.report ? ['', '### REPORT', '', '- None.'] : []),
    ].join('\n'),
    Verify: parts.verify,
  };
  const body = parts.sections.map((s) => `## ${s}\n\n${bodies[s] ?? ''}\n`).join('\n');
  return `---\n${parts.frontmatter}\n---\n\n# ${RULE_ID}\n\n${body}`;
}

function problems(text: string, ruleId: string | undefined = RULE_ID): string[] {
  return validateSkill(text, ruleId === undefined ? {} : { ruleId });
}

describe('parseSkill', () => {
  it('splits frontmatter and body', () => {
    const parsed = parseSkill('---\nname: a\ndescription: "Quoted"\n---\n# Body\n');
    expect(parsed?.frontmatter).toEqual({ name: 'a', description: 'Quoted' });
    expect(parsed?.body).toBe('# Body\n');
  });

  it('returns null without frontmatter', () => {
    expect(parseSkill('# Just a heading\n')).toBeNull();
  });

  it('normalises CRLF and a BOM', () => {
    const parsed = parseSkill('\uFEFF---\r\nname: a\r\n---\r\nbody\r\n');
    expect(parsed?.frontmatter).toEqual({ name: 'a' });
    expect(parsed?.body).toBe('body\n');
  });
});

describe('validateSkill', () => {
  it('accepts a valid document', () => {
    expect(problems(build())).toEqual([]);
  });

  it('accepts a valid document without a ruleId expectation', () => {
    expect(problems(build(), undefined)).toEqual([]);
  });

  it('still validates with CRLF line endings', () => {
    expect(problems(build().replaceAll('\n', '\r\n'))).toEqual([]);
  });

  it('reports missing frontmatter', () => {
    expect(problems('# Title\n\n## Verify\n')).toEqual([
      expect.stringContaining('Missing frontmatter'),
    ]);
  });

  it('reports a missing name', () => {
    expect(problems(build({ frontmatter: 'description: x' }))).toContainEqual(
      expect.stringContaining('missing "name"'),
    );
  });

  it('reports a badly formed name', () => {
    const result = problems(build({ frontmatter: 'name: Bad_Name\ndescription: x' }), undefined);
    expect(result).toContainEqual(expect.stringContaining('lowercase letters'));
  });

  it('reports a name that does not match the rule id', () => {
    const result = problems(build({ frontmatter: 'name: other\ndescription: x' }));
    expect(result).toContainEqual(expect.stringContaining('to match the rule id'));
  });

  it('reports a missing description', () => {
    expect(problems(build({ frontmatter: 'name: example-rule' }))).toContainEqual(
      expect.stringContaining('missing "description"'),
    );
  });

  it('reports a custom frontmatter field', () => {
    const result = problems(
      build({ frontmatter: 'name: example-rule\ndescription: x\nseverity: high' }),
    );
    expect(result).toContainEqual(expect.stringContaining('"severity"'));
  });

  it.each(['What this rule enforces and why', 'Manual fix', 'Agent fix instructions', 'Verify'])(
    'reports missing section %s',
    (section) => {
      const sections = [
        'What this rule enforces and why',
        'Manual fix',
        'Agent fix instructions',
        'Verify',
      ].filter((s) => s !== section);
      expect(problems(build({ sections }))).toContainEqual(
        expect.stringContaining(`Missing section "## ${section}"`),
      );
    },
  );

  it('reports sections out of order', () => {
    const result = problems(
      build({
        sections: [
          'Manual fix',
          'What this rule enforces and why',
          'Agent fix instructions',
          'Verify',
        ],
      }),
    );
    expect(result).toContainEqual(expect.stringContaining('out of order'));
  });

  it('reports a missing DETERMINISTIC block', () => {
    expect(problems(build({ deterministic: false }))).toContainEqual(
      expect.stringContaining('"### DETERMINISTIC"'),
    );
  });

  it('reports a missing REPORT block', () => {
    expect(problems(build({ report: false }))).toContainEqual(
      expect.stringContaining('"### REPORT"'),
    );
  });

  it('reports the wrong verify command', () => {
    const result = problems(build({ verify: '```\nfscale check --rules core/other\n```' }));
    expect(result).toContainEqual(expect.stringContaining('"Verify" must contain'));
  });

  describe('code fences', () => {
    const fence = (inner: string, mark = '```'): string => `${mark}\n${inner}\n${mark}`;

    it.each([
      ['Verify', '## Verify'],
      ['Manual fix', '## Manual fix'],
    ])('reports a section whose only heading is inside a fence: %s', (section, heading) => {
      const sections = [
        'What this rule enforces and why',
        'Manual fix',
        'Agent fix instructions',
        'Verify',
      ].filter((s) => s !== section);
      const doc = `${build({ sections })}\n${fence(heading)}\n`;
      expect(problems(doc)).toContainEqual(
        expect.stringContaining(`Missing section "## ${section}"`),
      );
    });

    it('reports a REPORT block whose only heading is inside a fence', () => {
      const doc = build({ report: false }).replace(
        '1. Step.',
        `1. Step.\n\n${fence('### REPORT\n\n- Fake.')}`,
      );
      expect(problems(doc)).toContainEqual(expect.stringContaining('"### REPORT"'));
    });

    it('does not cut a section short at a heading-like line inside a fence', () => {
      const verify = `${fence('## Not a heading\n# Nor this')}\n\n${fence('fscale check --rules core/example-rule')}`;
      expect(problems(build({ verify }))).toEqual([]);
      const manual = build().replace('1. Do it.', `1. Do it.\n\n${fence('## Not a heading')}`);
      expect(problems(manual)).toEqual([]);
    });

    it('does not treat a fenced heading as the end of an otherwise empty section', () => {
      const doc = build().replace('1. Do it.', fence('## Not a heading'));
      expect(problems(doc)).toEqual([]);
    });

    it('supports tilde fences and indented fences', () => {
      const verify = `~~~\n## Not a heading\nfscale check --rules core/example-rule\n~~~`;
      expect(problems(build({ verify }))).toEqual([]);
      const indented = `   \`\`\`\n## Not a heading\nfscale check --rules core/example-rule\n   \`\`\``;
      expect(problems(build({ verify: indented }))).toEqual([]);
      const hidden = build({ sections: ['Manual fix'] }) + `\n~~~\n## Verify\n~~~\n`;
      expect(problems(hidden)).toContainEqual(
        expect.stringContaining('Missing section "## Verify"'),
      );
    });

    it('closes a fence only with the same character and at least the same length', () => {
      // The ~~~ and the short ``` lines do not close the four-backtick fence.
      const verify =
        '````\n~~~\n```\n## Verify\n````\n\n```\nfscale check --rules core/example-rule\n```';
      expect(problems(build({ verify }))).toEqual([]);
      const doc = build({ sections: ['Manual fix'] }) + '\n````\n```\n## Verify\n````\n';
      expect(problems(doc)).toContainEqual(expect.stringContaining('Missing section "## Verify"'));
    });
  });
});
